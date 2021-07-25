use super::{Event, EventChange, EventId};
use anyhow::Result;
use chrono::{DateTime, Duration as SignedDuration, Utc};
use chrono_tz::Tz;
use futures::future::{abortable, AbortHandle};
use serenity::async_trait;
use std::{
    collections::BTreeSet,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{sync::Mutex, time::sleep};
use tracing::{error, info, warn};

/// Trait used to perform scheduled actions. Primarily this is implemented by EventManager, but this
/// allows for a simpler fake for unit testing.
#[async_trait]
pub trait ScheduledActionHandler: Send + Sync + 'static {
    /// Call func with the most current Event data for the given EventId. Used to verify that
    /// actions are not stale before performing them.
    async fn with_event_for_id<F, T>(&self, id: EventId, func: F) -> Option<T>
    where
        F: FnOnce(&Event) -> T + Send;

    /// Perform the given action.
    async fn perform_action(&self, action: &ScheduledAction) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduledAction {
    // Order by action time first, then id (which is unique).
    action_datetime: DateTime<Tz>,
    pub id: EventId,
    pub action: EventAction,

    // This lets us verify that this action .
    event_datetime: DateTime<Tz>,
}

impl ScheduledAction {
    pub fn new(event: &Event, delta: SignedDuration, action: EventAction) -> Self {
        ScheduledAction {
            action_datetime: event.datetime + delta,
            id: event.id,
            action,
            event_datetime: event.datetime,
        }
    }

    pub fn expired<T: chrono::TimeZone>(&self, now: &DateTime<T>) -> bool {
        &self.action_datetime <= now
    }
}

impl std::fmt::Display for ScheduledAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{} {} @ {}",
            self.action, self.id, self.action_datetime
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventAction {
    /// Alert event participants that the event is about to start.
    Alert,

    /// Clean up a past event, deleting it and (if needed) creating the next event for recurring
    /// events.
    Cleanup,
}

impl std::fmt::Display for EventAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventAction::Alert => f.write_str("Alert"),
            EventAction::Cleanup => f.write_str("Cleanup"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EventSchedulerConfig {
    // Duration before an event's scheduled time to trigger Alert Protocol.
    pub alert: Duration,
    // Duration after an event's scheduled time to clean up the event.
    pub cleanup: Duration,
}

impl EventSchedulerConfig {
    fn actions_for_event(&self, event: &Event) -> impl Iterator<Item = ScheduledAction> {
        IntoIterator::into_iter([
            ScheduledAction::new(
                event,
                -SignedDuration::from_std(self.alert).unwrap(),
                EventAction::Alert,
            ),
            ScheduledAction::new(
                event,
                SignedDuration::from_std(self.cleanup).unwrap(),
                EventAction::Cleanup,
            ),
        ])
    }
}

// Used to control apparent time for unit testing.
pub trait TimeSource: Send + Sync + 'static {
    fn utc_now(&self) -> DateTime<Utc>;
}

#[derive(Debug)]
pub struct RealTimeSource;

impl TimeSource for RealTimeSource {
    #[inline]
    fn utc_now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug)]
pub struct EventScheduler<T: TimeSource = RealTimeSource> {
    state: Arc<Mutex<EventSchedulerState<T>>>,
    config: EventSchedulerConfig,
}

impl EventScheduler {
    pub fn new<'a, I>(initial_events: I, config: EventSchedulerConfig) -> Self
    where
        I: Iterator<Item = &'a Arc<Event>>,
    {
        Self::new_with_time_source(initial_events, config, RealTimeSource)
    }
}

impl<T: TimeSource> EventScheduler<T> {
    fn new_with_time_source<'a, I>(
        initial_events: I,
        config: EventSchedulerConfig,
        time_source: T,
    ) -> EventScheduler<T>
    where
        I: Iterator<Item = &'a Arc<Event>>,
    {
        let now = time_source.utc_now();
        let actions = initial_events
            .flat_map(|e| config.actions_for_event(e))
            .filter(|a| !a.expired(&now))
            .collect();
        EventScheduler {
            state: Arc::new(Mutex::new(EventSchedulerState {
                actions,
                sleep_handle: None,
                time_source,
            })),
            config,
        }
    }

    pub async fn event_changed(&self, change: &EventChange) {
        // Remove any old actions for this event ID if needed, then add the new actions.
        // Technically we could leave the old actions in place on edit since the BTreeSet will
        // either dedup for us (if the timestamp hasn't changed) or we'll detect that they're stale
        // later, but this avoids stale actions sitting around and using memory for no reason.
        let mut state = self.state.lock().await;
        match change {
            EventChange::Edited(event) | EventChange::Deleted(event) => {
                state.actions.retain(|action| action.id != event.id);
            }
            EventChange::Added(_) => {}
        }
        match change {
            EventChange::Added(event) | EventChange::Edited(event) => {
                let now = state.time_source.utc_now();
                state.actions.extend(
                    self.config
                        .actions_for_event(event)
                        .filter(|a| !a.expired(&now)),
                )
            }
            EventChange::Deleted(_) => {}
        }

        // Kick the scheduler loop so it can check for new actions.
        state.sleep_handle.take().map(|a| a.abort());
    }

    pub fn start<H: ScheduledActionHandler>(&self, handler: Weak<H>) {
        let state = self.state.clone();
        tokio::spawn(async move {
            loop {
                // This scope ensures the MutexGuard is dropped before sleeping.
                // We don't care about the reason for the sleep future finishing (duration reached
                // vs abort) so the return value is ignored.
                let _ = {
                    let mut state = state.lock().await;
                    let sleep_duration = match handler.upgrade() {
                        Some(handler) => state.perform_actions(handler).await,
                        None => {
                            warn!("EventManager gone, stopping EventScheduler loop");
                            return;
                        }
                    };

                    let (sleep, sleep_handle) = abortable(sleep(sleep_duration));
                    state.sleep_handle = Some(sleep_handle);
                    sleep
                }
                .await;
            }
        });
    }
}

#[derive(Debug)]
struct EventSchedulerState<T: TimeSource> {
    // BinaryHeap would be a natural choice here, but BTreeSet ensures that we don't end up with
    // lots of duplicate actions.
    actions: BTreeSet<ScheduledAction>,
    sleep_handle: Option<AbortHandle>,
    time_source: T,
}

impl<T: TimeSource> EventSchedulerState<T> {
    /// Performs any actions whose time has been reached and then returns the time until the next
    /// scheduled action or StdDuration::MAX if there is no next action yet.
    pub async fn perform_actions<H: ScheduledActionHandler>(
        &mut self,
        handler: Arc<H>,
    ) -> Duration {
        let now = self.time_source.utc_now();
        while let Some(next) = self.actions.pop_if(|act| act.expired(&now)) {
            // Check that the action isn't stale before performing it.
            // We remove old actions when events are edited or deleted so this is unlikely to
            // actually skip anything, but it is technically possible if an edit/delete happens
            // while the outer loop is holding our own state lock.
            let stale = handler
                .with_event_for_id(next.id, |e| e.datetime != next.event_datetime)
                .await
                .unwrap_or(true);
            if stale {
                info!("Skipped stale action: {}", next);
                continue;
            }
            if let Err(err) = handler.perform_action(&next).await {
                error!("Error performing scheduled action ({}): {:?}", next, err);
            }
        }

        match self.actions.peek() {
            Some(next) => next
                .action_datetime
                .signed_duration_since(now)
                .to_std()
                .expect("Should have already popped all expired actions"),
            None => Duration::MAX,
        }
    }
}

// BTreeSet::first/pop_first aren't yet stabilized, so add peek/pop for usage above.
trait BTreeSetExt<T> {
    fn peek(&self) -> Option<&T>;
    fn pop_if<P>(&mut self, pred: P) -> Option<T>
    where
        P: Fn(&T) -> bool;
}

impl<T: Ord + Clone> BTreeSetExt<T> for BTreeSet<T> {
    fn peek(&self) -> Option<&T> {
        self.iter().next()
    }

    fn pop_if<P>(&mut self, pred: P) -> Option<T>
    where
        P: Fn(&T) -> bool,
    {
        let first = self.peek();
        if let Some(first) = first {
            if pred(first) {
                let first = first.clone();
                self.remove(&first);
                return Some(first);
            }
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::activity::Activity;
    use parking_lot::Mutex as SyncMutex;
    use std::collections::HashMap;
    use tokio::time::Instant;

    // Fixture for testing EventScheduler. Creates and wraps EventScheduler, implements
    // ScheduledActionHandler, and keeps track of events that exist for the test as well as actions that
    // occur.
    struct EventSchedulerTest {
        time_source: TestTimeSource,
        scheduler: EventScheduler<TestTimeSource>,
        events: SyncMutex<HashMap<EventId, Arc<Event>>>,
        last_actions: SyncMutex<Option<Vec<ScheduledAction>>>,
    }

    impl EventSchedulerTest {
        pub fn start<'a, I>(
            initial_events: I,
            config: EventSchedulerConfig,
            time_source: TestTimeSource,
        ) -> Arc<Self>
        where
            I: Iterator<Item = &'a Arc<Event>> + Clone,
        {
            // For these simple tests events are only unique by index; check that this is true.
            let mut idxs: Vec<u8> = initial_events.clone().map(|e| e.id.idx).collect();
            idxs.sort_unstable();
            assert!(
                idxs.windows(2).all(|w| w[0] != w[1]),
                "Test events must have unique indexes"
            );

            let scheduler = EventScheduler::new_with_time_source(
                initial_events.clone(),
                config,
                time_source.clone(),
            );
            let test = Arc::new(EventSchedulerTest {
                time_source,
                scheduler,
                events: SyncMutex::new(
                    initial_events
                        .cloned()
                        .map(|event| (event.id, event))
                        .collect(),
                ),
                last_actions: SyncMutex::new(None),
            });
            test.scheduler.start(Arc::downgrade(&test));
            test
        }

        // Takes the last action that occurred, resetting the internal action buffer. Panics if more
        // than one action has occurred since last reset.
        pub fn take_last_action(&self) -> Option<ScheduledAction> {
            let actions = self.last_actions.lock().take();
            match actions {
                Some(mut actions) => {
                    assert!(actions.len() == 1);
                    Some(actions.remove(0))
                }
                None => None,
            }
        }

        // Takes last actions that occurred, resetting the internal action buffer.
        pub fn take_last_actions(&self) -> Option<Vec<ScheduledAction>> {
            self.last_actions.lock().take()
        }

        pub async fn add_event(&self, event: Arc<Event>) {
            let mut events = self.events.lock();
            assert!(
                events.keys().find(|id| id.idx == event.id.idx).is_none(),
                "Test events must have unique indexes"
            );

            events.insert(event.id, event.clone());
            self.scheduler
                .event_changed(&EventChange::Added(event))
                .await;
        }

        pub async fn delete_event(&self, idx: u8) {
            let mut events = self.events.lock();
            let id = *events
                .keys()
                .find(|id| id.idx == idx)
                .expect("No matching EventId");
            let removed = events.remove(&id).unwrap();
            self.scheduler
                .event_changed(&EventChange::Deleted(removed))
                .await;
        }

        pub async fn edit_event_time(&self, idx: u8, secs_from_start: u64) {
            let mut events = self.events.lock();
            let (id, event) = events
                .iter()
                .find(|(id, _)| id.idx == idx)
                .expect("No matching EventId");
            let (id, mut event) = (id.clone(), event.clone());
            Arc::make_mut(&mut event).datetime = self
                .time_source
                .from_start(Duration::from_secs(secs_from_start))
                .with_timezone(&Tz::UTC);

            events.insert(id, event.clone());
            self.scheduler
                .event_changed(&EventChange::Edited(event))
                .await;
        }

        /// Edit the event with given index in some arbitrary way other than changing its datetime.
        pub async fn edit_event_non_time(&self, idx: u8) {
            let mut events = self.events.lock();
            let (id, event) = events
                .iter()
                .find(|(id, _)| id.idx == idx)
                .expect("No matching EventId");
            let (id, mut event) = (id.clone(), event.clone());
            Arc::make_mut(&mut event).description.push_str("foo");

            events.insert(id, event.clone());
            self.scheduler
                .event_changed(&EventChange::Edited(event))
                .await;
        }
    }

    #[async_trait]
    impl ScheduledActionHandler for EventSchedulerTest {
        async fn with_event_for_id<F, T>(&self, id: EventId, func: F) -> Option<T>
        where
            F: FnOnce(&Event) -> T + Send,
        {
            self.events.lock().get(&id).map(|event| func(event))
        }

        async fn perform_action(&self, action: &ScheduledAction) -> Result<()> {
            self.last_actions
                .lock()
                .get_or_insert(Vec::new())
                .push(action.clone());
            Ok(())
        }
    }

    // A test time source that generates DateTime<Utc> objects that advance in lockstep with tokio
    // sense of time, e.g. as tokio's time is paused or advanced in tests.
    #[derive(Debug, Clone)]
    struct TestTimeSource {
        start_utc: DateTime<Utc>,
        start_instant: Instant,
    }

    impl TestTimeSource {
        pub fn new() -> Self {
            TestTimeSource {
                start_utc: Utc::now(),
                start_instant: Instant::now(),
            }
        }

        pub fn from_start(&self, duration: Duration) -> DateTime<Utc> {
            self.start_utc + SignedDuration::from_std(duration).unwrap()
        }
    }

    impl TimeSource for TestTimeSource {
        fn utc_now(&self) -> DateTime<Utc> {
            let dur = Instant::now().duration_since(self.start_instant);
            self.from_start(dur)
        }
    }

    fn test_event(time_source: &TestTimeSource, idx: u8, secs_from_start: u64) -> Arc<Event> {
        Arc::new(Event {
            id: EventId {
                activity: Activity::Custom,
                idx,
            },
            datetime: time_source
                .from_start(Duration::from_secs(secs_from_start))
                .with_timezone(&Tz::UTC),
            ..Default::default()
        })
    }

    #[tokio::test(start_paused = true)]
    async fn test_scheduler_with_initial_events() {
        let time_source = TestTimeSource::new();
        let config = EventSchedulerConfig {
            alert: Duration::from_secs(10),
            cleanup: Duration::from_secs(30),
        };
        let events = vec![
            test_event(&time_source, 1, 90),
            test_event(&time_source, 2, 30),
            test_event(&time_source, 3, 50),
        ];
        let test = EventSchedulerTest::start(events.iter(), config, time_source);

        // t == 11
        tokio::time::sleep(Duration::from_secs(11)).await;
        assert!(test.take_last_action().is_none());

        // t == 21
        tokio::time::sleep(Duration::from_secs(10)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 2);
        assert_eq!(last.action, EventAction::Alert);

        // t == 41
        tokio::time::sleep(Duration::from_secs(20)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 3);
        assert_eq!(last.action, EventAction::Alert);

        // t == 61
        tokio::time::sleep(Duration::from_secs(20)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 2);
        assert_eq!(last.action, EventAction::Cleanup);

        // t == 81
        // Check that both actions occured.
        tokio::time::sleep(Duration::from_secs(20)).await;
        let last = test.take_last_actions().unwrap();
        assert_eq!(last.len(), 2);
        assert_eq!(last[0].id.idx, 1);
        assert_eq!(last[0].action, EventAction::Alert);
        assert_eq!(last[1].id.idx, 3);
        assert_eq!(last[1].action, EventAction::Cleanup);

        // t == 121
        tokio::time::sleep(Duration::from_secs(40)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 1);
        assert_eq!(last.action, EventAction::Cleanup);
    }

    #[tokio::test(start_paused = true)]
    async fn test_scheduler_add_edit_delete() {
        let time_source = TestTimeSource::new();
        let config = EventSchedulerConfig {
            alert: Duration::from_secs(10),
            cleanup: Duration::from_secs(30),
        };
        let events = vec![
            test_event(&time_source, 1, 200),
            test_event(&time_source, 2, 40),
        ];
        let test = EventSchedulerTest::start(events.iter(), config, time_source.clone());

        // t == 21
        tokio::time::sleep(Duration::from_secs(21)).await;
        assert!(test.take_last_action().is_none());

        // t == 31
        tokio::time::sleep(Duration::from_secs(10)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 2);
        assert_eq!(last.action, EventAction::Alert);

        // Edit both events, changing their times.
        // Note that event 1's Alert action shouldn't happen since it's in the past.
        test.edit_event_time(1, 30).await;
        test.edit_event_time(2, 50).await;

        // t == 41
        tokio::time::sleep(Duration::from_secs(10)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 2);
        assert_eq!(last.action, EventAction::Alert);

        // Edit both events, but not changing their times.
        test.edit_event_non_time(1).await;
        test.edit_event_non_time(2).await;

        // t == 61
        tokio::time::sleep(Duration::from_secs(20)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 1);
        assert_eq!(last.action, EventAction::Cleanup);

        // Delete the remaining event and add a new one.
        test.delete_event(2).await;
        test.add_event(test_event(&time_source, 4, 200)).await;

        // t == 191
        tokio::time::sleep(Duration::from_secs(130)).await;
        let last = test.take_last_action().unwrap();
        assert_eq!(last.id.idx, 4);
        assert_eq!(last.action, EventAction::Alert);
    }
}
