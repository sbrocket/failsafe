use super::{get_event_from_str, opts};
use crate::{event::EventEmbedMessage, util::*};
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    client::Context,
    model::interactions::application_command::{
        ApplicationCommandInteraction, ApplicationCommandInteractionDataOption,
    },
};

define_leaf_command!(
    LfgShow,
    "show",
    "Display an existing event",
    lfg_show,
    options: [opts::EventId],
);

#[command_attr::hook]
async fn lfg_show(
    ctx: &Context,
    interaction: &ApplicationCommandInteraction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
) -> Result<()> {
    let event_id = match options.get_value("event_id")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required event_id value")),
    }?;

    let event_manager = ctx.get_event_manager(interaction).await?;
    match get_event_from_str(&event_manager, &event_id).await {
        Ok(event) => {
            interaction
                .create_embed_response(&ctx, "", event.as_embed(), event.event_buttons(), false)
                .await?;

            let msg = interaction.get_interaction_response(&ctx).await?;
            event_manager
                .keep_embed_updated(event.id, EventEmbedMessage::Normal(msg.channel_id, msg.id))
                .await?;
        }
        Err(content) => {
            interaction.create_response(&ctx, content, true).await?;
        }
    }

    Ok(())
}
