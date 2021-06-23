use super::{edit_event_from_str, opts};
use crate::util::*;
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    client::Context,
    model::{
        interactions::{ApplicationCommandInteractionDataOption, Interaction},
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
    interaction: &Interaction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
) -> Result<()> {
    let user = interaction.get_user()?;
    let event_id = match options.get_value("event_id")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required event_id value")),
    }?;

    leave(ctx, interaction, event_id, user).await
}

pub async fn leave(
    ctx: &Context,
    interaction: &Interaction,
    event_id: impl AsRef<str>,
    user: &User,
) -> Result<()> {
    let event_manager = ctx.get_event_manager().await;
    let edit_result = edit_event_from_str(&event_manager, &event_id, |event| {
        match event.leave(&user) {
            Ok(()) => format!(
                "Removed you from the {} event at {}",
                event.activity,
                event.formatted_datetime()
            ),
            Err(_) => {
                "*Hey, you're not even in that event... did you think I'd forget?*".to_owned()
            }
        }
    })
    .await;

    match (edit_result, interaction.kind) {
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
