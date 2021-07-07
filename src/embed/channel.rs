use crate::event::Event;
use anyhow::{Context as _, Result};
use derivative::Derivative;
use futures::prelude::*;
use serenity::{
    model::{channel::Message, id::ChannelId},
    CacheAndHttp,
};
use std::{cmp, collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{debug, error, warn};

#[derive(Debug)]
pub enum EventChange {
    Added(Arc<Event>),
    Deleted(Arc<Event>),
    Edited(Arc<Event>),
}

const CHANNEL_UPDATER_DELAY_PER_RETRY: u64 = 5;
const CHANNEL_UPDATER_DELAY_CAP: u64 = 60;

/// Wraps a single "event channel", i.e. a channel that events are automatically posted to based on
/// a filter.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct EventChannel {
    send: mpsc::Sender<EventChange>,
}

impl EventChannel {
    pub fn new<'a, F, I>(
        http: Arc<CacheAndHttp>,
        channel: ChannelId,
        filter: Box<F>,
        initial_events: I,
    ) -> Self
    where
        F: FnMut(&Event) -> bool + Send + Sync + 'static,
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let events = ChannelEvents::new(filter, initial_events);
        let (send, recv) = mpsc::channel(EVENT_CHANGE_BUFFER_SIZE);
        tokio::spawn(Self::event_processing_loop(http, channel, recv, events));

        Self { send }
    }

    async fn event_processing_loop(
        http: Arc<CacheAndHttp>,
        channel: ChannelId,
        mut recv: mpsc::Receiver<EventChange>,
        mut events: ChannelEvents,
    ) {
        let mut retry = 0;
        loop {
            // Initialize a new ChannelUpdater. This gets the current messages in the channel
            // and compares them against the given events, updating as necessary to ensure our
            // state is consistent and ready to apply new event changes.
            let mut updater = match ChannelUpdater::new(http.clone(), channel, &events).await {
                Ok(updater) => updater,
                Err(err) => {
                    error!("Error creating ChannelUpdater, retry {}: {}", retry, err);

                    let delay =
                        CHANNEL_UPDATER_DELAY_CAP.min(retry * CHANNEL_UPDATER_DELAY_PER_RETRY);
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    retry += 1;
                    continue;
                }
            };
            retry = 0;

            // Process new event updates as they occur.
            'events: while let Some(change) = recv.recv().await {
                let updates = events.apply_event_change(change);
                for update in updates {
                    debug!("Applying event channel update: {:?}", update);
                    if let Err(err) = updater.apply_update(update).await {
                        error!("Error processing channel update: {:?}", err);
                        break 'events;
                    }
                }
            }

            // If an error occurs handling an event update, ChannelUpdater's state may be out of
            // sync, so throw it away and create a new ChannelUpdater.
            error!("ChannelUpdater error, restarting loop");
        }
    }

    pub async fn handle_event_change(&self, change: EventChange) {
        match self.send.try_send(change) {
            Ok(()) => {}
            Err(try_send_err) => match try_send_err {
                TrySendError::Full(change) => {
                    warn!("ChannelUpdater channel full when adding event change!");
                    if let Err(_) = self.send.send(change).await {
                        panic!("ChannelUpdater channel unexpectedly closed");
                    }
                }
                TrySendError::Closed(_) => {
                    panic!("ChannelUpdater channel unexpectedly closed");
                }
            },
        }
    }
}

/// A single update to an event channel.
#[derive(Debug, PartialEq, Eq)]
enum ChannelUpdate<'a> {
    /// Create a new message at the end of this channel for the given event.
    New { event: &'a Arc<Event> },
    /// Update the channel's message at idx with the given event.
    Update { event: &'a Arc<Event>, idx: usize },
    /// Delete the channel's message at idx.
    Delete { idx: usize },
}

// Rather than using an unbounded channel, which makes it impossible to get a signal if we're
// generating changes faster than they can be processed, this is an arbitrary buffer size and then
// check when sending if the buffer is currently full so that we can log.
const EVENT_CHANGE_BUFFER_SIZE: usize = 10;

// ChannelUpdater performs all updating of event embeds in event channels. It receives actions to
// apply from EventChannel, calculated by ChannelEvents, and applies them in order.
//
// It also keeps track of the message IDs for event channels. This avoids racey behavior when adding
// new messages, since the message ID then won't be known until ChannelUpdater creates it. Instead,
// ChannelEvents only keeps track of event ordering in a channel and then specifies actions in terms
// of indexes, and ChannelUpdater turns that into a message ID.
//
// TODO: Handle our messages getting deleted not by us, state would be stale.
struct ChannelUpdater {
    http: Arc<CacheAndHttp>,
    channel: ChannelId,
    messages: Vec<Message>,
}

impl ChannelUpdater {
    /// Creates a new ChannelUpdater, populating its state with the channel's current messages and
    /// updating those messages as needed to match the provided ChannelEvents, such that the
    /// ChannelUpdater is ready to apply updates for new event changes (through `apply_update`).
    pub async fn new(
        http: Arc<CacheAndHttp>,
        channel: ChannelId,
        events: &ChannelEvents,
    ) -> Result<Self> {
        let mut updater = ChannelUpdater {
            http,
            channel,
            messages: Vec::new(),
        };

        updater.populate_current_messages().await?;
        debug!(
            "ChannelUpdater {}: Initial messages: {:?}",
            updater.channel, updater.messages
        );

        let initial_updates = updater.updates_needed_to_match_events(events);
        debug!(
            "ChannelUpdater {}: Initial updates: {:?}",
            updater.channel, initial_updates
        );

        for update in initial_updates {
            updater.apply_update(update).await?;
        }

        debug!("ChannelUpdater {} ready", updater.channel);
        Ok(updater)
    }

    async fn populate_current_messages(&mut self) -> Result<()> {
        let cache = &self.http.cache;
        let mut messages: Vec<_> = self
            .channel
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

        // The returned messages have the newest first, so reverse the order.
        messages.reverse();

        self.messages = messages;
        Ok(())
    }

    fn updates_needed_to_match_events<'a>(
        &self,
        events: &'a ChannelEvents,
    ) -> Vec<ChannelUpdate<'a>> {
        let events = &events.events;

        // Update existing messages as needed.
        let updates = events
            .iter()
            .zip(self.messages.iter())
            .enumerate()
            .filter_map(|(idx, (event, _message))| {
                // TODO: Instead of just updating everything, detect which messages need to be
                // updated by checking the content of the embed.
                Some(ChannelUpdate::Update { event, idx })
            });

        // Only new or delete will yield any elements, not both, but this lets us simply chain the
        // iterators together.
        let delete = (events.len()..self.messages.len()).map(|idx| ChannelUpdate::Delete { idx });
        let new = events
            .iter()
            .skip(self.messages.len())
            .map(|event| ChannelUpdate::New { event });

        updates.chain(delete).chain(new).collect()
    }

    pub async fn apply_update(&mut self, update: ChannelUpdate<'_>) -> Result<()> {
        match update {
            ChannelUpdate::New { event } => {
                let message = self
                    .channel
                    .send_message(&self.http.http, |msg| {
                        msg.set_embed(event.as_embed()).components(|c| {
                            *c = event.event_buttons();
                            c
                        })
                    })
                    .await
                    .context("Failed to send new message to channel")?;
                self.messages.push(message);
            }
            ChannelUpdate::Update { event, idx } => {
                let message = self
                    .messages
                    .get_mut(idx)
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
            ChannelUpdate::Delete { idx } => {
                let message = self.messages.remove(idx);
                message
                    .delete(&self.http)
                    .await
                    .context("Failed to delete message")?;
            }
        }
        Ok(())
    }
}

struct ChannelEvents {
    filter: Box<dyn FnMut(&Event) -> bool + Send + Sync + 'static>,

    // Note that this relies on Event's Ord implementation that orders by event datetime.
    events: BTreeSet<Arc<Event>>,
}

impl ChannelEvents {
    pub fn new<'a, F, I>(mut filter: Box<F>, initial_events: I) -> Self
    where
        F: FnMut(&Event) -> bool + Send + Sync + 'static,
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let events = initial_events.filter(|e| filter(e)).cloned().collect();
        Self { filter, events }
    }

    pub fn apply_event_change(
        &mut self,
        change: EventChange,
    ) -> impl Iterator<Item = ChannelUpdate<'_>> + '_ {
        // Check if there's an old event with a matching ID that needs to be removed. May not
        // exist if previously did not meet filter.
        let old_idx = match &change {
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
                    let id = change.id;
                    self.events.insert(change);
                    Some(
                        self.events
                            .iter()
                            .position(|e| e.id == id)
                            .expect("Couldn't find just-inserted value"),
                    )
                } else {
                    None
                }
            }
            EventChange::Deleted(_) => None,
        };

        self.create_updates(old_idx, new_idx)
    }

    /// Create all the channel updates to apply given an "old_idx" where an event was removed or
    /// replaced and a "new_idx" where an event was added or replaced.
    fn create_updates(
        &self,
        old_idx: Option<usize>,
        new_idx: Option<usize>,
    ) -> impl Iterator<Item = ChannelUpdate<'_>> + '_ {
        let mut last_action = None;
        let update_range = match (old_idx, new_idx) {
            (None, None) => 0..0,
            (None, Some(new)) => {
                // Update [new,len] + New
                let event = self
                    .events
                    .iter()
                    .last()
                    .expect("Events shouldn't be empty");
                last_action = Some(ChannelUpdate::New { event });
                new..self.events.len() - 1
            }
            (Some(old), None) => {
                // Delete old
                last_action = Some(ChannelUpdate::Delete { idx: old });
                0..0
            }
            (Some(old), Some(new)) => {
                // Update [min(old,new), max(old,new)]
                let (min, max) = (cmp::min(old, new), cmp::max(old, new));
                min..max + 1
            }
        };

        self.events
            .iter()
            .enumerate()
            .filter(move |(i, _)| update_range.contains(i))
            .map(move |(idx, event)| ChannelUpdate::Update { event, idx })
            .chain(last_action)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::activity::{Activity, ActivityType};
    use crate::event::{Event, EventId};
    use chrono::{Duration, Utc};
    use chrono_tz::Tz;
    use std::iter;

    fn test_event(activity: Activity, idx: u8, hours_away: i64) -> Arc<Event> {
        Arc::new(Event {
            id: EventId { activity, idx },
            activity,
            datetime: Utc::now().with_timezone(&Tz::PST8PDT) + Duration::hours(hours_away),
            ..Default::default()
        })
    }

    fn new_action(event: &Arc<Event>) -> ChannelUpdate {
        ChannelUpdate::New { event }
    }

    fn update_action(event: &Arc<Event>, idx: usize) -> ChannelUpdate {
        ChannelUpdate::Update { event, idx }
    }

    fn delete_action(idx: usize) -> ChannelUpdate<'static> {
        ChannelUpdate::Delete { idx }
    }

    #[test]
    fn add_update_delete_matching_event() {
        let mut chan = ChannelEvents::new(
            Box::new(|event: &Event| event.activity.activity_type() == ActivityType::Raid),
            iter::empty(),
        );

        let event = test_event(Activity::DeepStoneCrypt, 1, 0);
        assert_eq!(
            chan.apply_event_change(EventChange::Added(event.clone()))
                .collect::<Vec<_>>(),
            vec![new_action(&event)]
        );

        let mut event = event.clone();
        Arc::make_mut(&mut event).group_size = 4;
        assert_eq!(
            chan.apply_event_change(EventChange::Edited(event.clone()))
                .collect::<Vec<_>>(),
            vec![update_action(&event, 0)]
        );

        assert_eq!(
            chan.apply_event_change(EventChange::Deleted(event.clone()))
                .collect::<Vec<_>>(),
            vec![delete_action(0)]
        );
    }

    #[test]
    fn add_edit_delete_earlier_events_test() {
        let mut chan = ChannelEvents::new(
            Box::new(|event: &Event| event.activity.activity_type() == ActivityType::Raid),
            iter::empty(),
        );

        let event1 = test_event(Activity::DeepStoneCrypt, 10, 0);
        let event2 = test_event(Activity::VaultOfGlass, 20, 1);
        let event3 = test_event(Activity::LastWish, 30, 2);

        // Add the latest event first.
        assert_eq!(
            chan.apply_event_change(EventChange::Added(event3.clone()))
                .collect::<Vec<_>>(),
            vec![new_action(&event3)]
        );

        // Add the earliest event.
        assert_eq!(
            chan.apply_event_change(EventChange::Added(event1.clone()))
                .collect::<Vec<_>>(),
            vec![update_action(&event1, 0), new_action(&event3),]
        );

        // Add the middle event.
        assert_eq!(
            chan.apply_event_change(EventChange::Added(event2.clone()))
                .collect::<Vec<_>>(),
            vec![update_action(&event2, 1), new_action(&event3),]
        );

        // Update the time of the latest event so that it is now the earliest.
        let mut event0 = event3.clone();
        Arc::make_mut(&mut event0).datetime = event3.datetime - Duration::hours(4);
        assert_eq!(
            chan.apply_event_change(EventChange::Edited(event0.clone()))
                .collect::<Vec<_>>(),
            vec![
                update_action(&event0, 0),
                update_action(&event1, 1),
                update_action(&event2, 2),
            ]
        );

        // Delete the middle event.
        assert_eq!(
            chan.apply_event_change(EventChange::Deleted(event1.clone()))
                .collect::<Vec<_>>(),
            vec![delete_action(1)],
        );

        // Edit the earliest event so that it no longer matches the filter.
        let mut event0 = event0.clone();
        Arc::make_mut(&mut event0).activity = Activity::Presage;
        assert_eq!(
            chan.apply_event_change(EventChange::Edited(event0.clone()))
                .collect::<Vec<_>>(),
            vec![delete_action(0)],
        );
    }
}
