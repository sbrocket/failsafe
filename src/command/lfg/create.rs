use super::{ask_for_description, opts};
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
};
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
                    options: [ [<ActivityOpt $enum_name>], opts::Datetime ],
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

with_activity_types! { define_create_commands }

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
    let description = match ask_for_description(ctx, interaction, content).await? {
        Some(str) => str,
        None => return Ok(()),
    };
    debug!("Got event description: {:?}", description);

    // Create the event!
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

    let event_id = event.id;
    let content = format!("Your event **{}** has been created, Captain!", event_id);
    interaction
        .edit_embed_response(&ctx, &content, event.as_embed(), event.event_buttons())
        .await
        .context("Failed to edit response after creating event")?;
    event_manager
        .keep_embed_updated(
            event_id,
            EventEmbedMessage::EphemeralResponse(interaction.clone(), recv_time, content),
        )
        .await?;

    Ok(())
}
