use crate::{
    event::{Event, EventChange, EventId},
    store::{PersistentStore, PersistentStoreBuilder},
};
use anyhow::Result;
use derivative::Derivative;
use serenity::{model::id::ChannelId, prelude::*};
use std::{collections::HashMap, sync::Arc};

mod channel;
mod fixed;

use channel::EventChannel;
pub use channel::EventChannelFilterFn;
pub use fixed::EventEmbedMessage;

#[derive(Default)]
pub struct EmbedManagerConfig {
    pub event_channels: HashMap<ChannelId, EventChannelFilterFn>,
}

impl std::fmt::Debug for EmbedManagerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(
                self.event_channels
                    .keys()
                    .zip(std::iter::repeat("EventChannelFilterFn")),
            )
            .finish()
    }
}

impl EmbedManagerConfig {
    fn create_event_channels<'a, I>(self, ctx: &Context, initial_events: I) -> Vec<EventChannel>
    where
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        self.event_channels
            .into_iter()
            .map(|(chan_id, filter)| {
                EventChannel::new(ctx.clone(), chan_id, filter, initial_events.clone())
            })
            .collect()
    }
}

const STORE_NAME: &str = "embeds.json";

#[derive(Derivative)]
#[derivative(Debug)]
pub struct EmbedManager {
    #[derivative(Debug = "ignore")]
    ctx: Context,

    event_channels: Vec<EventChannel>,

    // Messages that this event's embed has been added to, and which need to be updated when the
    // event is updated.
    // TODO: Could we keep track of a hash of the last embed's data, so we can update on restart if
    // the embed content has changed (say through a code change)?
    embed_messages: fixed::EmbedMessages,
    store: PersistentStore<fixed::EmbedMessages>,
}

impl EmbedManager {
    pub async fn new<'a, I>(
        ctx: Context,
        store_builder: &PersistentStoreBuilder,
        config: EmbedManagerConfig,
        initial_events: I,
    ) -> Result<Self>
    where
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let store = store_builder.build(STORE_NAME).await?;
        let embed_messages = store.load().await?;

        let event_channels = config.create_event_channels(&ctx, initial_events);
        Ok(EmbedManager {
            ctx,
            event_channels,
            embed_messages,
            store,
        })
    }

    pub async fn event_changed(&mut self, change: EventChange) -> Result<()> {
        for chan in self.event_channels.iter_mut() {
            chan.handle_event_change(change.clone()).await;
        }

        match change {
            EventChange::Added(_) => {}
            EventChange::Edited(event) | EventChange::Alert(event) => {
                self.embed_messages.start_updating_embeds(&self.ctx, &event)
            }
            EventChange::Deleted(event) => {
                self.embed_messages
                    .start_deleting_embeds(&self.ctx, &event)
                    .await;
                self.store.store(&self.embed_messages).await?;
            }
        }
        Ok(())
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
