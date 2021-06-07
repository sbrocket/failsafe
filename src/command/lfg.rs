use super::{CommandOption, LeafCommand};
use crate::{
    activity::{Activity, ActivityType},
    command::OptionType,
    event::{Event, EventEmbedMessage, EventHandle, EventId, EventManager},
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
        InteractionApplicationCommandCallbackDataFlags,
    },
    utils::MessageBuilder,
};
use std::{concat, str::FromStr, time::Duration};
use tracing::{debug, error};

define_command!(Lfg, "lfg", "Create and interact with scheduled events",
                Subcommands: [LfgJoin, LfgShow, LfgCreate]);
define_command!(LfgJoin, "join", "Join an existing event", Leaf);
define_command!(LfgShow, "show", "Display an existing event", Leaf);

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

/// Returns the matching Event or else an error message to use in the interaction reponse.
fn get_event_from_str(
    event_manager: &EventManager,
    id_str: impl AsRef<str>,
) -> Result<EventHandle<'_>, String> {
    let id_str = id_str.as_ref();
    match EventId::from_str(&id_str) {
        Ok(event_id) => match event_manager.get_event(&event_id) {
            Some(event) => Ok(event),
            None => Err(format!("I couldn't find an event with ID '{}'", event_id)),
        },
        Err(_) => {
            Err("That's not a valid event ID, Captain. They look like this: `dsc123`".to_owned())
        }
    }
}

/// Runs the given closure on the matching Event, returning the message it generates or else an
/// error message to use in the interaction reponse.
async fn edit_event_from_str(
    ctx: &Context,
    event_manager: &mut EventManager,
    id_str: impl AsRef<str>,
    edit_fn: impl FnOnce(&mut Event) -> String,
) -> Result<String> {
    let id_str = id_str.as_ref();
    match EventId::from_str(&id_str) {
        Ok(event_id) => {
            event_manager
                .edit_event(ctx, &event_id, |event| match event {
                    Some(event) => edit_fn(event),
                    None => format!("I couldn't find an event with ID '{}'", event_id),
                })
                .await
        }
        Err(_) => {
            Ok("That's not a valid event ID, Captain. They look like this: `dsc123`".to_owned())
        }
    }
}

#[async_trait]
impl LeafCommand for LfgJoin {
    fn options(&self) -> Vec<CommandOption> {
        vec![CommandOption {
            name: "event_id",
            description: "Event ID",
            required: true,
            option_type: OptionType::String(vec![]),
        }]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let user = interaction.get_user()?;
        let event_id = match options.get_value("event_id")? {
            Value::String(v) => Ok(v),
            v => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;

        let mut type_map = ctx.data.write().await;
        let event_manager = type_map.get_mut::<EventManager>().unwrap();
        let edit_result = edit_event_from_str(&ctx, event_manager, &event_id, |event| match event
            .join(&user)
        {
            Ok(()) => format!(
                "Added you to the {} event at {}!",
                event.activity,
                event.formatted_datetime()
            ),
            Err(_) => "You're already in that event!".to_owned(),
        })
        .await;
        let content = match edit_result {
            Ok(msg) => msg,
            Err(err) => {
                error!("Failed to edit event: {:?}", err);
                "Sorry Captain, I seem to be having trouble adding you to that event...".to_owned()
            }
        };

        interaction
            .create_interaction_response(&ctx, |resp| {
                resp.interaction_response_data(|msg| msg.content(content).flags(EPHEMERAL_FLAG))
            })
            .await?;
        Ok(())
    }
}

#[async_trait]
impl LeafCommand for LfgShow {
    fn options(&self) -> Vec<CommandOption> {
        vec![CommandOption {
            name: "event_id",
            description: "Event ID",
            required: true,
            option_type: OptionType::String(vec![]),
        }]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let event_id = match options.get_value("event_id")? {
            Value::String(v) => Ok(v),
            v => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;

        let type_map = ctx.data.read().await;
        let event_manager = type_map.get::<EventManager>().unwrap();
        let mut event = None;
        interaction
            .create_interaction_response(&ctx, |resp| {
                resp.interaction_response_data(|msg| {
                    match get_event_from_str(event_manager, &event_id) {
                        Ok(e) => {
                            let ret = msg.add_embed(e.as_embed());
                            event = Some(e);
                            ret
                        }
                        Err(content) => msg.content(content),
                    }
                })
            })
            .await?;
        if let Some(event) = event {
            let msg = interaction.get_interaction_response(&ctx).await?;
            event
                .keep_embed_updated(EventEmbedMessage::Normal(msg.channel_id, msg.id))
                .await?;
        }
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
            event
                .keep_embed_updated(EventEmbedMessage::EphemeralResponse(interaction.clone()))
                .await?;
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
