use crate::{activity::Activity, time::serialize_datetime_tz, util::*};
use anyhow::{ensure, format_err, Context as _, Error, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serenity::{
    builder::CreateEmbed,
    model::{id::UserId, prelude::*},
    prelude::TypeMapKey,
};
use std::{
    collections::{BTreeMap, HashMap},
    convert::TryFrom,
    iter::successors,
    path::PathBuf,
    str::FromStr,
};
use tokio::{fs, io::AsyncWriteExt};

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
#[derive(Serialize, Deserialize)]
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

/// A single scheduled event.
#[derive(Serialize, Deserialize)]
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
}

impl Event {
    pub fn join(&mut self, user: &User) {
        self.confirmed.push(user.into())
    }

    pub fn formatted_datetime(&self) -> String {
        self.datetime.format("%-I:%M %p %Z %-m/%-d").to_string()
    }

    fn confirmed_groups(&self) -> impl Iterator<Item = &[EventUser]> {
        self.confirmed
            .chunks(self.group_size as usize)
            .pad_using(1, |_| &[])
    }

    // TODO: The event needs to keep track of all the messages that exist with an event embed, so it
    // can update them as the event is modified.
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
}

/// Manages a single server's worth of events.
#[cfg_attr(test, derive(Default))]
pub struct EventManager {
    store_path: Option<PathBuf>,
    events: BTreeMap<EventId, Event>,
    next_id: HashMap<Activity, u8>,
}

impl EventManager {
    pub async fn new(store_path: impl Into<PathBuf>) -> Result<Self> {
        let store_path = store_path.into();
        let events = match fs::read(&store_path).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::default()),
            Ok(store_bytes) => serde_json::from_slice(&store_bytes)
                .context("Failed to deserialize EventManger's event data"),
            Err(err) => Err(Error::from(err).context("Failed to read event store")),
        }?;

        Ok(EventManager {
            store_path: Some(store_path),
            events,
            next_id: Default::default(),
        })
    }

    pub async fn create_event(
        &mut self,
        creator: &User,
        activity: Activity,
        datetime: DateTime<Tz>,
        description: impl Into<String>,
    ) -> Result<&Event> {
        let id = self.next_id(activity)?;
        let description = description.into();
        self.add_event(Event {
            id,
            activity,
            datetime,
            description,
            group_size: activity.default_group_size(),
            creator: creator.into(),
            confirmed: vec![],
            alternates: vec![],
        })
        .await
    }

    async fn add_event(&mut self, event: Event) -> Result<&Event> {
        ensure!(
            !self.events.contains_key(&event.id),
            "Event already exists with ID {}",
            &event.id
        );
        let key = event.id;
        self.events.insert(key, event);
        self.update_store().await?;
        Ok(self.events.get(&key).unwrap())
    }

    pub fn get_event(&self, id: &EventId) -> Option<&Event> {
        self.events.get(&id)
    }

    pub async fn edit_event<T>(
        &mut self,
        id: &EventId,
        edit_fn: impl FnOnce(Option<&mut Event>) -> T,
    ) -> Result<T> {
        let ret = edit_fn(self.events.get_mut(&id));
        self.update_store().await?;
        Ok(ret)
    }

    async fn update_store(&mut self) -> Result<()> {
        let store_path = match &self.store_path {
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

        fs::rename(temppath, &store_path)
            .await
            .context("Failed to atomically replace event store")
    }

    fn next_id(&mut self, activity: Activity) -> Result<EventId> {
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
