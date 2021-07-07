use anyhow::{format_err, Result};
use rand::{distributions::Alphanumeric, prelude::*};
use serde_json::Value;
use serenity::{
    async_trait,
    builder::{CreateComponents, CreateEmbed},
    client::Context,
    http::Http,
    model::{
        interactions::{
            application_command::{
                ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
                ApplicationCommandInteractionDataOptionValue as OptionValue,
            },
            message_component::MessageComponentInteraction,
        },
        prelude::*,
    },
};
use std::{io::ErrorKind, path::PathBuf, sync::Arc};
use tokio::fs::File;

use crate::{event::EventManager, guild::GuildManager};

const EPHEMERAL_FLAG: InteractionApplicationCommandCallbackDataFlags =
    InteractionApplicationCommandCallbackDataFlags::EPHEMERAL;

#[async_trait]
pub trait InteractionExt: Send + Sync {
    const KIND: InteractionType;

    fn kind(&self) -> InteractionType {
        Self::KIND
    }

    fn guild_id(&self) -> Option<GuildId>;

    async fn create_response<'a>(
        &'a self,
        http: impl AsRef<Http> + Send + Sync + 'a,
        content: impl ToString + Send + Sync + 'a,
        ephemeral: bool,
    ) -> serenity::Result<()>;

    async fn create_embed_response<'a>(
        &'a self,
        http: impl AsRef<Http> + Send + Sync + 'a,
        content: impl ToString + Send + Sync + 'a,
        embed: CreateEmbed,
        components: CreateComponents,
        ephemeral: bool,
    ) -> serenity::Result<()>;

    async fn edit_response<'a>(
        &'a self,
        http: impl AsRef<Http> + Send + Sync + 'a,
        content: impl ToString + Send + Sync + 'a,
    ) -> serenity::Result<Message>;

    async fn edit_embed_response<'a>(
        &'a self,
        http: impl AsRef<Http> + Send + Sync + 'a,
        content: impl ToString + Send + Sync + 'a,
        embed: CreateEmbed,
        components: CreateComponents,
    ) -> serenity::Result<Message>;

    async fn create_ack_response<'a>(
        &'a self,
        http: impl AsRef<Http> + Send + Sync + 'a,
    ) -> serenity::Result<()>;

    async fn create_followup<'a>(
        &'a self,
        http: impl AsRef<Http> + Send + Sync + 'a,
        content: impl ToString + Send + Sync + 'a,
        ephemeral: bool,
    ) -> serenity::Result<Message>;
}

pub trait OptionsExt {
    fn get_value(&self, name: impl AsRef<str>) -> Result<Option<&Value>>;

    fn get_resolved(&self, name: impl AsRef<str>) -> Result<Option<&OptionValue>>;
}

#[async_trait]
pub trait ContextExt {
    async fn get_event_manager<I: InteractionExt>(
        &self,
        interaction: &I,
    ) -> Result<Arc<EventManager>>;
}

macro_rules! impl_interaction_ext {
    ($ty:ty, $kind:ident) => {
        #[async_trait]
        impl InteractionExt for $ty {
            const KIND: InteractionType = InteractionType::$kind;

            fn guild_id(&self) -> Option<GuildId> {
                self.guild_id
            }

            async fn create_response<'a>(
                &'a self,
                http: impl AsRef<Http> + Send + Sync + 'a,
                content: impl ToString + Send + Sync + 'a,
                ephemeral: bool,
            ) -> serenity::Result<()> {
                let http = http.as_ref();
                self.create_interaction_response(http, |resp| {
                    resp.interaction_response_data(|msg| {
                        if ephemeral {
                            msg.flags(EPHEMERAL_FLAG);
                        }
                        msg.content(content.to_string())
                    })
                })
                .await
            }

            async fn create_embed_response<'a>(
                &'a self,
                http: impl AsRef<Http> + Send + Sync + 'a,
                content: impl ToString + Send + Sync + 'a,
                embed: CreateEmbed,
                components: CreateComponents,
                ephemeral: bool,
            ) -> serenity::Result<()> {
                let http = http.as_ref();
                self.create_interaction_response(http, |resp| {
                    resp.interaction_response_data(|msg| {
                        if ephemeral {
                            msg.flags(EPHEMERAL_FLAG);
                        }
                        msg.content(content.to_string())
                            .add_embed(embed)
                            .components(|c| {
                                *c = components;
                                c
                            })
                    })
                })
                .await
            }

            async fn edit_response<'a>(
                &'a self,
                http: impl AsRef<Http> + Send + Sync + 'a,
                content: impl ToString + Send + Sync + 'a,
            ) -> serenity::Result<Message> {
                let http = http.as_ref();
                self.edit_original_interaction_response(http, |resp| {
                    resp.content(content.to_string())
                })
                .await
            }

            async fn edit_embed_response<'a>(
                &'a self,
                http: impl AsRef<Http> + Send + Sync + 'a,
                content: impl ToString + Send + Sync + 'a,
                embed: CreateEmbed,
                components: CreateComponents,
            ) -> serenity::Result<Message> {
                let http = http.as_ref();
                self.edit_original_interaction_response(http, |resp| {
                    resp.content(content.to_string())
                        .add_embed(embed)
                        .components(|c| {
                            *c = components;
                            c
                        })
                })
                .await
            }

            async fn create_ack_response<'a>(
                &'a self,
                http: impl AsRef<Http> + Send + Sync + 'a,
            ) -> serenity::Result<()> {
                let http = http.as_ref();
                self.create_interaction_response(http, |resp| {
                    resp.kind(InteractionResponseType::DeferredUpdateMessage)
                })
                .await
            }

            async fn create_followup<'a>(
                &'a self,
                http: impl AsRef<Http> + Send + Sync + 'a,
                content: impl ToString + Send + Sync + 'a,
                ephemeral: bool,
            ) -> serenity::Result<Message> {
                let http = http.as_ref();
                self.create_followup_message(http, |msg| {
                    if ephemeral {
                        msg.flags(EPHEMERAL_FLAG);
                    }
                    msg.content(content.to_string())
                })
                .await
            }
        }
    };
}

impl_interaction_ext!(ApplicationCommandInteraction, ApplicationCommand);
impl_interaction_ext!(MessageComponentInteraction, MessageComponent);

impl OptionsExt for &Vec<ApplicationCommandInteractionDataOption> {
    fn get_value(&self, name: impl AsRef<str>) -> Result<Option<&Value>> {
        let name = name.as_ref();
        let option = if let Some(option) = self.iter().find(|opt| opt.name == name) {
            option
        } else {
            return Ok(None);
        };
        option.value.as_ref().map_or_else(
            || Err(format_err!("No value for option '{}'", name)),
            |v| Ok(Some(v)),
        )
    }

    fn get_resolved(&self, name: impl AsRef<str>) -> Result<Option<&OptionValue>> {
        let name = name.as_ref();
        let option = if let Some(option) = self.iter().find(|opt| opt.name == name) {
            option
        } else {
            return Ok(None);
        };
        option.resolved.as_ref().map_or_else(
            || Err(format_err!("No resolved value for option '{}'", name)),
            |v| Ok(Some(v)),
        )
    }
}

#[async_trait]
impl ContextExt for Context {
    async fn get_event_manager<I: InteractionExt>(
        &self,
        interaction: &I,
    ) -> Result<Arc<EventManager>> {
        let type_map = self.data.read().await;
        let guild_manager = type_map
            .get::<GuildManager>()
            .expect("No GuildManager in TypeMap");

        let guild_id = interaction
            .guild_id()
            .expect("Called with non-guild command Interaction");
        guild_manager.get_event_manager(guild_id).await
    }
}

pub async fn tempfile() -> Result<(PathBuf, File)> {
    const TEMP_PREFIX: &str = "tmpfile_";
    const RAND_LEN: usize = 10;
    const RETRIES: usize = 4;

    for _ in 0..RETRIES {
        let mut tempname = String::with_capacity(TEMP_PREFIX.len() + RAND_LEN);
        tempname.push_str(TEMP_PREFIX);
        tempname.extend(
            thread_rng()
                .sample_iter(Alphanumeric)
                .take(RAND_LEN)
                .map(char::from),
        );

        let mut path = std::env::temp_dir();
        path.push(tempname);
        match File::create(&path).await {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            file => return Ok((path, file?)),
        };
    }
    Err(format_err!("Failed to create tempfile"))
}
