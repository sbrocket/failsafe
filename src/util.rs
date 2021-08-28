use crate::{event::EventManager, guild::GuildManager};
use anyhow::{format_err, Result};
use rand::{distributions::Alphanumeric, prelude::*};
use serenity::{
    async_trait,
    builder::{CreateComponents, CreateEmbed},
    client::Context,
    http::Http,
    model::{
        interactions::{
            application_command::{
                ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
            },
            message_component::MessageComponentInteraction,
        },
        prelude::*,
    },
    prelude::*,
};
use std::{io::ErrorKind, path::PathBuf, sync::Arc};
use thiserror::Error;
use tokio::fs::File;

pub use serenity::model::interactions::application_command::ApplicationCommandInteractionDataOptionValue as OptionValue;

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

#[derive(Error, Debug)]
pub enum OptionError {
    #[error("No value for option '{0}'")]
    MissingValue(String),
    #[error("Missing resolved value for option '{0}'")]
    MissingResolvedValue(String),
}

pub trait OptionsExt {
    fn get_resolved(&self, name: impl AsRef<str>) -> Result<Option<&OptionValue>, OptionError>;
}

impl OptionsExt for &Vec<ApplicationCommandInteractionDataOption> {
    fn get_resolved(&self, name: impl AsRef<str>) -> Result<Option<&OptionValue>, OptionError> {
        let name = name.as_ref();
        let option = if let Some(option) = self.iter().find(|opt| opt.name == name) {
            option
        } else {
            return Ok(None);
        };
        option.resolved.as_ref().map_or_else(
            || Err(OptionError::MissingResolvedValue(name.to_owned())),
            |v| Ok(Some(v)),
        )
    }
}

#[async_trait]
pub trait ContextExt {
    async fn get_event_manager<I: InteractionExt>(
        &self,
        interaction: &I,
    ) -> Result<Arc<EventManager>>;
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

pub trait MemberLike: Send + Sync {
    fn user(&self) -> &User;
    fn id(&self) -> UserId;
    fn display_name(&self) -> &str;
}

impl MemberLike for Member {
    fn user(&self) -> &User {
        &self.user
    }

    fn id(&self) -> UserId {
        self.user.id
    }

    fn display_name(&self) -> &str {
        self.nick
            .as_deref()
            .unwrap_or_else(|| self.user.name.as_str())
    }
}

impl MemberLike for (&User, &PartialMember) {
    fn user(&self) -> &User {
        &self.0
    }

    fn id(&self) -> UserId {
        self.0.id
    }

    fn display_name(&self) -> &str {
        self.1
            .nick
            .as_deref()
            .unwrap_or_else(|| self.0.name.as_str())
    }
}

// From https://discord.com/developers/docs/topics/opcodes-and-status-codes#json
pub enum DiscordJsonErrorCode {
    UnknownMessage = 10008,
}

pub trait SerenityErrorExt {
    fn is_discord_json_error(&self, code: DiscordJsonErrorCode) -> bool;
}

impl SerenityErrorExt for SerenityError {
    fn is_discord_json_error(&self, code: DiscordJsonErrorCode) -> bool {
        if let SerenityError::Http(http_err) = self {
            if let HttpError::UnsuccessfulRequest(err_resp) = http_err.as_ref() {
                return err_resp.error.code == code as isize;
            }
        }
        false
    }
}

/// Intended to be used with the #[serde(with = "module")] annotation on DateTime<Tz> fields
pub mod serialize_datetime_tz {
    use super::*;
    use chrono::{DateTime, Utc};
    use chrono_tz::Tz;
    use serde::{
        de::{Error, Unexpected},
        Deserialize, Deserializer, Serialize, Serializer,
    };
    use std::str::FromStr;

    #[derive(Serialize, Deserialize)]
    struct UtcDatetimeAndTimezone<'a>(DateTime<Utc>, &'a str);

    pub fn serialize<S>(dt: &DateTime<Tz>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(
            &UtcDatetimeAndTimezone(dt.with_timezone(&Utc), dt.timezone().name()),
            s,
        )
    }

    pub fn deserialize<'de, D>(d: D) -> Result<DateTime<Tz>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: UtcDatetimeAndTimezone = Deserialize::deserialize(d)?;
        let tz = Tz::from_str(value.1)
            .map_err(|s| D::Error::invalid_value(Unexpected::Str(&s), &"a chrono_tz::Tz name"))?;
        Ok(value.0.with_timezone(&tz))
    }
}
