use super::{CommandOption, LeafCommand};
use anyhow::Result;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{Interaction, InteractionResponseType},
};

define_command!(Ping, "ping", "A ping command", Leaf);

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
