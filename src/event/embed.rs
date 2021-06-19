use super::Event;
use crate::activity::ActivityType;
use anyhow::{Context as _, Result};
use futures::prelude::*;
use serenity::{
    model::{channel::Message, id::ChannelId},
    CacheAndHttp,
};
use std::{
    cmp,
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{debug, error, warn};

struct EventChannel {
    channel: ChannelId,
    filter: Box<dyn FnMut(&Event) -> bool + Send + Sync + 'static>,
    // Note that this relies on Event's Ord implementation that orders by event datetime.
    events: BTreeSet<Arc<Event>>,
}

#[derive(Debug)]
enum EventChange<'a> {
    Added(&'a Arc<Event>),
    Deleted(&'a Arc<Event>),
    Edited(&'a Arc<Event>),
}

impl EventChannel {
    pub fn new<'a, F, I>(channel: ChannelId, mut filter: Box<F>, initial_events: I) -> Self
    where
        F: FnMut(&Event) -> bool + Send + Sync + 'static,
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let events = initial_events.filter(|e| filter(e)).cloned().collect();
        EventChannel {
            channel,
            filter,
            events,
        }
    }

    pub fn handle_event_change(
        &mut self,
        change: EventChange<'_>,
    ) -> impl Iterator<Item = EmbedAction> + '_ {
        // Check if there's an old event with a matching ID that needs to be removed. May not
        // exist if previously did not meet filter.
        let old_idx = match change {
            EventChange::Deleted(change) | EventChange::Edited(change) => {
                // This might be better with drain_filter() once that is stabilized.
                let old = self
                    .events
                    .iter()
                    .enumerate()
                    .find(|(_, old)| old.id == change.id)
                    .map(|(idx, old)| (idx, old.clone()));

                if let Some((old_idx, old)) = old {
                    self.events.remove(&old);
                    Some(old_idx)
                } else {
                    None
                }
            }
            EventChange::Added(_) => None,
        };

        // Insert only if event still meets filter.
        let new_idx = match change {
            EventChange::Added(change) | EventChange::Edited(change) => {
                if (self.filter)(&change) {
                    self.events.insert(change.clone());
                    Some(
                        self.events
                            .iter()
                            .position(|e| e.id == change.id)
                            .expect("Couldn't find just-inserted value"),
                    )
                } else {
                    None
                }
            }
            EventChange::Deleted(_) => None,
        };

        self.create_embed_actions(old_idx, new_idx)
    }

    /// Create all the embed actions needed to update the channel, given an "old_idx" where an event
    /// was removed or replaced and a "new_idx" where an event was added or replaced.
    fn create_embed_actions(
        &self,
        old_idx: Option<usize>,
        new_idx: Option<usize>,
    ) -> impl Iterator<Item = EmbedAction> + '_ {
        let mut last_action = None;
        let update_range = match (old_idx, new_idx) {
            (None, None) => 0..0,
            (None, Some(new)) => {
                // Update [new,len] + New
                let event = self
                    .events
                    .iter()
                    .last()
                    .expect("Events shouldn't be empty")
                    .clone();
                last_action = Some(EmbedAction::New {
                    event,
                    chan: self.channel,
                });
                new..self.events.len() - 1
            }
            (Some(old), None) => {
                // Delete old
                last_action = Some(EmbedAction::Delete {
                    chan: self.channel,
                    idx: old,
                });
                0..0
            }
            (Some(old), Some(new)) => {
                // Update [min(old,new), max(old,new)]
                let (min, max) = (cmp::min(old, new), cmp::max(old, new));
                min..max + 1
            }
        };

        let chan = self.channel;
        self.events
            .iter()
            .enumerate()
            .filter(move |(i, _)| update_range.contains(i))
            .map(move |(idx, event)| EmbedAction::Update {
                event: event.clone(),
                chan,
                idx,
            })
            .chain(last_action)
    }
}

// TODO: Replace this hardcoded event channel configuration with commands to configure this.
fn test_event_channels<'a, I>(initial_events: I) -> Vec<EventChannel>
where
    I: Iterator<Item = &'a Arc<Event>> + Clone,
{
    let mut v = Vec::new();
    // #raid-lfg
    v.push(EventChannel::new(
        ChannelId(853744114377687090),
        Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Raid),
        initial_events.clone(),
    ));
    // #pve-lfg
    v.push(EventChannel::new(
        ChannelId(853744129774452766),
        Box::new(|e: &Event| match e.activity.activity_type() {
            ActivityType::Dungeon
            | ActivityType::Gambit
            | ActivityType::ExoticQuest
            | ActivityType::Seasonal
            | ActivityType::Other => true,
            _ => false,
        }),
        initial_events.clone(),
    ));
    // #pvp-lfg
    v.push(EventChannel::new(
        ChannelId(853744160926597150),
        Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Crucible),
        initial_events.clone(),
    ));
    // #special-lfg
    v.push(EventChannel::new(
        ChannelId(853744175908257813),
        Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Custom),
        initial_events.clone(),
    ));
    // #all-lfg
    v.push(EventChannel::new(
        ChannelId(853744186545274880),
        Box::new(|_: &Event| true),
        initial_events,
    ));
    v
}

// Rather than using an unbounded channel, which makes it impossible to get a signal if we're
// generating actions faster than they can be processed, this is an arbitrary buffer size and then
// check when sending if the buffer is currently full so that we can log.
const EMBED_ACTION_BUFFER_SIZE: usize = 20;

// TODO: Use a MessageCollector to collect messages in the event channels that aren't from the bot
// and delete them.
pub struct EmbedManager {
    /// Configuration data for event channels, i.e. channels that events are automatically posted to
    /// based on a filter.
    event_channels: Vec<EventChannel>,
    action_send: mpsc::Sender<EmbedAction>,
}

impl EmbedManager {
    pub fn new<'a, I>(http: Arc<CacheAndHttp>, initial_events: I) -> Self
    where
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let event_channels = test_event_channels(initial_events);

        let (send, recv) = mpsc::channel(EMBED_ACTION_BUFFER_SIZE);
        EmbedUpdater::start_processing_actions(http, recv);

        EmbedManager {
            event_channels,
            action_send: send,
        }
    }

    pub async fn event_added(&mut self, new: Arc<Event>) {
        for chan in self.event_channels.iter_mut() {
            for action in chan.handle_event_change(EventChange::Added(&new)) {
                send_log_on_backpressure(&self.action_send, action).await;
            }
        }
    }

    pub async fn event_edited(&mut self, new: Arc<Event>) {
        for chan in self.event_channels.iter_mut() {
            for action in chan.handle_event_change(EventChange::Edited(&new)) {
                send_log_on_backpressure(&self.action_send, action).await;
            }
        }
    }

    pub async fn event_deleted(&mut self, event: Arc<Event>) {
        for chan in self.event_channels.iter_mut() {
            for action in chan.handle_event_change(EventChange::Deleted(&event)) {
                send_log_on_backpressure(&self.action_send, action).await;
            }
        }
    }
}

async fn send_log_on_backpressure<T>(send: &mpsc::Sender<T>, value: T) {
    match send.try_send(value) {
        Ok(()) => {}
        Err(try_send_err) => match try_send_err {
            TrySendError::Full(value) => {
                warn!("EmbedUpdater channel full when adding action!");
                if let Err(_) = send.send(value).await {
                    panic!("EmbedUpdater channel unexpectedly closed");
                }
            }
            TrySendError::Closed(_) => {
                panic!("EmbedUpdater channel unexpectedly closed");
            }
        },
    }
}

#[derive(Debug)]
enum EmbedAction {
    // Create a new message at the end of this channel for the given event.
    New {
        event: Arc<Event>,
        chan: ChannelId,
    },
    // Update the channel's message at idx with the given event.
    Update {
        event: Arc<Event>,
        chan: ChannelId,
        idx: usize,
    },
    // Delete the channel's message at idx.
    Delete {
        chan: ChannelId,
        idx: usize,
    },
}

impl EmbedAction {
    pub fn channel(&self) -> ChannelId {
        match self {
            EmbedAction::New { event: _, chan }
            | EmbedAction::Update {
                event: _,
                chan,
                idx: _,
            }
            | EmbedAction::Delete { chan, idx: _ } => *chan,
        }
    }
}

// EmbedUpdater performs all updating of event embeds. It receives actions to apply from
// EmbedManager and applies them in order.
//
// It also keeps track of the message IDs for event channels. This avoids racey behavior when adding
// new messages, since the message ID then won't be known until EmbedUpdater creates it. Instead,
// EmbedManager only keeps track of event ordering in a channel and then specifies actions in terms
// of indexes, and EmbedUpdater turns that into a message ID. This works because there is only a
// single updater task and all updates are applied in order.
//
// TODO: Handle our messages getting deleted not by us, state would be stale.
struct EmbedUpdater {
    http: Arc<CacheAndHttp>,
    channel_messages: HashMap<ChannelId, Vec<Message>>,
}

impl EmbedUpdater {
    pub fn start_processing_actions(
        http: Arc<CacheAndHttp>,
        mut recv: mpsc::Receiver<EmbedAction>,
    ) {
        let mut updater = EmbedUpdater {
            http,
            channel_messages: HashMap::new(),
        };

        tokio::spawn(async move {
            while let Some(action) = recv.recv().await {
                debug!("Processing action: {:?}", action);
                if let Err(err) = updater.process_action(&action).await {
                    // TODO: How can we recover from a failed action better? The channel and our own
                    // internal state might be in a weird state.
                    error!("Error processing action {:?}: {:?}", action, err);
                }
            }
        });
    }

    async fn process_action(&mut self, action: &EmbedAction) -> Result<()> {
        // TODO: This currently assumes that the channel's existing messages from the bot are
        // consistent with the bot's internal event state, making this fragile. Need to improve
        // this.
        let chan = action.channel();
        let messages = match self.channel_messages.get_mut(&chan) {
            Some(m) => m,
            None => {
                // Haven't processed an action on this channel yet; fetch existing messages.
                let cache = &self.http.cache;
                let messages = chan
                    .messages_iter(&self.http.http)
                    .try_filter_map(|msg| async {
                        Ok(if msg.is_own(cache).await {
                            Some(msg)
                        } else {
                            None
                        })
                    })
                    .try_collect()
                    .await
                    .context("Failed to get channel messages")?;
                self.channel_messages.insert(chan, messages);
                self.channel_messages.get_mut(&chan).unwrap()
            }
        };

        match action {
            EmbedAction::New { event, chan } => {
                let message = chan
                    .send_message(&self.http.http, |msg| {
                        msg.set_embed(event.as_embed()).components(|c| {
                            *c = event.event_buttons();
                            c
                        })
                    })
                    .await
                    .context("Failed to send new message to channel")?;
                messages.push(message);
            }
            EmbedAction::Update {
                event,
                chan: _,
                idx,
            } => {
                let message = messages
                    .get_mut(*idx)
                    .expect("Message index OOB, state inconsistent");
                message
                    .edit(&self.http, |msg| {
                        msg.set_embed(event.as_embed()).components(|c| {
                            *c = event.event_buttons();
                            c
                        })
                    })
                    .await
                    .context("Failed to edit message")?;
            }
            EmbedAction::Delete { chan: _, idx } => {
                let message = messages.remove(*idx);
                message
                    .delete(&self.http)
                    .await
                    .context("Failed to delete message")?;
            }
        }
        Ok(())
    }
}
