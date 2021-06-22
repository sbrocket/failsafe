use super::{ask_for_description, edit_event_from_str, get_event_from_str, opts};
use crate::{
    command::{CommandHandler, OptionType},
    event::{Event, EventManager},
    time::parse_datetime,
    util::*,
};
use anyhow::{format_err, Context as _, Error, Result};
use chrono::DateTime;
use chrono_tz::Tz;
use serde_json::Value;
use serenity::{
    client::Context,
    model::interactions::{
        ApplicationCommandInteractionDataOption,
        ApplicationCommandInteractionDataOptionValue as OptionValue, Interaction,
    },
};
use std::convert::TryFrom;
use tracing::error;

define_command_group!(LfgEdit, "edit", "Edit an existing event", subcommands: [
    LfgEditDatetime,
    LfgEditDescription,
    LfgEditGroupSize,
    LfgEditRecur,
]);

macro_rules! define_edit_command {
    ($id:ident, $name:literal, $descr:expr, $handler:ident, options: $opts:tt $(,)?) => {
        paste::paste! {
            const [<$id:snake:upper>]: CommandHandler =
                |ctx: &Context, interaction: &Interaction, opts: &Vec<ApplicationCommandInteractionDataOption>| {
                    $handler(ctx, interaction, opts, $name)
                };
            define_leaf_command!($id, $name, $descr, [<$id:snake:upper>], options: $opts);
        }
    };
}

define_edit_command!(
    LfgEditDatetime,
    "datetime",
    "Edit an existing event's date and time",
    lfg_edit,
    options: [opts::EventId, opts::Datetime],
);

define_edit_command!(
    LfgEditDescription,
    "description",
    "Edit an existing event's description",
    lfg_edit,
    options: [opts::EventId],
);

define_command_option!(
    id: GroupSizeOpt,
    name: "group-size",
    description: "Number of guardians per group",
    required: true,
    option_type: OptionType::Integer(&[("1", 1), ("2", 2), ("3", 3), ("4", 4), ("5", 5), ("6", 6), ("12", 12)]),
);
define_edit_command!(
    LfgEditGroupSize,
    "group-size",
    "Edit an existing event's group size",
    lfg_edit,
    options: [opts::EventId, GroupSizeOpt],
);

define_command_option!(
    id: RecurOpt,
    name: "recur",
    description: "Enable weekly recurrence for this event?",
    required: true,
    option_type: OptionType::Boolean,
);
define_edit_command!(
    LfgEditRecur,
    "recur",
    "Enable/disable weekly recurrence for an existing event",
    lfg_edit,
    options: [opts::EventId, RecurOpt],
);

enum EditType {
    Datetime(Result<DateTime<Tz>, (String, Error)>),
    // Description is unique in that the value doesn't come from an option, but from a separate
    // query & response with the user.
    Description(Option<String>),
    GroupSize(u8),
    Recur(bool),
}

impl EditType {
    pub fn from_option(
        options: &Vec<ApplicationCommandInteractionDataOption>,
        option_name: &str,
    ) -> Result<Self> {
        if option_name == "description" {
            return Ok(EditType::Description(None));
        }

        let value = match options.get_resolved(option_name)? {
            Some(v) => Ok(v),
            None => Err(format_err!("Missing required {} value", option_name)),
        }?;
        match option_name {
            "datetime" => match value {
                OptionValue::String(dt_str) => {
                    Ok(EditType::Datetime(match parse_datetime(&dt_str) {
                        Ok(datetime) => Ok(datetime),
                        Err(err) => Err((
                            format!("Sorry Captain, I don't understand what '{}' means", dt_str),
                            err.context(format!("Unable to parse provided datetime: {:?}", dt_str)),
                        )),
                    }))
                }
                _ => Err(format_err!("Wrong {} value type", option_name)),
            },
            "group-size" => match value {
                OptionValue::Integer(size) => Ok(EditType::GroupSize(
                    u8::try_from(*size).context("Group size too large")?,
                )),
                _ => Err(format_err!("Wrong {} value type", option_name)),
            },
            "recur" => match value {
                OptionValue::Boolean(recur) => Ok(EditType::Recur(*recur)),
                _ => Err(format_err!("Wrong {} value type", option_name)),
            },
            _ => unreachable!("Unknown edit option name"),
        }
    }

    pub fn apply_edit(self, event: &mut Event) {
        match self {
            EditType::Datetime(Ok(datetime)) => event.datetime = datetime,
            EditType::Description(Some(descr)) => event.description = descr,
            EditType::GroupSize(size) => event.group_size = size,
            EditType::Recur(recur) => event.recur = recur,
            EditType::Datetime(Err(_)) => unreachable!("Tried to apply invalid datetime"),
            EditType::Description(None) => unreachable!("Tried to apply empty description"),
        }
    }
}

#[command_attr::hook]
async fn lfg_edit(
    ctx: &Context,
    interaction: &Interaction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
    option_name: &str,
) -> Result<()> {
    let event_id = match options.get_value("event_id")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required event_id value")),
    }?;

    let member = interaction
        .member
        .as_ref()
        .ok_or_else(|| format_err!("Guild interaction missing member data"))?;
    let perms = member
        .permissions
        .as_ref()
        .ok_or_else(|| format_err!("Interaction missing member permissions"))?;

    // Check permissions upfront, before potentially asking for a new description, but make sure to
    // drop the lock guard before asking.
    {
        let type_map = ctx.data.read().await;
        let event_manager = type_map.get::<EventManager>().unwrap();
        let err_msg = match get_event_from_str(event_manager, &event_id) {
            Ok(event) => {
                // First we need to check that the member issuing the command is either the creator or an admin.
                if member.user.id == event.creator.id || perms.administrator() {
                    None
                } else {
                    Some("Only the event creator or an admin can edit an event".to_owned())
                }
            }
            Err(msg) => Some(msg),
        };
        if let Some(err_msg) = err_msg {
            interaction.create_response(ctx, err_msg, true).await?;
            return Ok(());
        }
    }

    let mut edit = EditType::from_option(options, option_name)?;
    let mut response_created = false;
    match edit {
        EditType::Datetime(Err((content, err))) => {
            interaction.create_response(&ctx, content, true).await?;
            return Err(err);
        }
        EditType::Description(None) => {
            // Ask the user for a new event description.
            let content = "What's the new description? *And try to get it right this time...*";
            match ask_for_description(ctx, interaction, content).await? {
                Some(str) => edit = EditType::Description(Some(str)),
                None => return Ok(()),
            };
            response_created = true;
        }
        _ => {}
    }

    let mut type_map = ctx.data.write().await;
    let event_manager = type_map.get_mut::<EventManager>().unwrap();
    let edit_result = edit_event_from_str(event_manager, &event_id, |event| {
        edit.apply_edit(event);
        format!("Event **{}** updated!", event.id)
    })
    .await;
    let content = match edit_result {
        Ok(content) => content,
        Err(err) => {
            error!("Failed to edit event {}: {:?}", event_id, err);
            "Sorry Captain, I seem to be having trouble editing that event...".to_owned()
        }
    };
    if response_created {
        interaction.edit_response(ctx, content).await?;
    } else {
        interaction.create_response(ctx, content, true).await?;
    }

    Ok(())
}