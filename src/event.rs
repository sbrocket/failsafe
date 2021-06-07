use crate::{activity::Activity, time::serialize_datetime_tz, util::*};
use anyhow::{format_err, Context as _, Error, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serenity::{
    builder::CreateEmbed,
    client::Context,
    model::{id::UserId, prelude::*},
    prelude::TypeMapKey,
};
use std::{
    collections::{BTreeMap, HashMap},
    convert::TryFrom,
    iter::successors,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};
use tokio::{fs, io::AsyncWriteExt, sync::RwLock};
use tracing::{error, info, warn};

/// Unique identifier for an Event.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Serialize, Deserialize)]
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
#[derive(Debug, Serialize, Deserialize)]
pub struct EventUser {
    pub id: UserId,
    pub name: String,
}

impl From<&User> for EventUser {
    fn from(user: &User) -> Self {
        EventUser {
            id: user.id,
            name: user.name.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum EventEmbedMessage {
    Normal(ChannelId, MessageId),
    // TODO: The interaction token expires after 15 minutes. Don't try to update after the token has
    // expired, and consider deleting the response before then as well.
    EphemeralResponse(Interaction),
}

impl EventEmbedMessage {
    fn strip_unneeded_fields(&mut self) {
        match self {
            EventEmbedMessage::EphemeralResponse(interaction) => {
                interaction.data = None;
                interaction.guild_id = None;
                interaction.channel_id = None;
                interaction.member = None;
                interaction.user = None;
            }
            _ => {}
        }
    }
}

impl PartialEq for EventEmbedMessage {
    fn eq(&self, other: &Self) -> bool {
        use EventEmbedMessage::*;
        match (self, other) {
            (Normal(a1, a2), Normal(b1, b2)) => a1 == b1 && a2 == b2,
            (EphemeralResponse(a), EphemeralResponse(b)) => a.id == b.id,
            _ => false,
        }
    }
}

impl Eq for EventEmbedMessage {}

/// A single scheduled event.
#[derive(Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub activity: Activity,
    #[serde(with = "serialize_datetime_tz")]
    pub datetime: DateTime<Tz>,
    pub description: String,
    pub group_size: u8,
    pub creator: EventUser,
    pub confirmed: Vec<EventUser>,
    // TODO: Need to make use of alternates. Distinguish between confirmed alts and unsure? Fill out
    // partial groups with alts, distinguish them with italics and "(alt)"?
    pub alternates: Vec<EventUser>,

    // Messages that this event's embed has been added to, and which need to be updated when the
    // event is updated.
    // TODO: This data probably needs to move out of the Event so that the mappings of messages to
    // events can be more easily modified for posting events to an event channel.
    #[serde(with = "serialize_arc_rwlock")]
    embed_messages: Arc<RwLock<Vec<EventEmbedMessage>>>,
}

// This is a debugging feature, to allow testing the bot with a small number of users.
lazy_static! {
    static ref ALLOW_DUPLICATE_JOIN: bool =
        std::env::var("ALLOW_DUPLICATE_JOIN").map_or(false, |v| v == "1");
}

impl Event {
    pub fn join(&mut self, user: &User) -> Result<()> {
        if !*ALLOW_DUPLICATE_JOIN && self.confirmed.iter().any(|u| u.id == user.id) {
            return Err(format_err!("User already in event"));
        }
        self.confirmed.push(user.into());
        Ok(())
    }

    pub fn formatted_datetime(&self) -> String {
        self.datetime.format("%-I:%M %p %Z %-m/%-d").to_string()
    }

    fn confirmed_groups(&self) -> impl Iterator<Item = &[EventUser]> {
        self.confirmed
            .chunks(self.group_size as usize)
            .pad_using(1, |_| &[])
    }

    pub fn as_embed(&self) -> CreateEmbed {
        let mut embed = CreateEmbed::default();
        embed
            .field("Activity", self.activity, true)
            .field("Start Time", self.formatted_datetime(), true)
            .field("Event ID", self.id, true)
            .field("Description", self.description.clone(), false)
            .footer(|f| f.text(format!("Creator | {} | Your Time", self.creator.name)))
            .timestamp(&self.datetime.with_timezone(&Utc));

        self.confirmed_groups().enumerate().for_each(|(i, group)| {
            let names: String = group
                .iter()
                .map(|g| g.name.as_str())
                .pad_using(1, |_| "None")
                .join(", ");
            embed.field(
                format!("Group {} ({}/{})", i + 1, group.len(), self.group_size),
                names,
                true,
            );
        });
        embed
    }

    /// Adds a new message that contains this event's embed and which should be kept up to date as
    /// the event is modified.
    async fn keep_embed_updated(&self, mut message: EventEmbedMessage) {
        let mut msgs = self.embed_messages.write().await;
        if msgs.contains(&message) {
            warn!("Event {} already tracking message {:?}", self.id, message);
            return;
        }
        message.strip_unneeded_fields();
        msgs.push(message);

        // TODO: Cleanup any expired EphemeralResponse entries while we're holding the write lock?
    }

    /// Asychronously (in a spawned task) update the embeds in tracked messages.
    fn start_updating_embeds(&self, ctx: Context) {
        let embed = self.as_embed();
        let messages = self.embed_messages.clone();
        let update_fut = async move {
            let messages = messages.read().await;
            future::join_all(messages.iter().map(|msg| {
                let ctx = &ctx;
                let embed = &embed;
                async move {
                    match msg {
                        EventEmbedMessage::Normal(chan_id, msg_id) => {
                            chan_id
                                .edit_message(ctx, msg_id, |edit| {
                                    edit.embed(|e| {
                                        *e = embed.clone();
                                        e
                                    })
                                })
                                .await
                        }
                        EventEmbedMessage::EphemeralResponse(interaction) => {
                            interaction
                                .edit_original_interaction_response(ctx, |resp| {
                                    resp.set_embeds(vec![embed.clone()])
                                })
                                .await
                        }
                    }
                    .context("Failed to edit message")
                }
            }))
            .await
        };

        let event_id = self.id;
        tokio::spawn(async move {
            let results = update_fut.await;
            let (successes, failures): (Vec<_>, Vec<_>) =
                results.into_iter().partition(Result::is_ok);
            let count = successes.len() + failures.len();
            if failures.is_empty() {
                info!("Successfully updated embeds for event {}", event_id);
            } else if successes.is_empty() {
                error!(
                    "All ({}) embeds failed to update for event {}",
                    count, event_id
                );
                failures.into_iter().for_each(|f| error!("{:?}", f));
            } else {
                error!(
                    "Some ({}/{}) embeds failed to update for event {}",
                    failures.len(),
                    count,
                    event_id
                );
                failures.into_iter().for_each(|f| error!("{:?}", f));
            }
        });
    }
}

#[derive(Default)]
struct EventStore {
    path: Option<PathBuf>,
    events: BTreeMap<EventId, Event>,
}

impl EventStore {
    pub async fn new(store_path: impl Into<PathBuf>) -> Result<Self> {
        let path = store_path.into();
        let events = match fs::read(&path).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::default()),
            Ok(store_bytes) => serde_json::from_slice(&store_bytes)
                .context("Failed to deserialize EventManger's event data"),
            Err(err) => Err(Error::from(err).context("Failed to read event store")),
        }?;

        Ok(EventStore {
            path: Some(path),
            events,
        })
    }

    pub async fn save(&self) -> Result<()> {
        let path = match &self.path {
            Some(path) => path,
            None => return Ok(()),
        };

        let json = serde_json::to_vec(&self.events).context("Failed to serialize events")?;

        let (temppath, mut tempfile) = tempfile().await.context("Unable to create tempfile")?;
        tempfile
            .write_all(&json)
            .await
            .context("Failed writing event store tempfile")?;
        tempfile.flush().await?;
        std::mem::drop(tempfile);

        fs::rename(temppath, &path)
            .await
            .context("Failed to atomically replace event store")
    }
}

pub struct EventHandle<'a> {
    store: &'a EventStore,
    event: &'a Event,
}

impl EventHandle<'_> {
    pub async fn keep_embed_updated(&self, msg: EventEmbedMessage) -> Result<()> {
        self.event.keep_embed_updated(msg).await;
        self.store.save().await
    }
}

impl std::ops::Deref for EventHandle<'_> {
    type Target = Event;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}

/// Manages a single server's worth of events.
#[cfg_attr(test, derive(Default))]
pub struct EventManager {
    store: EventStore,
    next_id: HashMap<Activity, u8>,
}

impl EventManager {
    pub async fn new(store_path: impl Into<PathBuf>) -> Result<Self> {
        Ok(EventManager {
            store: EventStore::new(store_path).await?,
            next_id: Default::default(),
        })
    }

    pub async fn create_event(
        &mut self,
        creator: &User,
        activity: Activity,
        datetime: DateTime<Tz>,
        description: impl Into<String>,
    ) -> Result<EventHandle<'_>> {
        let id = self.next_id(activity)?;
        let description = description.into();
        self.store.events.insert(
            id,
            Event {
                id,
                activity,
                datetime,
                description,
                group_size: activity.default_group_size(),
                creator: creator.into(),
                confirmed: vec![],
                alternates: vec![],
                embed_messages: Default::default(),
            },
        );
        self.store.save().await?;

        let event = self.store.events.get(&id).unwrap();
        Ok(EventHandle {
            store: &self.store,
            event,
        })
    }

    #[cfg(test)]
    async fn add_event(&mut self, event: Event) -> Result<&Event> {
        anyhow::ensure!(
            !self.store.events.contains_key(&event.id),
            "Event already exists with ID {}",
            &event.id
        );
        let key = event.id;
        self.store.events.insert(key, event);
        self.store.save().await?;
        Ok(self.store.events.get(&key).unwrap())
    }

    pub fn get_event(&self, id: &EventId) -> Option<EventHandle<'_>> {
        let store = &self.store;
        store
            .events
            .get(&id)
            .map(|event| EventHandle { store, event })
    }

    /// Run the provided closure with a mutable reference to the event with the given ID, if one
    /// exists. State is persisted to the store before this returns, and an async task started to
    /// update event embeds.
    pub async fn edit_event<T>(
        &mut self,
        ctx: &Context,
        id: &EventId,
        edit_fn: impl FnOnce(Option<&mut Event>) -> T,
    ) -> Result<T> {
        // This needs two lookups because edit_fn needs `&mut Event`, which would require passing it
        // `&mut Option<&mut Event>`, but then it could also modify the Option out from under us
        // (e.g. with `take()`).
        let ret = edit_fn(self.store.events.get_mut(&id));
        if let Some(event) = self.store.events.get(&id) {
            event.start_updating_embeds(ctx.clone());
            self.store.save().await?;
        }
        Ok(ret)
    }

    fn next_id(&mut self, activity: Activity) -> Result<EventId> {
        // We don't need to find the lowest unused ID or anything fancy, just find the next unused
        // ID and wrap once maxed out. next_id can be inaccurate or uninitialized for a given
        // activity type since we check the known events.
        let events = &self.store.events;
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

impl TypeMapKey for EventManager {
    type Value = EventManager;
}

#[cfg(test)]
mod tests {
    // No clue why * doesn't pull in 'Event'
    use super::{Event, *};
    use std::iter;

    async fn add_events_to_manager(
        manager: &mut EventManager,
        activity: Activity,
        indexes: impl IntoIterator<Item = u8>,
    ) {
        let user = User::default();
        for index in indexes {
            manager
                .add_event(Event {
                    id: event_id(activity, index),
                    activity,
                    datetime: Utc::now().with_timezone(&Tz::PST8PDT),
                    description: "".to_string(),
                    group_size: 1,
                    creator: EventUser {
                        id: user.id,
                        name: user.name.clone(),
                    },
                    confirmed: vec![],
                    alternates: vec![],
                    embed_messages: Default::default(),
                })
                .await
                .expect("Error while adding test events");
        }
    }

    const VOG: Activity = Activity::VaultOfGlass;
    const GOS: Activity = Activity::GardenOfSalvation;

    #[test]
    fn test_event_id_display() {
        assert_eq!(event_id(VOG, 42).to_string(), "vog42");
        assert_eq!(event_id(GOS, 128).to_string(), "gos128");
    }

    #[test]
    fn test_next_id_advances() {
        let mut manager = EventManager::default();
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 1));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 2));
        // Other activities are unaffected.
        assert_eq!(manager.next_id(GOS).unwrap(), event_id(GOS, 1));
        assert_eq!(manager.next_id(GOS).unwrap(), event_id(GOS, 2));
    }

    #[tokio::test]
    async fn test_next_id_gaps() {
        let mut manager = EventManager::default();
        add_events_to_manager(&mut manager, VOG, (1u8..=20).chain(23u8..=50)).await;
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 21));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 22));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 51));
    }

    #[tokio::test]
    async fn test_next_id_wraps() {
        let mut manager = EventManager::default();
        add_events_to_manager(&mut manager, VOG, (1u8..=41).chain(44u8..=255)).await;
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 42));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 43));
        // Will wrap around and find the still unused indexes.
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 42));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 43));
        add_events_to_manager(&mut manager, VOG, iter::once(42)).await;
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 43));
    }

    #[tokio::test]
    async fn test_next_exhausted() {
        let mut manager = EventManager::default();
        add_events_to_manager(&mut manager, VOG, 1u8..=255).await;
        assert!(manager.next_id(VOG).is_err());
        // Other activities are unaffected.
        assert_eq!(manager.next_id(GOS).unwrap(), event_id(GOS, 1));
    }

    #[tokio::test]
    async fn test_create_event() {
        let mut manager = EventManager::default();
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
