use super::{ask_for_description, opts};
use crate::{
    activity::{Activity, ActivityType},
    command::OptionType,
    event::EventEmbedMessage,
    util::*,
};
use anyhow::{format_err, Context as _, Result};
use lazy_static::lazy_static;
use paste::paste;
use serde_json::Value;
use serenity::{
    client::Context,
    model::interactions::application_command::{
        ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
    },
};
use tracing::{debug, error};

// Macro to create the individual leaf commands for each ActivityType. An "activity" option is added
// to the command depending on whether the ActivityType has a single Activity or not.
macro_rules! define_create_command {
    ($enum_name:ident: ($name:literal, $cmd:literal)) => {
        paste! {
            define_leaf_command!(
                [<LfgCreate $enum_name>],
                $cmd,
                concat!("Create a new ", $name, " event"),
                lfg_create,
                options: [ [<ActivityOpt $enum_name>], opts::time::Datetime ],
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
        }
    };
    ($enum_name:ident: ($name:literal, $cmd:literal, Single)) => {
        paste! {
            define_leaf_command!(
                [<LfgCreate $enum_name>],
                $cmd,
                concat!("Create a new ", $name, " event"),
                [<lfg_create_ $enum_name:lower>],
                options: [ opts::time::Datetime ],
            );

            // For ActivityTypes with a single Activity, create a command handler that just passes
            // that single Activity along (as opposed to the standard lfg_create that parses the
            // "activity" option).
            #[command_attr::hook]
            async fn [<lfg_create_ $enum_name:lower>](
                ctx: &Context,
                interaction: &ApplicationCommandInteraction,
                options: &Vec<ApplicationCommandInteractionDataOption>,
            ) -> Result<()> {
                create(ctx, interaction, options, Activity::$enum_name).await
            }
        }
    };
}

macro_rules! define_create_commands {
    ($($enum_name:ident: $props:tt),+ $(,)?) => {
        paste! {
            define_command_group!(LfgCreate, "create", "Create a new event",
                            subcommands: [
                                $(
                                    [<LfgCreate $enum_name>]
                                ),+
                            ]);

            $(
                define_create_command!($enum_name: $props);
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
    let activity = match options.get_value("activity")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required activity value")),
    }?;
    let activity = Activity::activity_with_id_prefix(activity)
        .ok_or_else(|| format_err!("Unexpected activity value: {:?}", activity))?;

    create(ctx, interaction, options, activity).await
}

async fn create(
    ctx: &Context,
    interaction: &ApplicationCommandInteraction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
    activity: Activity,
) -> Result<()> {
    let member = interaction
        .member
        .as_ref()
        .ok_or_else(|| format_err!("Interaction not in a guild"))?;

    // Parse the datetime options.
    let datetime = match opts::time::parse_datetime_options(options) {
        Ok(datetime) => datetime,
        Err(err) => {
            let content = match err.user_error() {
                Some(descr) => descr,
                None => {
                    error!("Error parsing datetime options: {:?}", err);
                    "Sorry Captain, something went wrong with my internal chronometers..."
                        .to_owned()
                }
            };
            interaction.create_response(&ctx, content, true).await?;
            return Ok(());
        }
    };

    // TODO: Check that the datetime isn't far in the future (>6 months?), likely means misstaken
    // user input led to bad assumed year.

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
            EventEmbedMessage::EphemeralResponse(interaction.clone(), content),
        )
        .await?;

    Ok(())
}
