use crate::{
    activity::ActivityType,
    event::{Event, EventChange, EventId},
    store::{PersistentStore, PersistentStoreBuilder},
};
use anyhow::Result;
use derivative::Derivative;
use serenity::{model::id::ChannelId, prelude::*};
use std::sync::Arc;

mod channel;
mod fixed;

use channel::EventChannel;
pub use fixed::EventEmbedMessage;

// TODO: Replace this hardcoded event channel configuration with commands to configure this.
fn test_event_channels<'a, I>(ctx: &Context, initial_events: I) -> Vec<EventChannel>
where
    I: Iterator<Item = &'a Arc<Event>> + Clone,
{
    let mut v = Vec::new();
    // #raid-lfg
    v.push(EventChannel::new(
        ctx.clone(),
        ChannelId(853744114377687090),
        Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Raid),
        initial_events.clone(),
    ));
    // #pve-lfg
    v.push(EventChannel::new(
        ctx.clone(),
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
        ctx.clone(),
        ChannelId(853744160926597150),
        Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Crucible),
        initial_events.clone(),
    ));
    // #special-lfg
    v.push(EventChannel::new(
        ctx.clone(),
        ChannelId(853744175908257813),
        Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Custom),
        initial_events.clone(),
    ));
    // #all-lfg
    v.push(EventChannel::new(
        ctx.clone(),
        ChannelId(853744186545274880),
        Box::new(|_: &Event| true),
        initial_events,
    ));
    v
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
        initial_events: I,
    ) -> Result<Self>
    where
        I: Iterator<Item = &'a Arc<Event>> + Clone,
    {
        let store = store_builder.build(STORE_NAME).await?;
        let embed_messages = store.load().await?;

        let event_channels = test_event_channels(&ctx, initial_events);
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
            EventChange::Edited(event) => {
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
