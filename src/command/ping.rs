use super::{Command, CommandOption, CommandType, LeafCommand};
use anyhow::Result;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{Interaction, InteractionResponseType},
};

pub struct Ping;

impl Command for Ping {
    fn name(&self) -> &'static str {
        "ping"
    }

    fn description(&self) -> &'static str {
        "A ping command"
    }

    fn command_type(&self) -> CommandType {
        CommandType::Leaf(self as &dyn LeafCommand)
    }
}

#[async_trait]
impl LeafCommand for Ping {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()> {
        interaction
            .create_interaction_response(&ctx, |resp| {
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content("Pong!"))
            })
            .await?;
        Ok(())
    }
}
