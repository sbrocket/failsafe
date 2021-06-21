use crate::{
    event::{Event, EventHandle, EventId, EventManager, JoinKind},
    util::*,
};
use anyhow::{format_err, Result};
use serenity::{
    client::Context,
    model::interactions::{Interaction, MessageComponent},
};
use std::str::FromStr;
use tracing::debug;

mod opts;

mod create;
mod delete;
mod join;
mod leave;
mod show;

// TODO: Reorder these so that join & leave appear first when typing `/lfg` in Discord. Need to
// delete and recreate.
define_command_group!(
    Lfg,
    "lfg",
    "Create and interact with scheduled events",
    subcommands: [
        create::LfgCreate,
        delete::LfgDelete,
        join::LfgJoin,
        leave::LfgLeave,
        show::LfgShow,
    ]
);

/// Returns the matching Event or else an error message to use in the interaction reponse.
fn get_event_from_str(
    event_manager: &EventManager,
    id_str: impl AsRef<str>,
) -> Result<EventHandle<'_>, String> {
    let id_str = id_str.as_ref();
    match EventId::from_str(&id_str) {
        Ok(event_id) => match event_manager.get_event(&event_id) {
            Some(event) => Ok(event),
            None => Err(format!("I couldn't find an event with ID '{}'", event_id)),
        },
        Err(_) => {
            Err("That's not a valid event ID, Captain. They look like this: `dsc123`".to_owned())
        }
    }
}

/// Runs the given closure on the matching Event, returning the message it generates or else an
/// error message to use in the interaction reponse.
async fn edit_event_from_str(
    ctx: &Context,
    event_manager: &mut EventManager,
    id_str: impl AsRef<str>,
    edit_fn: impl FnOnce(&mut Event) -> String,
) -> Result<String> {
    let id_str = id_str.as_ref();
    match EventId::from_str(&id_str) {
        Ok(event_id) => {
            event_manager
                .edit_event(ctx, &event_id, |event| match event {
                    Some(event) => edit_fn(event),
                    None => format!("I couldn't find an event with ID '{}'", event_id),
                })
                .await
        }
        Err(_) => {
            Ok("That's not a valid event ID, Captain. They look like this: `dsc123`".to_owned())
        }
    }
}

pub async fn handle_component_interaction(
    ctx: &Context,
    interaction: &Interaction,
    data: &MessageComponent,
) -> Result<()> {
    debug!("handling component interaction, id '{}'", data.custom_id);

    let user = interaction.get_user()?;

    let custom_id = &data.custom_id;
    let (action, event_id) = custom_id
        .split_once(":")
        .ok_or_else(|| format_err!("Received unexpected component custom_id: {}", custom_id))?;

    match action {
        "join" => join::join(ctx, interaction, event_id, user, None, JoinKind::Confirmed).await,
        "alt" => join::join(ctx, interaction, event_id, user, None, JoinKind::Alternate).await,
        "maybe" => join::join(ctx, interaction, event_id, user, None, JoinKind::Maybe).await,
        "leave" => leave::leave(ctx, interaction, event_id, user).await,
        _ => Err(format_err!(
            "Received unexpected component custom_id: {}",
            custom_id
        )),
    }
}
