use super::{CommandOption, LeafCommand};
use crate::{
    activity::{Activity, ActivityType},
    command::OptionType,
    event::EventManager,
    time::parse_datetime,
    util::*,
};
use anyhow::{format_err, Context as _, Result};
use paste::paste;
use serde_json::Value;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{
        ApplicationCommandInteractionDataOption, Interaction,
        InteractionApplicationCommandCallbackDataFlags, InteractionResponseType,
    },
    utils::MessageBuilder,
};
use std::{concat, time::Duration};
use tracing::{debug, error};

define_command!(Lfg, "lfg", "Create and interact with scheduled events",
                Subcommands: [LfgJoin, LfgCreate]);
define_command!(LfgJoin, "join", "Join an existing event", Leaf);

macro_rules! define_create_commands {
    ($($enum_name:ident: ($name:literal, $cmd:literal)),+ $(,)?) => {
        paste! {
            define_command!(LfgCreate, "create", "Create a new event",
                            Subcommands: [
                                $(
                                    [<LfgCreate $enum_name>]
                                ),+
                            ]);

            $(
                define_command!([<LfgCreate $enum_name>], $cmd,
                                concat!("Create a new ", $name, " event"), Leaf);

                impl LfgCreateActivity for [<LfgCreate $enum_name>] {
                    const ACTIVITY: ActivityType = ActivityType::$enum_name;
                }
            )+
        }
    }
}

with_activity_types! { define_create_commands }

#[async_trait]
impl LeafCommand for LfgJoin {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        _options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let user = interaction.get_user()?;
        interaction
            .create_interaction_response(&ctx, |resp| {
                let message = MessageBuilder::new()
                    .push("Hi ")
                    .mention(user)
                    .push("! There's no events to join yet...because I can't create them yet...")
                    .build();
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content(message))
            })
            .await?;
        Ok(())
    }
}

pub trait LfgCreateActivity: LeafCommand {
    const ACTIVITY: ActivityType;
}

const LFG_CREATE_DESCRIPTION_TIMEOUT_SEC: u64 = 60;
const EPHEMERAL_FLAG: InteractionApplicationCommandCallbackDataFlags =
    InteractionApplicationCommandCallbackDataFlags::EPHEMERAL;

#[async_trait]
impl<T: LfgCreateActivity> LeafCommand for T {
    fn options(&self) -> Vec<CommandOption> {
        let activities = Activity::activities_with_type(T::ACTIVITY)
            .map(|a| (a.name().to_string(), a.id_prefix().to_string()))
            .collect();
        vec![
            CommandOption {
                name: "activity",
                description: "Activity for this event",
                required: true,
                option_type: OptionType::String(activities),
            },
            CommandOption {
                name: "datetime",
                description: "Date & time for this event, in \"h:m am/pm tz mm/dd\" format (e.g. \"8:00 PM CT 4/20\")",
                required: true,
                option_type: OptionType::String(vec![]),
            },
        ]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let user = interaction.get_user()?;
        let activity = match options.get_value("activity")? {
            Value::String(v) => Ok(v),
            v => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;
        let activity = Activity::activity_with_id_prefix(activity)
            .ok_or_else(|| format_err!("Unexpected activity value: {:?}", activity))?;
        let datetime = match options.get_value("datetime")? {
            Value::String(v) => Ok(v),
            v => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;

        let send_response = |content| {
            interaction.create_interaction_response(&ctx, |resp| {
                resp.interaction_response_data(|msg| msg.content(content).flags(EPHEMERAL_FLAG))
            })
        };

        // Check that datetime format is good.
        let datetime = match parse_datetime(&datetime) {
            Ok(datetime) => datetime,
            Err(err) => {
                send_response(format!(
                    "Sorry Captain, I don't understand what '{}' means",
                    datetime
                ))
                .await?;
                return Err(
                    err.context(format!("Unable to parse provided datetime: {:?}", datetime))
                );
            }
        };
        debug!("Parsed datetime: {}", datetime);

        // TODO: Maybe check that the date doesn't seem unreasonably far away? (>1 months, ask to
        // confirm?)

        // Ask for the event description in the main response.
        send_response(format!(
            "Captain, what's so special about this... *uhhh, \"{}\"?*  ...event anyway? \
                    Describe it for me...but in simple terms like for a Guardia...*oop!*",
            activity
        ))
        .await?;

        // Wait for the user to reply with the description.
        if let Some(reply) = user
            .await_reply(&ctx)
            .timeout(Duration::from_secs(LFG_CREATE_DESCRIPTION_TIMEOUT_SEC))
            .await
        {
            // Immediately delete the user's (public) message since the rest of the bot interaction
            // is ephemeral.
            reply.delete(&ctx).await?;

            let description = &reply.content;
            debug!("Got event description: {:?}", description);

            let mut type_map = ctx.data.write().await;
            let event_manager = type_map.get_mut::<EventManager>().unwrap();
            let event = match event_manager
                .create_event(&user, activity, datetime, description)
                .await
            {
                Ok(event) => event,
                Err(err) => {
                    if let Err(edit_err) = interaction
                        .edit_original_interaction_response(&ctx, |resp| {
                            resp.content(
                                "Sorry Captain, I seem to be having trouble creating your event...",
                            )
                        })
                        .await
                    {
                        error!(
                            "Failed to edit response to indicate an error: {:?}",
                            edit_err
                        );
                    }
                    return Err(err.context("Failed to create event"));
                }
            };

            // TODO: Add buttons to join event, post publicly
            let content = "Your event has been created, Captain! Would you like to join it or have me post it publicly?";
            interaction
                .edit_original_interaction_response(&ctx, |resp| {
                    resp.content(content).add_embed(event.as_embed())
                })
                .await
                .context("Failed to edit response after creating event")?;
        } else {
            // Timed out waiting for the description, send a followup message so that the user can
            // see the description request still and so the mention works.
            let content = MessageBuilder::new()
                .push("**Yoohoo, ")
                .mention(user)
                .push("!** Are the Fallen dismantling *your* brain now? *Whatever, just gonna ask me again...not like I'm going anywhere...*")
                .build();
            interaction
                .create_followup_message(&ctx, |msg| msg.content(content).flags(EPHEMERAL_FLAG))
                .await?;
        };
        Ok(())
    }
}
