use anyhow::{format_err, Result};
use lazy_static::lazy_static;
use serenity::{
    async_trait,
    builder::{CreateApplicationCommand, CreateApplicationCommandOption},
    client::Context,
    model::{
        id::GuildId,
        interactions::{ApplicationCommand, ApplicationCommandOptionType, Interaction},
    },
};
use std::collections::BTreeMap;

mod ping;

/// Trait for each individual slash command to implement.
// TODO: Does this really need to be a trait?
#[async_trait]
trait Command: Send + Sync {
    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    /// Commands can have [0,25] options.
    fn options(&self) -> Vec<CommandOption>;

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()>;
}

/// Definition of a single command option.
struct CommandOption {
    name: String,
    description: String,
    required: bool,
    type_choices: OptionType,
}

/// The type for a CommandOption, including choices for the type if supported.
enum OptionType {
    /// Subcommand cannot contain options with type Subcommand or SubcommandGroup, and can have
    /// [1,25] options.
    Subcommand(Vec<CommandOption>),
    /// SubcommandGroup can only contain options with type Subcommand, and can have [1,25]
    /// subcommands.
    SubcommandGroup(Vec<CommandOption>),
    /// String options can have [0,25] choices.
    String(Vec<(String, String)>),
    /// Integer options can have [0,25] choices.
    Integer(Vec<(String, i32)>),
    Boolean,
    User,
    Channel,
    Role,
    Mentionable,
}

impl OptionType {
    fn api_type(&self) -> ApplicationCommandOptionType {
        match self {
            OptionType::Subcommand(_) => ApplicationCommandOptionType::SubCommand,
            OptionType::SubcommandGroup(_) => ApplicationCommandOptionType::SubCommandGroup,
            OptionType::String(_) => ApplicationCommandOptionType::String,
            OptionType::Integer(_) => ApplicationCommandOptionType::Integer,
            OptionType::Boolean => ApplicationCommandOptionType::Boolean,
            OptionType::User => ApplicationCommandOptionType::User,
            OptionType::Channel => ApplicationCommandOptionType::Channel,
            OptionType::Role => ApplicationCommandOptionType::Role,
            OptionType::Mentionable => ApplicationCommandOptionType::Mentionable,
        }
    }
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
            .set_application_commands(&ctx, |commands| {
                let app_commands = COMMANDS
                    .values()
                    .map(|command| {
                        let mut builder = CreateApplicationCommand::default();
                        command.build(&mut builder);
                        builder
                    })
                    .collect();
                commands.set_application_commands(app_commands)
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

impl dyn Command {
    fn build(&self, builder: &mut CreateApplicationCommand) {
        assert!(self.options().len() <= 25);
        let options = self
            .options()
            .into_iter()
            .map(|option| {
                let mut builder = CreateApplicationCommandOption::default();
                option.build(&mut builder);
                builder
            })
            .collect();
        builder
            .name(self.name())
            .description(self.description())
            .set_options(options);
    }
}

impl CommandOption {
    fn build(self, builder: &mut CreateApplicationCommandOption) {
        let kind = self.type_choices.api_type();
        match self.type_choices {
            OptionType::Subcommand(options) | OptionType::SubcommandGroup(options) => {
                assert!(!options.is_empty());
                assert!(options.len() <= 25);
                options.into_iter().for_each(|option| {
                    let mut sub_builder = CreateApplicationCommandOption::default();
                    option.build(&mut sub_builder);
                    builder.add_sub_option(sub_builder);
                })
            }
            OptionType::String(choices) => {
                assert!(choices.len() <= 25);
                choices.into_iter().for_each(|(name, value)| {
                    builder.add_string_choice(name, value);
                })
            }
            OptionType::Integer(choices) => {
                assert!(choices.len() <= 25);
                choices.into_iter().for_each(|(name, value)| {
                    builder.add_int_choice(name, value);
                })
            }
            OptionType::Boolean
            | OptionType::User
            | OptionType::Channel
            | OptionType::Role
            | OptionType::Mentionable => {}
        };
        builder
            .kind(kind)
            .name(self.name)
            .description(self.description)
            .required(self.required);
    }
}
