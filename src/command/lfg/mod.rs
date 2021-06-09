use super::{CommandOption, LeafCommand};
use crate::event::{Event, EventHandle, EventId, EventManager};
use anyhow::Result;
use serenity::{
    client::Context, model::interactions::InteractionApplicationCommandCallbackDataFlags,
};
use std::str::FromStr;

mod create;
mod join;
mod leave;
mod show;

const EPHEMERAL_FLAG: InteractionApplicationCommandCallbackDataFlags =
    InteractionApplicationCommandCallbackDataFlags::EPHEMERAL;

define_command!(Lfg, "lfg", "Create and interact with scheduled events",
                Subcommands: [join::LfgJoin, leave::LfgLeave, show::LfgShow, create::LfgCreate]);

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
