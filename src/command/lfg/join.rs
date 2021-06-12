use std::str::FromStr;

use super::{edit_event_from_str, get_event_from_str, CommandOption, LeafCommand, EPHEMERAL_FLAG};
use crate::{
    command::OptionType,
    event::{EventEmbedMessage, EventManager, JoinKind},
    util::*,
};
use anyhow::{format_err, Context as _, Result};
use serde_json::Value;
use serenity::{
    async_trait,
    client::Context,
    model::{
        interactions::{
            ApplicationCommandInteractionDataOption,
            ApplicationCommandInteractionDataOptionValue as OptionValue, Interaction,
        },
        prelude::*,
    },
    utils::MessageBuilder,
};
use tracing::error;

define_command!(LfgJoin, "join", "Join an existing event", Leaf);

#[async_trait]
impl LeafCommand for LfgJoin {
    fn options(&self) -> Vec<CommandOption> {
        vec![
            CommandOption {
                name: "event_id",
                description: "Event ID",
                required: true,
                option_type: OptionType::String(vec![]),
            },
            CommandOption {
                name: "user",
                description: "User to add to event",
                required: false,
                option_type: OptionType::User,
            },
            CommandOption {
                name: "join_kind",
                description: "Do you want to join as confirmed, alternate, or maybe?",
                required: false,
                option_type: OptionType::String(vec![
                    ("Confirmed".to_owned(), "confirmed".to_owned()),
                    ("Confirmed Alt".to_owned(), "alt".to_owned()),
                    ("Maybe".to_owned(), "maybe".to_owned()),
                ]),
            },
        ]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let event_id = match options.get_value("event_id")? {
            Some(Value::String(v)) => Ok(v),
            Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
            None => Err(format_err!("Missing required event_id value")),
        }?;

        let command_user = interaction.get_user()?;
        let target_user = match options.get_resolved("user")? {
            None => Ok(command_user),
            Some(OptionValue::User(user, _)) => Ok(user),
            Some(v) => Err(format_err!("Unexpected resolved value type: {:?}", v)),
        }?;
        let kind = match options.get_value("join_kind")? {
            None => Ok(JoinKind::Confirmed),
            Some(Value::String(s)) => JoinKind::from_str(s),
            Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;

        join(
            ctx,
            interaction,
            event_id,
            command_user,
            Some(target_user),
            kind,
        )
        .await?;

        Ok(())
    }
}

pub async fn join(
    ctx: &Context,
    interaction: &Interaction,
    event_id: impl AsRef<str>,
    command_user: &User,
    target_user: Option<&User>,
    kind: JoinKind,
) -> Result<()> {
    let event_id = event_id.as_ref();
    let target_user = target_user.unwrap_or(command_user);

    let user_str = if command_user != target_user {
        target_user.mention().to_string()
    } else {
        "you".to_owned()
    };

    let mut type_map = ctx.data.write().await;
    let event_manager = type_map.get_mut::<EventManager>().unwrap();
    let edit_result = edit_event_from_str(&ctx, event_manager, &event_id, |event| {
        match event.join(&target_user, kind) {
            Ok(()) => format!(
                "Added {} to the {} event at {} as **{}**!",
                user_str,
                event.activity,
                event.formatted_datetime(),
                kind,
            ),
            Err(_) => "You're already in that event!".to_owned(),
        }
    })
    .await;
    let content = match edit_result {
        Ok(msg) => msg,
        Err(err) => {
            error!(
                "Failed to add {} to event {}: {:?}",
                target_user, event_id, err
            );
            "Sorry Captain, I seem to be having trouble adding you to that event...".to_owned()
        }
    };

    interaction
        .create_interaction_response(&ctx, |resp| {
            resp.interaction_response_data(|msg| msg.content(content).flags(EPHEMERAL_FLAG))
        })
        .await?;

    // If the command's issuer was adding someone else to an event, notify the added user over DM.
    if command_user != target_user {
        let type_map = type_map.downgrade();
        let event_manager = type_map.get::<EventManager>().unwrap();
        let event = get_event_from_str(event_manager, &event_id)
            .map_err(|_| format_err!("Unable to get just-joined event to send notification DM"))?;

        let content = MessageBuilder::new()
            .push("Pssssst, ")
            .mention(target_user)
            .push(", just letting you know that ")
            .mention(command_user)
            .push(" added you as ")
            .push_bold(kind)
            .push(" to this event! *People usually just do things without telling me too...*")
            .build();
        let dm = target_user
            .direct_message(&ctx, |msg| {
                msg.content(content)
                    .set_embed(event.as_embed())
                    .components(|c| {
                        *c = event.event_buttons();
                        c
                    })
            })
            .await
            .context("Error sending added user a DM notification")?;
        event
            .keep_embed_updated(EventEmbedMessage::Normal(dm.channel_id, dm.id))
            .await?;
    }

    Ok(())
}