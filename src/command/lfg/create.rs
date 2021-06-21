use crate::{
    activity::{Activity, ActivityType},
    command::OptionType,
    event::{EventEmbedMessage, EventManager},
    time::parse_datetime,
    util::*,
};
use anyhow::{format_err, Context as _, Result};
use chrono::Utc;
use lazy_static::lazy_static;
use paste::paste;
use serde_json::Value;
use serenity::{
    client::Context,
    model::interactions::{ApplicationCommandInteractionDataOption, Interaction},
    utils::MessageBuilder,
};
use std::time::Duration;
use tracing::{debug, error};

macro_rules! define_create_commands {
    ($($enum_name:ident: ($name:literal, $cmd:literal)),+ $(,)?) => {
        paste! {
            define_command_group!(LfgCreate, "create", "Create a new event",
                            subcommands: [
                                $(
                                    [<LfgCreate $enum_name>]
                                ),+
                            ]);

            $(
                define_leaf_command!(
                    [<LfgCreate $enum_name>],
                    $cmd,
                    concat!("Create a new ", $name, " event"),
                    lfg_create,
                    options: [ [<ActivityOpt $enum_name>], DatetimeOpt ],
                );

                define_command_option!(
                    id: [<ActivityOpt $enum_name>],
                    name: "activity",
                    description: "Activity for this event",
                    required: true,
                    option_type: OptionType::String(&*[<ACTIVITIES_ $enum_name:upper>]),
                );

                lazy_static! {
                    static ref [<ACTIVITIES_ $enum_name:upper>]: Vec<(&'static str, &'static str)> = {
                        Activity::activities_with_type(ActivityType::$enum_name)
                            .map(|a| (a.name(), a.id_prefix()))
                            .collect()
                    };
                }
            )+
        }
    }
}

define_command_option!(
    id: DatetimeOpt,
    name: "datetime",
    description: "Date & time for this event, in \"h:m am/pm tz mm/dd\" format (e.g. \"8:00 PM CT 4/20\")",
    required: true,
    option_type: OptionType::String(&[]),
);

with_activity_types! { define_create_commands }

const LFG_CREATE_DESCRIPTION_TIMEOUT_SEC: u64 = 60;

#[command_attr::hook]
async fn lfg_create(
    ctx: &Context,
    interaction: &Interaction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
) -> Result<()> {
    let recv_time = Utc::now();

    let user = interaction.get_user()?;
    let activity = match options.get_value("activity")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required activity value")),
    }?;
    let activity = Activity::activity_with_id_prefix(activity)
        .ok_or_else(|| format_err!("Unexpected activity value: {:?}", activity))?;
    let datetime = match options.get_value("datetime")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required datetime value")),
    }?;

    // Check that datetime format is good.
    let datetime = match parse_datetime(&datetime) {
        Ok(datetime) => datetime,
        Err(err) => {
            let content = format!(
                "Sorry Captain, I don't understand what '{}' means",
                datetime
            );
            interaction.create_response(&ctx, content, true).await?;
            return Err(err.context(format!("Unable to parse provided datetime: {:?}", datetime)));
        }
    };
    debug!("Parsed datetime: {}", datetime);

    // TODO: Maybe check that the date doesn't seem unreasonably far away? (>1 months, ask to
    // confirm?)

    // Ask for the event description in the main response.
    let content = format!(
        "Captain, what's so special about this... *uhhh, \"{}\"?*  ...event anyway? \
                    Describe it for me...but in simple terms like for a Guardia...*oop!*",
        activity
    );
    interaction.create_response(&ctx, content, true).await?;

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
                    .edit_response(
                        &ctx,
                        "Sorry Captain, I seem to be having trouble creating your event...",
                    )
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

        let content = format!("Your event **{}** has been created, Captain!", event.id);
        interaction
            .edit_embed_response(&ctx, &content, event.as_embed(), event.event_buttons())
            .await
            .context("Failed to edit response after creating event")?;
        event
            .keep_embed_updated(EventEmbedMessage::EphemeralResponse(
                interaction.clone(),
                recv_time,
                content,
            ))
            .await?;
    } else {
        // Timed out waiting for the description, send a followup message so that the user can
        // see the description request still and so the mention works.
        let content = MessageBuilder::new()
                .push("**Yoohoo, ")
                .mention(user)
                .push("!** Are the Fallen dismantling *your* brain now? *Whatever, just gonna ask me again...not like I'm going anywhere...*")
                .build();
        interaction.create_followup(&ctx, content, true).await?;
    };
    Ok(())
}
