use super::{edit_event_from_str, opts};
use crate::util::*;
use anyhow::{format_err, Result};
use serenity::{
    client::Context,
    model::{
        interactions::application_command::{
            ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
        },
        prelude::*,
    },
};
use tracing::error;

define_leaf_command!(
    LfgLeave,
    "leave",
    "Leave an existing event",
    lfg_leave,
    options: [opts::EventId],
);

#[command_attr::hook]
async fn lfg_leave(
    ctx: &Context,
    interaction: &ApplicationCommandInteraction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
) -> Result<()> {
    let member = interaction
        .member
        .as_ref()
        .ok_or_else(|| format_err!("Interaction not in a guild"))?;
    let event_id = match options.get_resolved("event_id")? {
        Some(OptionValue::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required event_id value")),
    }?;

    leave(ctx, interaction, event_id, member).await
}

pub async fn leave(
    ctx: &Context,
    interaction: &impl InteractionExt,
    event_id: impl AsRef<str>,
    member: &Member,
) -> Result<()> {
    let event_manager = ctx.get_event_manager(interaction).await?;
    let edit_result = edit_event_from_str(&event_manager, &event_id, |event| {
        match event.leave(member) {
            Ok(()) => format!(
                "Removed you from the {} event at {}",
                event.activity,
                event.timestamp()
            ),
            Err(_) => {
                "*Hey, you're not even in that event... did you think I'd forget?*".to_owned()
            }
        }
    })
    .await;

    match (edit_result, interaction.kind()) {
        (Err(err), _) => {
            error!("Failed to edit event: {:?}", err);
            let content =
                "Sorry Captain, I seem to be having trouble removing you from that event...";
            interaction.create_response(&ctx, content, true).await?;
        }
        (Ok(content), InteractionType::ApplicationCommand) => {
            interaction.create_response(&ctx, content, true).await?;
        }
        (Ok(_), InteractionType::MessageComponent) => {
            // Just ACK component interactions.
            interaction.create_ack_response(&ctx).await?;
        }
        (_, kind) => error!("Unexpected interaction kind {:?}", kind),
    }

    Ok(())
}
