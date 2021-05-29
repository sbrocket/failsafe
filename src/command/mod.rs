use anyhow::{format_err, Result};
use lazy_static::lazy_static;
use serenity::{
    async_trait,
    builder::CreateApplicationCommand,
    client::Context,
    model::{
        id::GuildId,
        interactions::{ApplicationCommand, Interaction},
    },
};
use std::collections::BTreeMap;

mod ping;

/// Trait for each individual slash command to implement.
#[async_trait]
trait Command: Send + Sync {
    fn name(&self) -> &'static str;

    fn create_command<'a>(
        &self,
        builder: &'a mut CreateApplicationCommand,
    ) -> &'a mut CreateApplicationCommand;

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()>;
}

lazy_static! {
    /// List of all known commands; add new commands here as they're created.
    static ref COMMANDS: BTreeMap<&'static str, Box<dyn Command>> = {
        vec![ping::Ping]
            .into_iter()
            .map(|c| (c.name(), Box::new(c) as Box<dyn Command>))
            .collect()
    };
}

/// Manages the bot's slash commands, handling creating the commands on startup and dispatching
/// interactions as they're received.
pub struct CommandManager {
    _commands: Vec<ApplicationCommand>,
}

impl CommandManager {
    /// Create a new CommandManager, registering all known commands with Discord.
    /// TODO: For now this creates guild commands (under the given GuildId) instead of global
    /// commands since the former update immediately but the latter are cached for an hour.
    pub async fn new(ctx: &Context, guild: &GuildId) -> Result<CommandManager> {
        // There's a rate limit on creating commands (200 per day per guild) that could get hit if
        // restarting the bot frequently, unclear if replacing/updating commands counts against that
        // limit.
        let commands = guild
            .set_application_commands(&ctx, |mut commands| {
                for command in COMMANDS.values() {
                    commands = commands
                        .create_application_command(|builder| command.create_command(builder))
                }
                commands
            })
            .await?;
        Ok(Self {
            _commands: commands,
        })
    }

    /// Dispatch the given interaction to the appropriate command.
    pub async fn dispatch_interaction(
        &self,
        ctx: &Context,
        interaction: Interaction,
    ) -> Result<()> {
        let command = if let Some(data) = &interaction.data {
            COMMANDS
                .get(data.name.as_str())
                .ok_or_else(|| format_err!("Unknown interaction command name '{}'", &data.name))?
        } else {
            return Err(format_err!("Interaction has no data: {:?}", interaction));
        };
        command.handle_interaction(ctx, interaction).await
    }
}
