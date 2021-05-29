use crate::activity::Activity;
use anyhow::{ensure, format_err, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serenity::model::id::UserId;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    iter::successors,
};

/// Unique identifier for an Event.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Serialize, Deserialize)]
struct EventId {
    pub activity: Activity,
    pub idx: u8,
}

fn event_id(activity: Activity, idx: u8) -> EventId {
    EventId { activity, idx }
}

impl Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}{}", self.activity.id_prefix(), self.idx))
    }
}

/// A single scheduled event.
#[derive(Serialize, Deserialize)]
struct Event {
    activity: Activity,
    datetime: DateTime<Utc>,
    description: String,
    event_id: EventId,
    confirmed: Vec<UserId>,
    alternates: Vec<UserId>,
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

    fn add_event(&mut self, event: Event) -> Result<()> {
        ensure!(
            !self.events.contains_key(&event.event_id),
            "Event already exists with ID {}",
            &event.event_id
        );
        self.events.insert(event.event_id, event);
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;

    fn add_events_to_manager(
        manager: &mut EventManager,
        activity: Activity,
        indexes: impl IntoIterator<Item = u8>,
    ) {
        indexes
            .into_iter()
            .try_for_each(|idx| {
                manager.add_event(Event {
                    activity,
                    datetime: Utc::now(),
                    description: "".to_string(),
                    event_id: event_id(activity, idx),
                    confirmed: vec![],
                    alternates: vec![],
                })
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
}
