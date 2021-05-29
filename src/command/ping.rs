use super::Command;
use anyhow::Result;
use serenity::{
    async_trait,
    builder::CreateApplicationCommand,
    client::Context,
    model::interactions::{Interaction, InteractionResponseType},
};

pub struct Ping;

#[async_trait]
impl Command for Ping {
    fn name(&self) -> &'static str {
        "ping"
    }

    fn create_command<'a>(
        &self,
        builder: &'a mut CreateApplicationCommand,
    ) -> &'a mut CreateApplicationCommand {
        builder.name(self.name()).description("A ping command")
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
