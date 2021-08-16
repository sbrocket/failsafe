use super::{ask_for_description, opts};
use crate::{
    activity::{Activity, ActivityType},
    command::OptionType,
    event::EventEmbedMessage,
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
    model::interactions::application_command::{
        ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
    },
};
use tracing::{debug, error, info, warn};

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
    interaction: &ApplicationCommandInteraction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
) -> Result<()> {
    let recv_time = Utc::now();

    let member = interaction
        .member
        .as_ref()
        .ok_or_else(|| format_err!("Interaction not in a guild"))?;
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
            warn!(
                "Unable to parse provided datetime ('{}'): {:?}",
                datetime, err
            );
            return Ok(());
        }
    };
    debug!("Parsed datetime: {}", datetime);

    // Check that the datetime isn't in the past.
    if datetime <= Utc::now() {
        let content = "Sorry Captain, you can't create events in the past... *I'm an AI, not a time-traveling Vex*";
        interaction.create_response(&ctx, content, true).await?;
        info!("Rejected new event datetime in the past ({})", datetime);
        return Ok(());
    }

    // Ask for the event description in the main response.
    let content = format!(
        "What's so special about this... *uhhh, \"{}\"?*  ...event?\n\
                    **Give me a description.** *(In simple terms, like for a Guardi...errr, nevermind...)*",
        activity
    );
    let description = match ask_for_description(ctx, interaction, content).await? {
        Some(str) => str,
        None => return Ok(()),
    };
    debug!("Got event description: {:?}", description);

    // Create the event!
    let event_manager = ctx.get_event_manager(interaction).await?;
    let event = match event_manager
        .create_event(member, activity, datetime, description)
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
