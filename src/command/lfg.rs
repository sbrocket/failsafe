use super::{CommandOption, LeafCommand};
use crate::util::InteractionExt;
use anyhow::Result;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{Interaction, InteractionResponseType},
    utils::MessageBuilder,
};

define_command!(Lfg, "lfg", "Command for interacting with scheduled events", Subcommands: [LfgJoin, LfgCreate]);
define_command!(LfgJoin, "join", "Join an existing event", Leaf);
define_command!(LfgCreate, "create", "Create a new event", Leaf);

#[async_trait]
impl LeafCommand for LfgJoin {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()> {
        let user = interaction.get_user_id()?;
        interaction
            .create_interaction_response(&ctx, |resp| {
                let message = MessageBuilder::new()
                    .push("Hi ")
                    .mention(&user)
                    .push("! There's no events to join yet...because I can't create them yet...")
                    .build();
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content(message))
            })
            .await?;
        Ok(())
    }
}

#[async_trait]
impl LeafCommand for LfgCreate {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()> {
        let user = interaction.get_user_id()?;
        interaction
            .create_interaction_response(&ctx, |resp| {
                let message = MessageBuilder::new()
                    .push("Hi ")
                    .mention(&user)
                    .push("! I'll be able to create events soon...")
                    .build();
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content(message))
            })
            .await?;
        Ok(())
    }
}
