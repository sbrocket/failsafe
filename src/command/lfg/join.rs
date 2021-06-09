use super::{edit_event_from_str, CommandOption, LeafCommand, EPHEMERAL_FLAG};
use crate::{command::OptionType, event::EventManager, util::*};
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{ApplicationCommandInteractionDataOption, Interaction},
};
use tracing::error;

define_command!(LfgJoin, "join", "Join an existing event", Leaf);

#[async_trait]
impl LeafCommand for LfgJoin {
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
            Value::String(v) => Ok(v),
            v => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;

        let mut type_map = ctx.data.write().await;
        let event_manager = type_map.get_mut::<EventManager>().unwrap();
        let edit_result = edit_event_from_str(&ctx, event_manager, &event_id, |event| match event
            .join(&user)
        {
            Ok(()) => format!(
                "Added you to the {} event at {}!",
                event.activity,
                event.formatted_datetime()
            ),
            Err(_) => "You're already in that event!".to_owned(),
        })
        .await;
        let content = match edit_result {
            Ok(msg) => msg,
            Err(err) => {
                error!("Failed to edit event: {:?}", err);
                "Sorry Captain, I seem to be having trouble adding you to that event...".to_owned()
            }
        };

        interaction
            .create_interaction_response(&ctx, |resp| {
                resp.interaction_response_data(|msg| msg.content(content).flags(EPHEMERAL_FLAG))
            })
            .await?;
        Ok(())
    }
}
