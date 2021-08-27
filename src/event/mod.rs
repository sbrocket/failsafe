use crate::{
    activity::Activity,
    embed::EmbedManager,
    guild::GuildConfig,
    store::{PersistentStore, PersistentStoreBuilder},
    util::*,
};
use anyhow::{format_err, Context as _, Error, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use derivative::Derivative;
use itertools::Itertools;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serenity::{
    async_trait,
    builder::{CreateActionRow, CreateButton, CreateComponents, CreateEmbed},
    http::CacheHttp,
    model::{interactions::message_component::ButtonStyle, prelude::*},
    prelude::*,
    utils::Color,
};
use std::{
    collections::{BTreeMap, HashMap},
    convert::TryFrom,
    iter::successors,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::RwLock;
use tracing::{error, info};

mod alert;

pub use crate::embed::EventEmbedMessage;

// Debugging features, enabled through environment variables.
lazy_static! {
    // Allows the same user to join an event multiple times, to enable testing with a small number
    // of users.
    static ref ALLOW_DUPLICATE_JOIN: bool =
        std::env::var("ALLOW_DUPLICATE_JOIN").map_or(false, |v| v == "1");

    // Don't start the EventScheduler so that scheduled event actions don't occur.
    static ref DISABLE_EVENT_SCHEDULER: bool =
        std::env::var("DISABLE_EVENT_SCHEDULER").map_or(false, |v| v == "1");
}

/// Unique identifier for an Event.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct EventId {
    pub activity: Activity,
    pub idx: u8,
}

fn event_id(activity: Activity, idx: u8) -> EventId {
    EventId { activity, idx }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}{}", self.activity.id_prefix(), self.idx))
    }
}

impl From<EventId> for String {
    fn from(id: EventId) -> String {
        id.to_string()
    }
}

impl FromStr for EventId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        if let Some((split_idx, _)) = s
            .char_indices()
            .skip_while(|(_, c)| c.is_ascii_alphabetic())
            .next()
        {
            let (s1, s2) = s.split_at(split_idx);
            let activity = Activity::activity_with_id_prefix(s1)
                .ok_or_else(|| format_err!("Unknown activity prefix"))?;
            let event_idx: u8 = s2.parse().context("Invalid number in event ID")?;
            Ok(event_id(activity, event_idx))
        } else {
            Err(format_err!("Unexpected event ID format"))
        }
    }
}

impl TryFrom<String> for EventId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::from_str(&value)
    }
}

// TODO: We currently persist the last seen username. Need to make sure these stay up to date as we
// get new user info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMember {
    pub id: UserId,
    pub name: String,
}

impl PartialEq for EventMember {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for EventMember {}

impl From<&dyn MemberLike> for EventMember {
    fn from(member: &dyn MemberLike) -> Self {
        EventMember {
            id: member.user().id,
            name: member.display_name().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum JoinKind {
    Confirmed,
    Alternate,
    Maybe,
}

impl FromStr for JoinKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "confirmed" => Ok(JoinKind::Confirmed),
            "alt" => Ok(JoinKind::Alternate),
            "maybe" => Ok(JoinKind::Maybe),
            _ => Err(format_err!("Unknown join kind: {}", s)),
        }
    }
}

impl std::fmt::Display for JoinKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinKind::Confirmed => f.write_str("confirmed"),
            JoinKind::Alternate => f.write_str("a confirmed alt"),
            JoinKind::Maybe => f.write_str("a maybe"),
        }
    }
}

/// A single scheduled event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub activity: Activity,
    #[serde(with = "serialize_datetime_tz")]
    datetime: DateTime<Tz>,
    pub description: String,
    pub group_size: u8,
    pub recur: bool,
    pub creator: EventMember,
    pub confirmed: Vec<EventMember>,
    pub alternates: Vec<EventMember>,
    pub maybe: Vec<EventMember>,
    /// If alert_message is Some, alert protocol has been triggered for this event. This gets reset
    /// if the Event's time changes.
    alert_message: Option<String>,
}

#[cfg(test)]
impl Default for Event {
    fn default() -> Self {
        let creator = EventMember {
            id: UserId(1),
            name: "default".into(),
        };
        let activity = Activity::Custom;
        Event {
            id: event_id(activity, 1),
            activity,
            datetime: Utc::now().with_timezone(&Tz::PST8PDT),
            description: "".to_owned(),
            group_size: activity.default_group_size(),
            recur: false,
            creator: creator.clone(),
            confirmed: vec![creator],
            alternates: vec![],
            maybe: vec![],
            alert_message: None,
        }
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Events are ordered first by their datetime, and then by their event IDs (which are unique).
impl Ord for Event {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.datetime
            .cmp(&other.datetime)
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl Event {
    /// Get the current datetime.
    pub fn datetime(&self) -> DateTime<Tz> {
        self.datetime
    }

    pub fn set_datetime(&mut self, new: DateTime<Tz>) {
        self.datetime = new;
        self.alert_message = None;
    }

    // TODO: Add a limit on how many people can join an event.
    pub fn join(&mut self, member: &dyn MemberLike, kind: JoinKind) -> Result<()> {
        let list = match kind {
            JoinKind::Confirmed => &mut self.confirmed,
            JoinKind::Alternate => &mut self.alternates,
            JoinKind::Maybe => &mut self.maybe,
        };
        if !*ALLOW_DUPLICATE_JOIN && list.iter().any(|u| u.id == member.id()) {
            return Err(format_err!("User already in event"));
        }

        // Remove user from any other lists so that they don't end up in multiple.
        if !*ALLOW_DUPLICATE_JOIN {
            self.leave(member).ok();
        }

        match kind {
            JoinKind::Confirmed => &mut self.confirmed,
            JoinKind::Alternate => &mut self.alternates,
            JoinKind::Maybe => &mut self.maybe,
        }
        .push(member.into());
        Ok(())
    }

    pub fn leave(&mut self, member: &dyn MemberLike) -> Result<()> {
        let count_before = self.confirmed.len() + self.alternates.len() + self.maybe.len();
        self.confirmed.retain(|u| u.id != member.id());
        self.alternates.retain(|u| u.id != member.id());
        self.maybe.retain(|u| u.id != member.id());
        let count_after = self.confirmed.len() + self.alternates.len() + self.maybe.len();

        if count_before == count_after {
            return Err(format_err!("User wasn't in the event"));
        }
        Ok(())
    }

    pub fn formatted_datetime(&self) -> String {
        self.datetime.format("%-I:%M %p %Z %-m/%-d").to_string()
    }

    fn confirmed_groups(&self) -> Vec<Vec<(&EventMember, bool)>> {
        let chunk_size = self.group_size as usize;
        let combined = self
            .confirmed
            .iter()
            .map(|u| (u, false))
            .chain(self.alternates.iter().map(|u| (u, true)))
            .collect_vec();
        combined
            .chunks(chunk_size)
            .take_while(|group| group.len() == chunk_size || !group[0].1)
            .map(Vec::from)
            .collect()
    }

    fn extra_alts(&self) -> impl Iterator<Item = &EventMember> {
        let total = self.confirmed.len() + self.alternates.len();
        let partial_group = total % self.group_size as usize;
        let skip = if self.alternates.len() >= partial_group {
            self.alternates.len() - partial_group
        } else {
            self.alternates.len()
        };
        self.alternates.iter().skip(skip)
    }

    pub fn as_embed(&self) -> CreateEmbed {
        let mut embed = CreateEmbed::default();
        let mut start_time = self.formatted_datetime();
        if self.recur {
            start_time.push_str("\nRecurs weekly");
        }
        embed
            .field("Activity", self.activity, true)
            .field("Start Time", start_time, true)
            .field("Event ID", self.id, true)
            .field("Description", self.description.clone(), false)
            .color(Color::DARK_GOLD)
            .footer(|f| f.text(format!("Creator | {} | Your Time", self.creator.name)))
            .timestamp(&self.datetime.with_timezone(&Utc));

        self.confirmed_groups()
            .into_iter()
            .pad_using(1, |_| vec![])
            .enumerate()
            .for_each(|(i, group)| {
                let names: String = group
                    .iter()
                    .map(|(user, alt)| {
                        let name = user.id.mention();
                        if *alt {
                            return format!("*{} (alt)*", name);
                        }
                        name.to_string()
                    })
                    .pad_using(1, |_| "None".to_owned())
                    .join(", ");

                embed.field(
                    format!("Group {} ({}/{})", i + 1, group.len(), self.group_size),
                    names,
                    false,
                );
            });

        let alt_names = self
            .extra_alts()
            .map(|user| user.id.mention().to_string())
            .pad_using(1, |_| "None".to_owned())
            .join(", ");
        let maybe_names = self
            .maybe
            .iter()
            .map(|user| user.id.mention().to_string())
            .pad_using(1, |_| "None".to_owned())
            .join(", ");
        embed
            .field("Alternates", alt_names, true)
            .field("Maybe", maybe_names, true);

        embed
    }

    pub fn trigger_alert_protocol(&mut self) -> Vec<EventMember> {
        // We generate and save the alert protocol message when it is triggered, which avoids it
        // changing if people join/leave after it is triggered.
        let groups = self
            .confirmed_groups()
            .into_iter()
            // Filter out any partial groups
            .filter(|group| group.len() == self.group_size as usize);
        let groups_str = groups
            .clone()
            .enumerate()
            .map(|(i, group)| {
                format!(
                    "Group {}: {}",
                    i + 1,
                    group.iter().map(|(user, _)| user.id.mention()).join(", "),
                )
            })
            .join("\n");

        let members = groups
            .flat_map(|group| group.into_iter().map(|(member, _)| member))
            .cloned()
            .collect();

        let message = if groups_str.is_empty() {
            String::new()
        } else {
            format!(
                "Alert Protocol initiated for LFG **{}** ({})\n{}",
                self.id, self.activity, groups_str,
            )
        };
        self.alert_message = Some(message);

        members
    }

    pub fn alerted(&self) -> bool {
        self.alert_message.is_some()
    }

    pub fn alert_protocol_message(&self) -> Option<String> {
        self.alert_message.clone()
    }

    pub fn event_buttons(&self) -> CreateComponents {
        let mut components = CreateComponents::default();
        let mut row = CreateActionRow::default();

        let buttons = [
            ("Join", ButtonStyle::Success),
            ("Leave", ButtonStyle::Danger),
            ("Alt", ButtonStyle::Primary),
            ("Maybe", ButtonStyle::Secondary),
        ];
        buttons.iter().for_each(|(label, style)| {
            let mut button = CreateButton::default();
            let id = format!("{}:{}", label.to_ascii_lowercase(), self.id);
            button.style(*style).label(label).custom_id(id);
            row.add_button(button);
        });

        components.add_action_row(row);
        components
    }
}

#[derive(Debug, Clone)]
pub enum EventChange {
    /// New event added.
    Added(Arc<Event>),
    /// Event deleted.
    Deleted(Arc<Event>),
    /// Event edited by user.
    Edited(Arc<Event>),
    /// Alert protocol initiated.
    Alert(Arc<Event>),
}

// TODO: Use a hardcoded config for now, but this should become per-guild config.
lazy_static! {
    static ref SCHEDULER_CONFIG: alert::EventSchedulerConfig = alert::EventSchedulerConfig {
        alert: Duration::from_secs(10 * 60),
        cleanup: Duration::from_secs(30 * 60),
    };
}

type EventsCollection = BTreeMap<EventId, Arc<Event>>;

const EVENTS_STORE_NAME: &str = "events.json";

#[derive(Debug)]
struct EventManagerState {
    events: EventsCollection,
    events_store: PersistentStore<EventsCollection>,
    next_id: HashMap<Activity, u8>,
    embed_manager: Option<EmbedManager>,
    event_scheduler: alert::EventScheduler,
}

impl EventManagerState {
    pub async fn load(
        ctx: Context,
        store_builder: &PersistentStoreBuilder,
        config: GuildConfig,
    ) -> Result<Self> {
        let events_store = store_builder.build(EVENTS_STORE_NAME).await?;
        let events: EventsCollection = events_store.load().await?;

        let embed_manager = Some(
            EmbedManager::new(ctx, store_builder, config.embed_config, events.values()).await?,
        );
        let event_scheduler = alert::EventScheduler::new(events.values(), *SCHEDULER_CONFIG);

        Ok(EventManagerState {
            events,
            events_store,
            next_id: Default::default(),
            embed_manager,
            event_scheduler,
        })
    }

    #[cfg(test)]
    pub fn default(events_store: PersistentStore<EventsCollection>) -> Self {
        EventManagerState {
            events: Default::default(),
            events_store,
            next_id: Default::default(),
            embed_manager: None,
            event_scheduler: alert::EventScheduler::new(std::iter::empty(), *SCHEDULER_CONFIG),
        }
    }

    pub async fn modify_event<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut EventsCollection) -> Result<(Option<EventChange>, T)>,
    {
        let (change, ret) = f(&mut self.events)?;
        if let Some(change) = change {
            self.events_store.store(&self.events).await?;
            self.event_scheduler.event_changed(&change).await;
            if let Some(mgr) = &mut self.embed_manager {
                mgr.event_changed(change).await?;
            }
        }
        Ok(ret)
    }
}

impl EventManagerState {
    pub fn next_id(&mut self, activity: Activity) -> Result<EventId> {
        // We don't need to find the lowest unused ID or anything fancy, just find the next unused
        // ID and wrap once maxed out. next_id can be inaccurate or uninitialized for a given
        // activity type since we check the known events.
        let events = &self.events;
        let next = self.next_id.entry(activity).or_insert(1);

        let found_next = successors(Some(*next), |n| {
            let succ = n.wrapping_add(1).max(1);

            // Ensure that we don't loop forever in the unlikely case that 256 events of a given
            // Activity exist.
            if succ != *next {
                Some(succ)
            } else {
                None
            }
        })
        .find(|&n| !events.contains_key(&event_id(activity, n)))
        .ok_or_else(|| format_err!("Maximum number of {} events created", activity.name()))?;

        let next_id = event_id(activity, found_next);
        *next = found_next.wrapping_add(1).max(1);
        Ok(next_id)
    }
}

/// Manages a single server's worth of events.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct EventManager<C: CacheHttp = Context> {
    #[derivative(Debug = "ignore")]
    ctx: C,
    store_builder: PersistentStoreBuilder,
    state: RwLock<EventManagerState>,
    removed_from_guild: AtomicBool,
}

impl EventManager {
    pub async fn new(
        ctx: Context,
        store_builder: PersistentStoreBuilder,
        config: GuildConfig,
    ) -> Result<Arc<Self>> {
        let state =
            RwLock::new(EventManagerState::load(ctx.clone(), &store_builder, config).await?);
        let mgr = Arc::new(EventManager {
            ctx,
            store_builder,
            state,
            removed_from_guild: Default::default(),
        });
        // We should be able to acquire the state lock immediately, nothing else could have acquired
        // it yet. We can't Arc::get_mut + RwLock::get_mut because then we wouldn't be able to
        // create the Weak.
        if !*DISABLE_EVENT_SCHEDULER {
            mgr.state
                .try_read()
                .expect("Should be able to acquire state lock")
                .event_scheduler
                .start(Arc::downgrade(&mgr));
        }
        Ok(mgr)
    }

    /// A test-only, async Default-like method.
    #[cfg(test)]
    pub async fn default() -> EventManager<serenity::CacheAndHttp> {
        let tempdir = tempdir::TempDir::new("EventManager").expect("Failed to create tempdir");
        let store_builder = PersistentStoreBuilder::new(tempdir.into_path())
            .await
            .expect("Failed to create PersistentStoreBuilder");
        let events_store = store_builder.build(EVENTS_STORE_NAME).await.unwrap();
        EventManager {
            ctx: Default::default(),
            store_builder,
            state: RwLock::new(EventManagerState::default(events_store)),
            removed_from_guild: Default::default(),
        }
    }
}

impl<C: CacheHttp> EventManager<C> {
    // Bot was removed from the guild for this EventManager, delete state.
    pub fn removed_from_guild(&self) {
        self.removed_from_guild.store(true, Ordering::Relaxed)
    }

    pub async fn create_event(
        &self,
        creator: &dyn MemberLike,
        activity: Activity,
        datetime: DateTime<Tz>,
        description: impl Into<String>,
    ) -> Result<Arc<Event>> {
        let mut state = self.state.write().await;
        let id = state.next_id(activity)?;
        let description = description.into();
        let creator: EventMember = creator.into();
        let event = Arc::new(Event {
            id,
            activity,
            datetime,
            description,
            group_size: activity.default_group_size(),
            recur: false,
            creator: creator.clone(),
            confirmed: vec![creator],
            alternates: vec![],
            maybe: vec![],
            alert_message: None,
        });

        state
            .modify_event(|events| {
                events.insert(id, event.clone());
                Ok((Some(EventChange::Added(event)), ()))
            })
            .await?;

        let event = state.events.get(&id).unwrap().clone();
        Ok(event)
    }

    #[cfg(test)]
    async fn add_test_event(&self, event: Event) -> Result<()> {
        let mut state = self.state.write().await;
        anyhow::ensure!(
            !state.events.contains_key(&event.id),
            "Event already exists with ID {}",
            &event.id
        );

        let key = event.id;
        let event = Arc::new(event);
        state
            .modify_event(|events| {
                events.insert(key, event.clone());
                Ok((Some(EventChange::Added(event)), ()))
            })
            .await
    }

    pub async fn get_event(&self, id: &EventId) -> Option<Arc<Event>> {
        let state = self.state.read().await;
        let events = &state.events;
        events.get(&id).map(|e| e.clone())
    }

    /// Run the provided closure with a mutable reference to the event with the given ID, if one
    /// exists. State is persisted to the store before this returns, and an async task started to
    /// update event embeds.
    pub async fn edit_event<T>(
        &self,
        id: &EventId,
        edit_fn: impl FnOnce(Option<&mut Event>) -> T,
    ) -> Result<T> {
        let mut state = self.state.write().await;

        // Clone the current Event value for this id
        state
            .modify_event(|events| match events.get_mut(&id) {
                Some(event) => {
                    let mut modified = (**event).clone();
                    let ret = edit_fn(Some(&mut modified));
                    *event = Arc::new(modified);

                    Ok((Some(EventChange::Edited(event.clone())), ret))
                }
                None => Ok((None, edit_fn(None))),
            })
            .await
    }

    pub async fn delete_event(&self, id: &EventId) -> Result<()> {
        let mut state = self.state.write().await;
        state
            .modify_event(|events| {
                let event = events
                    .remove(id)
                    .ok_or(format_err!("Event {} does not exist", id))?;
                Ok((Some(EventChange::Deleted(event)), ()))
            })
            .await
    }

    async fn alert_event(&self, id: EventId) -> Result<()> {
        info!("Triggering alert protocol for {}", id);

        let mut state = self.state.write().await;
        let (event, members) = state
            .modify_event(|events| match events.get_mut(&id) {
                Some(mut event) => {
                    let members = Arc::make_mut(&mut event).trigger_alert_protocol();
                    Ok((
                        Some(EventChange::Alert(event.clone())),
                        (event.clone(), members),
                    ))
                }
                None => Err(format_err!("Event {} didn't exist to alert", id)),
            })
            .await?;

        let message = event
            .alert_protocol_message()
            .ok_or(format_err!("Missing alert message??"))?;
        for member in members {
            member
                .id
                .create_dm_channel(&self.ctx)
                .await?
                .send_message(&self.ctx.http(), |msg| msg.content(message.clone()))
                .await?;
        }
        Ok(())
    }

    async fn cleanup_event(&self, id: EventId) -> Result<()> {
        info!("Cleaning up event {}", id);

        let mut state = self.state.write().await;
        let old = state
            .modify_event(|events| {
                let event = events
                    .remove(&id)
                    .ok_or(format_err!("Event {} does not exist", id))?;
                Ok((Some(EventChange::Deleted(event.clone())), event))
            })
            .await?;

        if old.recur {
            info!("Creating event recurrence from {}", id);

            // Check whether we're cleaning up an event that's >1 week old and increase date by
            // multiple weeks as needed. Otherwise we'll end up creating many events, spamming event
            // channels and so forth, to do the same thing.
            let weeks_to_add = Utc::now().signed_duration_since(old.datetime).num_weeks() + 1;

            let id = state.next_id(old.activity)?;
            let new = Arc::new(Event {
                id,
                activity: old.activity,
                datetime: old.datetime + chrono::Duration::weeks(weeks_to_add),
                description: old.description.clone(),
                group_size: old.group_size,
                recur: true,
                creator: old.creator.clone(),
                confirmed: vec![],
                alternates: vec![],
                maybe: vec![],
                alert_message: None,
            });
            state
                .modify_event(|events| {
                    events.insert(id, new.clone());
                    Ok((Some(EventChange::Added(new)), ()))
                })
                .await?;
        }
        Ok(())
    }

    /// Adds a new message that contains this event's embed and which should be kept up to date as
    /// the event is modified.
    pub async fn keep_embed_updated(
        &self,
        event_id: EventId,
        msg: EventEmbedMessage,
    ) -> Result<()> {
        let state = self.state.read().await;
        state
            .embed_manager
            .as_ref()
            .expect("EmbedManager None, bad test")
            .keep_embed_updated(event_id, msg)
            .await
    }

    #[cfg(test)]
    pub async fn next_id(&self, activity: Activity) -> Result<EventId> {
        let mut state = self.state.write().await;
        state.next_id(activity)
    }
}

#[async_trait]
impl alert::ScheduledActionHandler for EventManager {
    async fn with_event_for_id<F, T>(&self, id: EventId, func: F) -> Option<T>
    where
        F: FnOnce(&Event) -> T + Send,
    {
        let state = self.state.read().await;
        state.events.get(&id).map(|event| func(event))
    }

    async fn perform_action(&self, action: &alert::ScheduledAction) -> Result<()> {
        match action.action {
            alert::EventAction::Alert => self.alert_event(action.id).await,
            alert::EventAction::Cleanup => self.cleanup_event(action.id).await,
        }
    }
}

impl<C: CacheHttp> Drop for EventManager<C> {
    fn drop(&mut self) {
        if self.removed_from_guild.load(Ordering::Relaxed) {
            let store_builder = self.store_builder.clone();
            tokio::spawn(async move {
                if let Err(err) = store_builder.delete().await {
                    error!("Failed to delete guild data after removal: {:?}", err);
                }
            });
        }
    }
}

impl TypeMapKey for EventManager {
    type Value = Arc<EventManager>;
}

#[cfg(test)]
mod tests {
    use super::{Event, *};
    use std::iter;
    use test_env_log::test;

    async fn add_events_to_manager<C: CacheHttp>(
        manager: &EventManager<C>,
        activity: Activity,
        indexes: impl IntoIterator<Item = u8>,
    ) {
        for index in indexes {
            manager
                .add_test_event(Event {
                    id: event_id(activity, index),
                    activity,
                    ..Default::default()
                })
                .await
                .expect("Error while adding test events");
        }
    }

    // Helper for tests since Member doesn't implement Default
    impl MemberLike for User {
        fn user(&self) -> &User {
            &self
        }

        fn id(&self) -> UserId {
            self.id
        }

        fn display_name(&self) -> &str {
            &self.name
        }
    }

    const VOG: Activity = Activity::VaultOfGlass;
    const GOS: Activity = Activity::GardenOfSalvation;

    #[test]
    fn test_event_id_display() {
        assert_eq!(event_id(VOG, 42).to_string(), "vog42");
        assert_eq!(event_id(GOS, 128).to_string(), "gos128");
    }

    #[test(tokio::test)]
    async fn test_next_id_advances() {
        let manager = EventManager::default().await;
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 1));
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 2));
        // Other activities are unaffected.
        assert_eq!(manager.next_id(GOS).await.unwrap(), event_id(GOS, 1));
        assert_eq!(manager.next_id(GOS).await.unwrap(), event_id(GOS, 2));
    }

    #[test(tokio::test)]
    async fn test_next_id_gaps() {
        let manager = EventManager::default().await;
        add_events_to_manager(&manager, VOG, (1u8..=20).chain(23u8..=50)).await;
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 21));
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 22));
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 51));
    }

    #[test(tokio::test)]
    async fn test_next_id_wraps() {
        let manager = EventManager::default().await;
        add_events_to_manager(&manager, VOG, (1u8..=41).chain(44u8..=255)).await;
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 42));
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 43));
        // Will wrap around and find the still unused indexes.
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 42));
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 43));
        add_events_to_manager(&manager, VOG, iter::once(42)).await;
        assert_eq!(manager.next_id(VOG).await.unwrap(), event_id(VOG, 43));
    }

    #[test(tokio::test)]
    async fn test_next_exhausted() {
        let manager = EventManager::default().await;
        add_events_to_manager(&manager, VOG, 1u8..=255).await;
        assert!(manager.next_id(VOG).await.is_err());
        // Other activities are unaffected.
        assert_eq!(manager.next_id(GOS).await.unwrap(), event_id(GOS, 1));
    }

    #[test(tokio::test)]
    async fn test_create_event() {
        let manager = EventManager::default().await;
        let t = Utc::now().with_timezone(&Tz::PST8PDT);
        let user = User::default();
        assert_eq!(
            manager.create_event(&user, VOG, t, "").await.unwrap().id,
            event_id(VOG, 1)
        );
        assert_eq!(
            manager.create_event(&user, VOG, t, "").await.unwrap().id,
            event_id(VOG, 2)
        );
        assert_eq!(
            manager.create_event(&user, GOS, t, "").await.unwrap().id,
            event_id(GOS, 1)
        );
    }
}
