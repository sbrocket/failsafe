use super::{CommandOption, LeafCommand};
use crate::util::InteractionExt;
use anyhow::Result;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{
        ApplicationCommandInteractionDataOption, Interaction, InteractionResponseType,
    },
    utils::MessageBuilder,
};

define_command!(Ping, "ping", "A ping command", Leaf);

#[async_trait]
impl LeafCommand for Ping {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        _: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let user = interaction.get_user()?;
        interaction
            .create_interaction_response(&ctx, |resp| {
                let message = MessageBuilder::new()
                    .push("Pong! Hi ")
                    .mention(user)
                    .build();
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content(message))
            })
            .await?;
        Ok(())
    }
}
