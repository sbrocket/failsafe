use super::{edit_event_from_str, opts};
use crate::{command::OptionType, util::*};
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    client::Context,
    model::{
        interactions::application_command::{
            ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
            ApplicationCommandInteractionDataOptionValue as OptionValue,
        },
        prelude::*,
    },
};
use tracing::error;

define_command_option!(
    id: UserOpt,
    name: "user",
    description: "User to remove from event",
    required: true,
    option_type: OptionType::User,
);

define_leaf_command!(
    LfgKick,
    "kick",
    "Remove a user from an existing event",
    lfg_kick,
    options: [
        opts::EventId,
        UserOpt,
    ],
);

#[command_attr::hook]
async fn lfg_kick(
    ctx: &Context,
    interaction: &ApplicationCommandInteraction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
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

    // First we need to check that the member issuing the command is an admin.
    // TODO: Should event creators be able to kick? Maybe if the user is notified?
    if !perms.administrator() {
        let content = "Only admins can kick people out of events";
        interaction.create_response(ctx, content, true).await?;
        return Ok(());
    }

    let target_member = match options.get_resolved("user")? {
        Some(OptionValue::User(user, Some(member))) => Ok((user, member)),
        Some(OptionValue::User(..)) => Err(format_err!(
            "Missing PartialMember, interaction not in a guild"
        )),
        Some(v) => Err(format_err!("Unexpected resolved value type: {:?}", v)),
        None => Err(format_err!("Missing required user value")),
    }?;

    let user_mention = target_member.user().mention();
    let event_manager = ctx.get_event_manager(interaction).await?;
    let edit_result = edit_event_from_str(&event_manager, &event_id, |event| {
        match event.leave(&target_member) {
            Ok(()) => format!(
                "Removed {} from the {} event at {}",
                user_mention,
                event.activity,
                event.formatted_datetime()
            ),
            Err(_) => format!(
                "*Errr, Captain, you can't kick {} because they aren't in that event...*",
                user_mention
            ),
        }
    })
    .await;

    let content = match edit_result {
        Ok(content) => content,
        Err(err) => {
            error!("Failed to kick {} from event: {:?}", user_mention, err);
            format!(
                "Sorry Captain, I seem to be having trouble removing {} from that event...",
                user_mention
            )
        }
    };
    interaction.create_response(&ctx, content, true).await?;

    Ok(())
}
