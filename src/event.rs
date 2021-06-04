use crate::activity::Activity;
use crate::time::serialize_datetime_tz;
use anyhow::{ensure, format_err, Result};
use chrono::DateTime;
use chrono::Utc;
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
    iter::successors,
};

/// Unique identifier for an Event.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Serialize, Deserialize)]
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

// TODO: We currently persist the last seen username. Need to make sure these stay up to date as we
// get new user info.
#[derive(Serialize, Deserialize)]
pub struct EventUser {
    pub id: UserId,
    pub name: String,
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
    fn confirmed_groups(&self) -> impl Iterator<Item = &[EventUser]> {
        self.confirmed
            .chunks(self.group_size as usize)
            .pad_using(1, |_| &[])
    }

    // TODO: The event needs to keep track of all the messages that exist with an event embed, so it
    // can update them as the event is modified.
    pub fn as_embed(&self) -> CreateEmbed {
        let start_time = self.datetime.format("%I:%M %p %Z %m/%d");
        let mut embed = CreateEmbed::default();
        embed
            .field("Activity", self.activity, true)
            .field("Start Time", start_time, true)
            .field("Join ID", self.id, true)
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
#[derive(Default)]
pub struct EventManager {
    events: BTreeMap<EventId, Event>,
    next_id: HashMap<Activity, u8>,
}

impl EventManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_event(
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
            creator: EventUser {
                id: creator.id,
                name: creator.name.clone(),
            },
            confirmed: vec![],
            alternates: vec![],
        })
    }

    fn add_event(&mut self, event: Event) -> Result<&Event> {
        ensure!(
            !self.events.contains_key(&event.id),
            "Event already exists with ID {}",
            &event.id
        );
        let key = event.id;
        self.events.insert(key, event);
        Ok(self.events.get(&key).unwrap())
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

    fn add_events_to_manager(
        manager: &mut EventManager,
        activity: Activity,
        indexes: impl IntoIterator<Item = u8>,
    ) {
        let user = User::default();
        indexes
            .into_iter()
            .try_for_each(|idx| {
                manager
                    .add_event(Event {
                        id: event_id(activity, idx),
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
                    .and(Ok(()))
            })
            .expect("Error while adding test events")
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
        let mut manager = EventManager::new();
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 1));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 2));
        // Other activities are unaffected.
        assert_eq!(manager.next_id(GOS).unwrap(), event_id(GOS, 1));
        assert_eq!(manager.next_id(GOS).unwrap(), event_id(GOS, 2));
    }

    #[test]
    fn test_next_id_gaps() {
        let mut manager = EventManager::new();
        add_events_to_manager(&mut manager, VOG, (1u8..=20).chain(23u8..=50));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 21));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 22));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 51));
    }

    #[test]
    fn test_next_id_wraps() {
        let mut manager = EventManager::new();
        add_events_to_manager(&mut manager, VOG, (1u8..=41).chain(44u8..=255));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 42));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 43));
        // Will wrap around and find the still unused indexes.
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 42));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 43));
        add_events_to_manager(&mut manager, VOG, iter::once(42));
        assert_eq!(manager.next_id(VOG).unwrap(), event_id(VOG, 43));
    }

    #[test]
    fn test_next_exhausted() {
        let mut manager = EventManager::new();
        add_events_to_manager(&mut manager, VOG, 1u8..=255);
        assert!(manager.next_id(VOG).is_err());
        // Other activities are unaffected.
        assert_eq!(manager.next_id(GOS).unwrap(), event_id(GOS, 1));
    }

    #[test]
    fn test_create_event() {
        let mut manager = EventManager::new();
        let t = Utc::now().with_timezone(&Tz::PST8PDT);
        let user = User::default();
        assert_eq!(
            manager.create_event(&user, VOG, t, "").unwrap().id,
            event_id(VOG, 1)
        );
        assert_eq!(
            manager.create_event(&user, VOG, t, "").unwrap().id,
            event_id(VOG, 2)
        );
        assert_eq!(
            manager.create_event(&user, GOS, t, "").unwrap().id,
            event_id(GOS, 1)
        );
    }
}
