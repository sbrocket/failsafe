use crate::{
    event::{Event, EventId, EventManager, JoinKind},
    util::*,
};
use anyhow::{format_err, Result};
use serenity::{
    client::Context,
    model::interactions::{
        application_command::ApplicationCommandInteraction,
        message_component::MessageComponentInteraction,
    },
    utils::MessageBuilder,
};
use std::time::Duration;
use std::{str::FromStr, sync::Arc};
use tokio::time::sleep;
use tracing::{debug, error};

mod opts;

mod create;
mod delete;
mod edit;
mod join;
mod kick;
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
        edit::LfgEdit,
        join::LfgJoin,
        kick::LfgKick,
        leave::LfgLeave,
        show::LfgShow,
    ]
);

/// Returns the matching Event or else an error message to use in the interaction reponse.
async fn get_event_from_str(
    event_manager: &EventManager,
    id_str: impl AsRef<str>,
) -> Result<Arc<Event>, String> {
    let id_str = id_str.as_ref();
    match EventId::from_str(&id_str) {
        Ok(event_id) => match event_manager.get_event(&event_id).await {
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
    event_manager: &EventManager,
    id_str: impl AsRef<str>,
    edit_fn: impl FnOnce(&mut Event) -> String,
) -> Result<String> {
    let id_str = id_str.as_ref();
    match EventId::from_str(&id_str) {
        Ok(event_id) => {
            event_manager
                .edit_event(&event_id, |event| match event {
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

// Nudge the user for a description after this timeout.
const LFG_DESCRIPTION_NUDGE_SEC: u64 = 60;
// Overall description timeout.
const LFG_DESCRIPTION_TIMEOUT_SEC: u64 = 3 * 60;

// Note that this creates the original interaction response, so subsequent logic must take care to
// edit that response or create followups, rather than trying to create it again (which will fail).
pub async fn ask_for_description(
    ctx: &Context,
    interaction: &ApplicationCommandInteraction,
    query_content: impl ToString,
) -> Result<Option<String>> {
    let user = &interaction.user;

    interaction
        .create_response(&ctx, query_content.to_string(), true)
        .await?;

    let mut reply_fut = user
        .await_reply(&ctx)
        .timeout(Duration::from_secs(LFG_DESCRIPTION_TIMEOUT_SEC));
    let nudge_sleep = sleep(Duration::from_secs(LFG_DESCRIPTION_NUDGE_SEC));
    tokio::pin!(nudge_sleep);

    let mut nudge_followup = None;
    loop {
        tokio::select! {
            // Nudge the user for a description in case it was unclear what to do.
            _ = &mut nudge_sleep, if !nudge_sleep.is_elapsed() => {
                let content = MessageBuilder::new()
                        .push("*Pssst, ")
                        .mention(user)
                        .push(", still there?* Just send a message with the event description in this channel.")
                        .build();
                nudge_followup.insert(interaction.create_followup(&ctx, content, true).await?);
            }

            // Wait for the user to reply with the description.
            reply = &mut reply_fut => {
                if let Some(reply) = reply {
                    // Immediately delete the user's (public) message since the rest of the bot
                    // interaction is ephemeral.
                    if let Err(err) = reply.delete(&ctx).await {
                        // If the user is doing this in an event channel, our delete will race with
                        // ChannelUpdater's delete, so ignore "Unknown Message" errors.
                        if !err.is_discord_json_error(DiscordJsonErrorCode::UnknownMessage) {
                            Err(err)?;
                        }
                    }

                    if let Some(followup) = nudge_followup {
                        // We can't just delete the followup, since it's ephemeral, so just edit the
                        // message so the channel state doesn't look confusing.
                        let edit_fut = interaction.edit_followup_message(&ctx, followup.id, |msg| {
                            msg.content("*Good job human, you followed basic instructions!*")
                        });
                        if let Err(err) = edit_fut.await {
                            error!("Failed to edit nudge followup message: {:?}", err);
                        }
                    }

                    return Ok(Some(reply.content.clone()));
                } else {
                    // Timed out waiting for the description, send a followup message so that the
                    // user can see the description request still and so the mention works.
                    let content = MessageBuilder::new()
                            .push("**Yoohoo, ")
                            .mention(user)
                            .push("!** Are the Fallen dismantling *your* brain now? *Whatever, just ask me again...not like I'm going anywhere...*")
                            .build();
                    interaction.create_followup(&ctx, content, true).await?;
                    return Ok(None);
                }
            }
        };
    }
}

pub async fn handle_component_interaction(
    ctx: &Context,
    interaction: &MessageComponentInteraction,
) -> Result<()> {
    let custom_id = &interaction.data.custom_id;
    debug!("handling component interaction, id '{}'", custom_id);

    let member = interaction
        .member
        .as_ref()
        .ok_or_else(|| format_err!("Interaction not in a guild"))?;
    let (action, event_id) = custom_id
        .split_once(":")
        .ok_or_else(|| format_err!("Received unexpected component custom_id: {}", custom_id))?;

    match action {
        "join" => {
            join::join(
                ctx,
                interaction,
                event_id,
                member,
                None,
                JoinKind::Confirmed,
            )
            .await
        }
        "alt" => {
            join::join(
                ctx,
                interaction,
                event_id,
                member,
                None,
                JoinKind::Alternate,
            )
            .await
        }
        "maybe" => join::join(ctx, interaction, event_id, member, None, JoinKind::Maybe).await,
        "leave" => leave::leave(ctx, interaction, event_id, member).await,
        _ => Err(format_err!(
            "Received unexpected component custom_id: {}",
            custom_id
        )),
    }
}
