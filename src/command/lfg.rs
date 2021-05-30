use super::{CommandOption, LeafCommand};
use anyhow::Result;
use serenity::{async_trait, client::Context, model::interactions::Interaction};

define_command!(Lfg, "lfg", "Command for interacting with scheduled events", Subcommands: [LfgJoin, LfgCreate]);
define_command!(LfgJoin, "join", "Join an existing event", Leaf);
define_command!(LfgCreate, "create", "Create a new event", Leaf);

#[async_trait]
impl LeafCommand for LfgJoin {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()> {
        todo!()
    }
}

#[async_trait]
impl LeafCommand for LfgCreate {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()> {
        todo!()
    }
}
