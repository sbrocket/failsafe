use super::{edit_event_from_str, CommandOption, LeafCommand, EPHEMERAL_FLAG};
use crate::{command::OptionType, event::EventManager, util::*};
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    async_trait,
    client::Context,
    model::{
        interactions::{ApplicationCommandInteractionDataOption, Interaction},
        prelude::*,
    },
};
use tracing::error;

define_command!(LfgLeave, "leave", "Leave an existing event", Leaf);

#[async_trait]
impl LeafCommand for LfgLeave {
    fn options(&self) -> Vec<CommandOption> {
        vec![CommandOption {
            name: "event_id",
            description: "Event ID",
            required: true,
            option_type: OptionType::String(vec![]),
        }]
    }

    async fn handle_interaction(
        &self,
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
}

pub async fn leave(
    ctx: &Context,
    interaction: &Interaction,
    event_id: impl AsRef<str>,
    user: &User,
) -> Result<()> {
    let mut type_map = ctx.data.write().await;
    let event_manager = type_map.get_mut::<EventManager>().unwrap();
    let edit_result = edit_event_from_str(&ctx, event_manager, &event_id, |event| {
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
    let content = match edit_result {
        Ok(msg) => msg,
        Err(err) => {
            error!("Failed to edit event: {:?}", err);
            "Sorry Captain, I seem to be having trouble removing you from that event...".to_owned()
        }
    };

    interaction
        .create_interaction_response(&ctx, |resp| {
            resp.interaction_response_data(|msg| msg.content(content).flags(EPHEMERAL_FLAG))
        })
        .await?;
    Ok(())
}
