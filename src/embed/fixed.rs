use crate::event::{Event, EventId};
use anyhow::{Context as _, Result};
use chrono::{Duration, Utc};
use futures::prelude::*;
use lazy_static::lazy_static;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serenity::{
    http::Http,
    model::{
        id::{ChannelId, MessageId},
        interactions::application_command::ApplicationCommandInteraction,
    },
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

#[derive(Debug, Default)]
pub struct EmbedMessages {
    messages: Arc<RwLock<HashMap<EventId, Vec<EventEmbedMessage>>>>,
}

impl EmbedMessages {
    pub async fn keep_embed_updated(&self, event_id: EventId, mut message: EventEmbedMessage) {
        let mut msgs = self.messages.write().await;
        {
            let event_msgs = msgs.entry(event_id).or_default();
            if event_msgs.contains(&message) {
                warn!("Event {} already tracking message {:?}", event_id, message);
                return;
            }
            message.strip_unneeded_fields();
            message.schedule_ephemeral_response_cleanup();
            event_msgs.push(message);
        }

        // Cleanup any expired EphemeralResponse entries while we're holding the write lock
        msgs.values_mut()
            .for_each(|vec| vec.retain(|m| !m.expired()));
    }

    /// Asychronously (in a spawned task) update the embeds in tracked messages.
    pub fn start_updating_embeds(&self, http: impl AsRef<Arc<Http>>, event: &Event) {
        let embed = event.as_embed();
        let alert_message = event.alert_protocol_message().unwrap_or_default();
        let event_id = event.id;
        let http = http.as_ref().clone();
        let messages = self.messages.clone();
        let update_fut = async move {
            let messages = messages.read().await;
            let empty = vec![];
            let event_messages = messages.get(&event_id).unwrap_or(&empty);

            future::join_all(event_messages.iter().filter(|m| !m.expired()).map(|msg| {
                let (http, embed, alert_message) = (&http, &embed, &alert_message);
                async move {
                    match msg {
                        EventEmbedMessage::Normal(chan_id, msg_id) => {
                            chan_id
                                .edit_message(http, msg_id, |edit| {
                                    edit.embed(|e| {
                                        *e = embed.clone();
                                        e
                                    })
                                    .content(alert_message.clone())
                                })
                                .await
                        }
                        EventEmbedMessage::EphemeralResponse(interaction, ..) => {
                            interaction
                                .edit_original_interaction_response(&http, |resp| {
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

        tokio::spawn(async move {
            let results = update_fut.await;
            if results.is_empty() {
                return;
            }

            let (successes, failures): (Vec<_>, Vec<_>) =
                results.into_iter().partition(Result::is_ok);
            let count = successes.len() + failures.len();
            if failures.is_empty() {
                info!("Successfully updated fixed embeds for event {}", event_id);
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

    pub async fn start_deleting_embeds(&self, http: impl AsRef<Arc<Http>>, event: &Event) {
        let event_id = event.id;
        let http = http.as_ref().clone();
        let mut messages = self.messages.write().await;
        let mut event_messages = if let Some(m) = messages.remove(&event_id) {
            m
        } else {
            return;
        };

        let update_fut = async move {
            future::join_all(
                event_messages
                    .drain(..)
                    .filter(|m| !m.expired())
                    .map(|msg| {
                        let http = &http;
                        async move {
                            match msg {
                                EventEmbedMessage::Normal(chan_id, msg_id) => {
                                    chan_id.delete_message(http, msg_id).await
                                }
                                EventEmbedMessage::EphemeralResponse(interaction, ..) => {
                                    interaction
                                        .edit_original_interaction_response(http, |resp| {
                                            // set_embeds(vec![]) does nothing, rather than removing
                                            // existing embeds, so set embeds empty explicity
                                            resp.0
                                                .insert("embeds", serde_json::Value::Array(vec![]));
                                            resp.components(|c| {
                                                *c = Default::default();
                                                c
                                            })
                                        })
                                        .await
                                        .and(Ok(()))
                                }
                            }
                            .context("Failed to delete message")
                        }
                    }),
            )
            .await
        };

        tokio::spawn(async move {
            let results = update_fut.await;
            let (successes, failures): (Vec<_>, Vec<_>) =
                results.into_iter().partition(Result::is_ok);
            let count = successes.len() + failures.len();
            if failures.is_empty() {
                info!(
                    "Successfully deleted fixed embed messages for event {}",
                    event_id
                );
            } else if successes.is_empty() {
                error!(
                    "All ({}) embed messages failed to delete for event {}",
                    count, event_id
                );
                failures.into_iter().for_each(|f| error!("{:?}", f));
            } else {
                error!(
                    "Some ({}/{}) embed messages failed to delete for event {}",
                    failures.len(),
                    count,
                    event_id
                );
                failures.into_iter().for_each(|f| error!("{:?}", f));
            }
        });
    }
}

impl Serialize for EmbedMessages {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = futures::executor::block_on(self.messages.read());
        Serialize::serialize(&*value, s)
    }
}

impl<'de> Deserialize<'de> for EmbedMessages {
    fn deserialize<D>(d: D) -> Result<EmbedMessages, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value: HashMap<EventId, Vec<EventEmbedMessage>> = Deserialize::deserialize(d)?;

        // Do some special steps after deserializing this. Remove any expired ephemeral responses
        // that we no longer need to keep track of, and schedule cleanup for any not-yet-expired
        // responses.
        value.values_mut().for_each(|vec| {
            vec.retain(|m| !m.expired());
            vec.iter()
                .for_each(|m| m.schedule_ephemeral_response_cleanup());
        });

        Ok(EmbedMessages {
            messages: Arc::new(RwLock::new(value)),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum EventEmbedMessage {
    // A "normal" message in a channel, either posted directly by the bot or a non-ephemeral
    // interaction response.
    Normal(ChannelId, MessageId),
    // An ephemeral interaction response and the text of the response.
    // These cannot be edited by message ID, only through the Edit Original Interaction Response
    // endpoint, and then only within the 15 minute lifetime of the Interaction's token.
    //
    // As such, we will skip updating responses older than 15 minutes, and edit the responses to
    // only include the given text at 14 minutes to avoid stale embeds in the user's chat
    // scrollback.
    EphemeralResponse(ApplicationCommandInteraction, String),
}

lazy_static! {
    // Interaction tokens last 15 minutes, after which ephemeral responses can no longer be edited.
    static ref INTERACTION_LIFETIME: Duration = Duration::minutes(15);

    // The amount of time we wait before deleting an ephemeral response (to avoid stale embeds).
    static ref EPHEMERAL_LIFETIME: Duration = *INTERACTION_LIFETIME - Duration::seconds(30);
}

impl EventEmbedMessage {
    fn strip_unneeded_fields(&mut self) {
        match self {
            EventEmbedMessage::EphemeralResponse(interaction, ..) => {
                interaction.data.options.clear();
                interaction.data.resolved = Default::default();
                interaction.guild_id = None;
                interaction.member = None;
                interaction.user = Default::default();
            }
            _ => {}
        }
    }

    fn expired(&self) -> bool {
        match self {
            EventEmbedMessage::Normal(..) => false,
            EventEmbedMessage::EphemeralResponse(interaction, _) => {
                Utc::now().signed_duration_since(interaction.id.created_at())
                    >= *INTERACTION_LIFETIME
            }
        }
    }

    fn schedule_ephemeral_response_cleanup(&self) {
        if let EventEmbedMessage::EphemeralResponse(interaction, content) = self {
            if self.expired() {
                return;
            }

            let delay =
                *EPHEMERAL_LIFETIME - Utc::now().signed_duration_since(interaction.id.created_at());
            let delay = if delay < Duration::zero() {
                std::time::Duration::new(0, 0)
            } else {
                delay.to_std().expect("Already checked <0, shouldn't fail")
            };

            let interaction = interaction.clone();
            let content = content.clone();
            tokio::spawn(async move {
                debug!(
                    "Removing embeds from ephemeral response for interaction {} in {:?}",
                    interaction.id, delay
                );
                tokio::time::sleep(delay).await;

                let http = Http::new_with_application_id(interaction.application_id.into());
                if let Err(err) = interaction
                    .edit_original_interaction_response(&http, |resp| {
                        // set_embeds(vec![]) does nothing, rather than removing
                        // existing embeds, so set embeds empty explicity
                        resp.0.insert("embeds", serde_json::Value::Array(vec![]));
                        resp.content(content).components(|c| {
                            *c = Default::default();
                            c
                        })
                    })
                    .await
                {
                    error!(
                        "Failed to remove embeds from ephemeral response for interaction created at {}: {:?}",
                        interaction.id.created_at(), err
                    );
                }
            });
        }
    }
}

impl PartialEq for EventEmbedMessage {
    fn eq(&self, other: &Self) -> bool {
        use EventEmbedMessage::*;
        match (self, other) {
            (Normal(a1, a2), Normal(b1, b2)) => a1 == b1 && a2 == b2,
            (EphemeralResponse(a, ..), EphemeralResponse(b, ..)) => a.id == b.id,
            _ => false,
        }
    }
}

impl Eq for EventEmbedMessage {}
