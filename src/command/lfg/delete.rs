use super::{get_event_from_str, opts};
use crate::{event::EventManager, util::*};
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    client::Context,
    model::interactions::{ApplicationCommandInteractionDataOption, Interaction},
};
use tracing::error;

define_leaf_command!(
    LfgDelete,
    "delete",
    "Delete an existing event (creator or admin only)",
    lfg_delete,
    options: [opts::EventId],
);

#[command_attr::hook]
async fn lfg_delete(
    ctx: &Context,
    interaction: &Interaction,
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

    let mut type_map = ctx.data.write().await;
    let event_manager = type_map.get_mut::<EventManager>().unwrap();
    let check_result = match get_event_from_str(event_manager, &event_id) {
        Ok(event) => {
            // First we need to check that the member issuing the command is either the creator or an admin.
            if member.user.id == event.creator.id || perms.administrator() {
                Ok(event.id)
            } else {
                Err("Only the event creator or an admin can delete an event".to_owned())
            }
        }
        Err(err) => Err(err),
    };

    let content = match check_result {
        Ok(event_id) => {
            // Permission check passed, delete the event.
            if let Err(err) = event_manager.delete_event(&ctx, &event_id).await {
                error!("Failed to delete event {}: {}", event_id, err);
                "Sorry Captain, I seem to be having trouble deleting that event...".to_owned()
            } else {
                format!(
                    "Event {} deleted! *Hope that wasn't important...*",
                    event_id
                )
            }
        }
        Err(str) => str,
    };
    interaction.create_response(&ctx, content, true).await?;

    Ok(())
}
