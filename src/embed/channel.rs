use crate::event::{Event, EventChange};
use anyhow::{format_err, Context as _, Result};
use derivative::Derivative;
use futures::prelude::*;
use serenity::{
    builder::CreateEmbed,
    collector::{EventCollector, EventCollectorBuilder},
    model::{
        channel::{Message, MessageFlags},
        event::{Event as DiscordEvent, EventType},
        id::{ChannelId, GuildId},
    },
    prelude::*,
};
use std::{cmp, collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{debug, error, warn};

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
        ctx: Context,
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
        tokio::spawn(Self::event_processing_loop(ctx, channel, recv, events));

        Self { send }
    }

    async fn event_processing_loop(
        ctx: Context,
        channel: ChannelId,
        mut recv: mpsc::Receiver<EventChange>,
        mut events: ChannelEvents,
    ) -> ! {
        let mut retry = 0;
        loop {
            // Initialize a new ChannelUpdater. This gets the current messages in the channel
            // and compares them against the given events, updating as necessary to ensure our
            // state is consistent and ready to apply new event changes.
            let mut updater = match ChannelUpdater::new(ctx.clone(), channel, &events).await {
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

            'restart_updater: loop {
                tokio::select! {
                    // Let ChannelUpdater handle Discord message events as they come in. This will only
                    // yield a value if an error occurs while handling events, otherwise the select
                    // polling will keep handling message events.
                    updater_event = updater.next_updater_event() => {
                        match updater_event {
                            Ok(updater_event) => if let Err(err) = updater.process_updater_event(updater_event, &events).await {
                                error!("Error processing ChannelUpdaterEvent: {:?}", err);
                                break 'restart_updater;
                            }
                            Err(err) => {
                                error!("Error getting next ChannelUpdaterEvent: {:?}", err);
                                break 'restart_updater;
                            }
                        }
                    }

                    // Process new event updates as they occur.
                    Some(change) = recv.recv() => {
                        let updates = events.apply_event_change(change);
                        for update in updates {
                            debug!("Applying event channel update: {:?}", update);
                            if let Err(err) = updater.apply_update(update).await {
                                error!("Error processing channel update: {:?}", err);
                                break 'restart_updater;
                            }
                        }
                    }
                };
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

struct ChannelUpdaterEvent(Arc<DiscordEvent>);

// ChannelUpdater performs all updating of event embeds in event channels. It receives actions to
// apply from EventChannel, calculated by ChannelEvents, and applies them in order.
//
// It also keeps track of the message IDs for event channels. This avoids racey behavior when adding
// new messages, since the message ID then won't be known until ChannelUpdater creates it. Instead,
// ChannelEvents only keeps track of event ordering in a channel and then specifies actions in terms
// of indexes, and ChannelUpdater turns that into a message ID.
struct ChannelUpdater {
    ctx: Context,
    channel: ChannelId,
    messages: Vec<Message>,

    // Note that the "Event" in EventCollector is referring to Discord gateway events.
    collector: EventCollector,
}

impl ChannelUpdater {
    /// Creates a new ChannelUpdater, populating its state with the channel's current messages and
    /// updating those messages as needed to match the provided ChannelEvents, such that the
    /// ChannelUpdater is ready to apply updates for new event changes (through `apply_update`).
    pub async fn new(ctx: Context, channel: ChannelId, events: &ChannelEvents) -> Result<Self> {
        // Set up a collector for any message change events in this channel that aren't from the bot.
        let own_id = ctx.cache.current_user_id().await;
        let collector = EventCollectorBuilder::new(&ctx)
            .add_event_type(EventType::MessageCreate)
            .add_event_type(EventType::MessageUpdate)
            .add_event_type(EventType::MessageDelete)
            .add_event_type(EventType::MessageDeleteBulk)
            .add_channel_id(channel)
            .filter(move |e| match e.as_ref() {
                // Don't care about our own message create events. If we could filter out our own
                // updates and deletes here we would, but the event doesn't say who performed the
                // update/delete.
                DiscordEvent::MessageCreate(e) => e.message.author.id != own_id,
                _ => true,
            })
            .await
            .expect("Bad EventCollector filter setup");

        let mut updater = ChannelUpdater {
            ctx,
            channel,
            messages: Vec::new(),
            collector,
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

    pub async fn next_updater_event(&mut self) -> Result<ChannelUpdaterEvent> {
        Ok(ChannelUpdaterEvent(
            self.collector
                .next()
                .await
                .ok_or_else(|| format_err!("Collector stream returned None, closed?"))?,
        ))
    }

    pub async fn process_updater_event(
        &mut self,
        updater_event: ChannelUpdaterEvent,
        events: &ChannelEvents,
    ) -> Result<()> {
        let prev_len = self.messages.len();
        match updater_event.0.as_ref() {
            DiscordEvent::MessageCreate(e) => {
                // The collector filter already filtered out our own messages, so this is
                // someone else; delete it.
                e.message.delete(&self.ctx).await.with_context(|| {
                    format!(
                        "Failed to delete message {} in channel {}",
                        e.message.id, self.channel
                    )
                })?;
            }
            DiscordEvent::MessageUpdate(e) => {
                // Others can only suppress embeds, any other edits are from the bot.
                if let Some(flags) = e.flags {
                    if flags.contains(MessageFlags::SUPPRESS_EMBEDS) {
                        if let Some(existing) = self.messages.iter_mut().find(|m| m.id == e.id) {
                            existing
                                .edit(&self.ctx, |msg| msg.suppress_embeds(false))
                                .await
                                .with_context(|| {
                                    format!("Failed to un-suppress embeds on message {}", e.id)
                                })?;
                        } else {
                            error!("MessageUpdate event for unknown message {}", e.id);
                        }
                    }
                }
            }
            DiscordEvent::MessageDelete(e) => {
                self.messages.retain(|m| m.id != e.message_id);
            }
            DiscordEvent::MessageDeleteBulk(e) => {
                self.messages.retain(|m| !e.ids.contains(&m.id));
            }
            e => error!("Collector got unexpected event: {:?}", e),
        }

        // Repair channel if messages were deleted.
        if prev_len != self.messages.len() {
            let updates = self.updates_needed_to_match_events(events);
            for update in updates {
                self.apply_update(update)
                    .await
                    .context("Error while repairing channel messages after delete")?;
            }
        }

        Ok(())
    }

    async fn populate_current_messages(&mut self) -> Result<()> {
        let own_id = self.ctx.cache.current_user_id().await;
        let mut messages: Vec<_> = self
            .channel
            .messages_iter(&self.ctx)
            .try_filter_map(|mut msg| async {
                if msg.author.id != own_id {
                    // Delete messages that aren't from the bot.
                    // TODO(serenity-rs/serenity#1439): We set guild ID to something non-None
                    // because guild_id is missing for messages acquired over the HTTP API, which
                    // confuses delete() into thinking this is a private message we can't delete.
                    // The guild id doesn't actually have to be correct.
                    msg.guild_id = Some(GuildId(1));
                    if let Err(err) = msg.delete(&self.ctx).await {
                        error!("Failed to delete non-own message {}: {:?}", msg.id, err);
                    }
                    return Ok(None);
                }
                Ok(Some(msg))
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
            .filter_map(|(idx, (event, message))| {
                let update = Some(ChannelUpdate::Update { event, idx });

                // Check whether the current message has embeds suppressed or whether the embed
                // isn't in sync with the correct event state and update if so.
                if message
                    .flags
                    .map_or(false, |f| f.contains(MessageFlags::SUPPRESS_EMBEDS))
                {
                    return update;
                }
                if message.embeds.len() != 1 {
                    return update;
                }
                let current = CreateEmbed::from(message.embeds[0].clone());
                let target = event.as_embed();
                if current.0 != target.0 {
                    return update;
                }
                None
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
                    .send_message(&self.ctx, |msg| {
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
                    .edit(&self.ctx, |msg| {
                        msg.set_embed(event.as_embed())
                            .components(|c| {
                                *c = event.event_buttons();
                                c
                            })
                            .suppress_embeds(false)
                    })
                    .await
                    .context("Failed to edit message")?;
            }
            ChannelUpdate::Delete { idx } => {
                let message = self.messages.remove(idx);
                message
                    .delete(&self.ctx)
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
        let mut event = Event::default();
        event.id = EventId { activity, idx };
        event.activity = activity;
        event.set_datetime(Utc::now().with_timezone(&Tz::PST8PDT) + Duration::hours(hours_away));
        Arc::new(event)
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
        Arc::make_mut(&mut event0).set_datetime(event3.datetime() - Duration::hours(4));
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
