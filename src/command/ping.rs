use super::{CommandOption, LeafCommand};
use anyhow::{format_err, Result};
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{Interaction, InteractionResponseType},
    utils::MessageBuilder,
};

define_command!(Ping, "ping", "A ping command", Leaf);

#[async_trait]
impl LeafCommand for Ping {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()> {
        let user = interaction
            .member
            .as_ref()
            .map(|m| m.user.id)
            .ok_or(format_err!("Interaction from nowhere?! {:?}", &interaction))?;
        interaction
            .create_interaction_response(&ctx, |resp| {
                let message = MessageBuilder::new()
                    .push("Pong! Hi ")
                    .mention(&user)
                    .build();
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content(message))
            })
            .await?;
        Ok(())
    }
}
