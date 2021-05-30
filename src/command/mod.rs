use anyhow::{ensure, format_err, Result};
use lazy_static::lazy_static;
use serenity::{
    async_trait,
    builder::{CreateApplicationCommand, CreateApplicationCommandOption},
    client::Context,
    model::{
        id::GuildId,
        interactions::{
            ApplicationCommand, ApplicationCommandInteractionData, ApplicationCommandOptionType,
            Interaction,
        },
    },
};
use std::collections::BTreeMap;
use tracing::debug;

#[macro_use]
mod macros;

mod lfg;
mod ping;

/// Generic trait that all command types implement.
pub trait Command: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    fn command_type(&self) -> CommandType;
}

pub type SubcommandsMap = BTreeMap<&'static str, Box<dyn LeafCommand>>;
pub type SubcommandGroupsMap = BTreeMap<&'static str, Box<dyn SubcommandGroup>>;

/// Type of the command, which differs depending on the number of nested layers.
pub enum CommandType<'a> {
    /// A leaf command that handles user interactions.
    Leaf(&'a dyn LeafCommand),
    /// A top level command that contains subcommands, i.e. a single layer of nesting.
    Subcommands(&'a SubcommandsMap),
    /// A top level command that contains subcommand groups, i.e. two layers of nesting.
    SubcommandGroups(&'a SubcommandGroupsMap),
}

/// A subcommand group is a command that contains leaf (sub)commands.
pub trait SubcommandGroup: Command {
    /// The subcommands contained in this group. Can only be leaf commands; no further nesting is
    /// allowed.
    fn subcommands(&self) -> &SubcommandsMap;
}

/// A leaf command, i.e. one that does not contain any Subcommand or SubcommandGroup options and
/// that handles user interactions.
#[async_trait]
pub trait LeafCommand: Command {
    /// Commands can have [0,25] options.
    fn options(&self) -> Vec<CommandOption>;

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) -> Result<()>;
}

/// Definition of a single command option.
pub struct CommandOption {
    name: String,
    description: String,
    required: bool,
    option_type: OptionType,
}

/// The type for a CommandOption, including any choices if the type supports them.
pub enum OptionType {
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
        vec![
            Box::new(ping::Ping::new()) as Box<dyn Command>,
        ].into_iter().map(|c| (c.name(), c)).collect()
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
        debug!("Received interaction: {:?}", interaction);
        let (cmd_name, leaf) = interaction.data.as_ref().map_or_else(
            || Err(format_err!("Interaction has no data: {:?}", interaction)),
            |data| self.find_leaf_command(data),
        )?;
        debug!("'{}' handling interaction", cmd_name);
        leaf.handle_interaction(ctx, interaction).await
    }

    fn find_leaf_command(
        &self,
        data: &ApplicationCommandInteractionData,
    ) -> Result<(String, &dyn LeafCommand)> {
        let name = data.name.as_str();
        let first = COMMANDS
            .get(name)
            .ok_or_else(|| format_err!("Unknown command '{}'", &data.name))?;
        Ok(match first.command_type() {
            CommandType::Leaf(leaf) => (name.to_string(), leaf),
            CommandType::Subcommands(subcommands) => {
                ensure!(
                    data.options.len() == 1,
                    "Expected 1 option to identify subcommand: {:?}",
                    data
                );
                let sub_name = data.options.first().unwrap().name.as_str();
                let cmd = subcommands
                    .get(sub_name)
                    .ok_or_else(|| format_err!("Unknown subcommand '{} {}'", &data.name, sub_name))?
                    .as_ref();
                ([name, sub_name].join("."), cmd)
            }
            CommandType::SubcommandGroups(groups) => {
                ensure!(
                    data.options.len() == 1,
                    "Expected 1 option to identify subcommand group: {:?}",
                    data
                );
                let group_data = data.options.first().unwrap();
                let group_name = group_data.name.as_str();
                let group = groups.get(group_name).ok_or_else(|| {
                    format_err!("Unknown subcommand group '{} {}'", &data.name, group_name)
                })?;
                let group_name = group.name();
                let subcommands = group.subcommands();

                ensure!(
                    group_data.options.len() == 1,
                    "Expected 1 option to identify subcommand in group: {:?}",
                    group_data
                );
                let sub_name = data.options.first().unwrap().name.as_str();
                let cmd = subcommands
                    .get(sub_name)
                    .ok_or_else(|| format_err!("Unknown subcommand '{} {}'", &data.name, sub_name))?
                    .as_ref();
                ([name, group_name, sub_name].join("."), cmd)
            }
        })
    }
}

impl dyn Command + '_ {
    fn build(&self, builder: &mut CreateApplicationCommand) {
        match self.command_type() {
            CommandType::Leaf(leaf) => leaf.build(builder),
            CommandType::Subcommands(subcommands) => todo!(),
            CommandType::SubcommandGroups(groups) => todo!(),
        };
    }
}

impl dyn LeafCommand + '_ {
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
        let kind = self.option_type.api_type();
        match self.option_type {
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
