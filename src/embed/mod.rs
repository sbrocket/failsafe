use crate::{
    activity::ActivityType,
    event::{Event, EventId},
    store::{PersistentStore, PersistentStoreBuilder},
};
use anyhow::Result;
use derivative::Derivative;
use serenity::{model::id::ChannelId, CacheAndHttp};
use std::sync::Arc;
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::warn;

mod channel;
mod fixed;

use channel::{EmbedAction, EmbedUpdater, EventChange, EventChannel};
pub use fixed::EventEmbedMessage;

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

const STORE_NAME: &str = "embeds.json";

// Rather than using an unbounded channel, which makes it impossible to get a signal if we're
// generating actions faster than they can be processed, this is an arbitrary buffer size and then
// check when sending if the buffer is currently full so that we can log.
const EMBED_ACTION_BUFFER_SIZE: usize = 20;

// TODO: Use a MessageCollector to collect messages in the event channels that aren't from the bot
// and delete them.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct EmbedManager {
    /// Configuration data for event channels, i.e. channels that events are automatically posted to
    /// based on a filter.
    event_channels: Vec<EventChannel>,

    #[derivative(Debug = "ignore")]
    http: Arc<CacheAndHttp>,
    action_send: mpsc::Sender<EmbedAction>,

    // Messages that this event's embed has been added to, and which need to be updated when the
    // event is updated.
    // TODO: Could we keep track of a hash of the last embed's data, so we can update on restart if
    // the embed content has changed (say through a code change)?
    embed_messages: fixed::EmbedMessages,
    store: PersistentStore<fixed::EmbedMessages>,
}

impl EmbedManager {
    pub async fn new<'a, I>(
        store_builder: &PersistentStoreBuilder,
        http: Arc<CacheAndHttp>,
        initial_events: I,
    ) -> Result<Self>
    where
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let store = store_builder.build(STORE_NAME).await?;
        let embed_messages = store.load().await?;

        let event_channels = test_event_channels(initial_events);

        let (send, recv) = mpsc::channel(EMBED_ACTION_BUFFER_SIZE);
        EmbedUpdater::start_processing_actions(http.clone(), recv);

        Ok(EmbedManager {
            event_channels,
            action_send: send,
            http,
            embed_messages,
            store,
        })
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
        self.embed_messages
            .start_updating_embeds(&self.http.http, &new);
    }

    pub async fn event_deleted(&mut self, event: Arc<Event>) -> Result<()> {
        for chan in self.event_channels.iter_mut() {
            for action in chan.handle_event_change(EventChange::Deleted(&event)) {
                send_log_on_backpressure(&self.action_send, action).await;
            }
        }
        self.embed_messages
            .start_deleting_embeds(&self.http.http, &event)
            .await;
        self.store.store(&self.embed_messages).await
    }

    pub async fn keep_embed_updated(
        &self,
        event_id: EventId,
        message: EventEmbedMessage,
    ) -> Result<()> {
        self.embed_messages
            .keep_embed_updated(event_id, message)
            .await;
        self.store.store(&self.embed_messages).await
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
