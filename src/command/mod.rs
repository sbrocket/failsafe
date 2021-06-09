use anyhow::{ensure, format_err, Result};
use lazy_static::lazy_static;
use serenity::{
    async_trait,
    builder::{CreateApplicationCommand, CreateApplicationCommandOption},
    client::Context,
    model::{
        id::GuildId,
        interactions::{
            ApplicationCommand, ApplicationCommandInteractionData,
            ApplicationCommandInteractionDataOption, ApplicationCommandOptionType, Interaction,
            InteractionData,
        },
    },
};
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

pub type SubcommandsMap = Vec<(&'static str, Box<dyn Command>)>;

/// Type of the command, which differs depending on the number of nested layers.
pub enum CommandType<'a> {
    /// A leaf command that handles user interactions.
    Leaf(&'a dyn LeafCommand),
    /// A top level command that contains subcommands and/or subcommand groups. Note that
    /// subcommands can only be nested twice.
    Subcommands(&'a SubcommandsMap),
}

/// Trait for leaf commands, which have user-facing options and handle user interactions.
#[async_trait]
pub trait LeafCommand: Command {
    /// Commands can have [0,25] options.
    fn options(&self) -> Vec<CommandOption>;

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()>;
}

/// Definition of a single command option.
pub struct CommandOption {
    name: &'static str,
    description: &'static str,
    required: bool,
    option_type: OptionType,
}

/// The type for a CommandOption, including any choices if the type supports them.
#[allow(dead_code)]
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
    static ref COMMANDS: Vec<(&'static str, Box<dyn Command>)> = {
        vec![
            Box::new(ping::Ping::new()) as Box<dyn Command>,
            Box::new(lfg::Lfg::new()) as Box<dyn Command>,
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
                    .iter()
                    .map(|(_, command)| command.build())
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

        // TODO: Parse the options into an easier to consume form.
        let data = match interaction.data.as_ref() {
            Some(InteractionData::ApplicationCommand(data)) => data,
            _ => {
                return Err(format_err!(
                    "Unexpected interaction type: {:?}",
                    interaction
                ))
            }
        };
        let (cmd_name, leaf, options) = self.find_leaf_command(data)?;

        debug!("'{}' handling interaction", cmd_name);
        leaf.handle_interaction(ctx, &interaction, options).await
    }

    fn find_leaf_command<'a>(
        &self,
        data: &'a ApplicationCommandInteractionData,
    ) -> Result<(
        String,
        &dyn LeafCommand,
        &'a Vec<ApplicationCommandInteractionDataOption>,
    )> {
        let name1 = data.name.as_str();
        let first = &COMMANDS
            .iter()
            .find(|(name, _)| *name == name1)
            .ok_or_else(|| format_err!("Unknown command '{}'", &data.name))?
            .1;

        // TODO: This works but pretty clearly could be shortened with looping or recursion.
        Ok(match first.command_type() {
            CommandType::Leaf(leaf) => (name1.to_string(), leaf, &data.options),
            CommandType::Subcommands(subcommands) => {
                ensure!(
                    data.options.len() == 1,
                    "Expected 1 option to identify subcommand: {:?}",
                    data
                );
                let sub_data = data.options.first().unwrap();
                let name2 = data.options.first().unwrap().name.as_str();
                let cmd = &subcommands
                    .iter()
                    .find(|(name, _)| *name == name2)
                    .ok_or_else(|| format_err!("Unknown subcommand '{} {}'", name1, name2))?
                    .1;

                // Check if this is a leaf or if there's a 2nd layer of nesting.
                match cmd.command_type() {
                    CommandType::Leaf(leaf) => ([name1, name2].join("."), leaf, &sub_data.options),
                    CommandType::Subcommands(cmds) => {
                        ensure!(
                            sub_data.options.len() == 1,
                            "Expected 1 option to identify subcommand: {:?}",
                            sub_data
                        );
                        let group_data = sub_data.options.first().unwrap();
                        let name3 = group_data.name.as_str();
                        let cmd = &cmds
                            .iter()
                            .find(|(name, _)| *name == name3)
                            .ok_or_else(|| {
                                format_err!(
                                    "Unknown subcommand in group '{} {} {}'",
                                    name1,
                                    name2,
                                    name3
                                )
                            })?
                            .1;

                        if let CommandType::Leaf(leaf) = cmd.command_type() {
                            ([name1, name2, name3].join("."), leaf, &group_data.options)
                        } else {
                            // Unreachable because this should have been caught during command
                            // creation.
                            unreachable!("Only 2 layers of nesting are allowed");
                        }
                    }
                }
            }
        })
    }
}

impl dyn Command + '_ {
    fn build(&self) -> CreateApplicationCommand {
        let mut command = CreateApplicationCommand::default();
        let options = match self.command_type() {
            CommandType::Leaf(leaf) => leaf.build_options(),
            CommandType::Subcommands(cmds) => {
                assert!(cmds.len() <= 25);
                cmds.iter()
                    .map(|(_, cmd)| match cmd.command_type() {
                        CommandType::Leaf(leaf) => leaf.build_as_subcommand(),
                        CommandType::Subcommands(_) => cmd.build_subcommand_group(),
                    })
                    .collect()
            }
        };
        command
            .name(self.name())
            .description(self.description())
            .set_options(options);
        command
    }

    fn build_subcommand_group(&self) -> CreateApplicationCommandOption {
        let mut command = CreateApplicationCommandOption::default();
        command
            .kind(ApplicationCommandOptionType::SubCommandGroup)
            .name(self.name())
            .description(self.description());
        if let CommandType::Subcommands(cmds) = self.command_type() {
            assert!(cmds.len() <= 25);
            cmds.iter().for_each(|(_, cmd)| {
                if let CommandType::Leaf(leaf) = cmd.command_type() {
                    let _ = command.add_sub_option(leaf.build_as_subcommand());
                } else {
                    // Fine to panic since this is operating on static command definitions
                    panic!("Only 2 layers of nesting are allowed");
                }
            });
        } else {
            unreachable!("Only called for CommandType::Subcommands");
        }
        command
    }
}

impl dyn LeafCommand + '_ {
    fn build_as_subcommand(&self) -> CreateApplicationCommandOption {
        let mut command = CreateApplicationCommandOption::default();
        command
            .kind(ApplicationCommandOptionType::SubCommand)
            .name(self.name())
            .description(self.description());
        self.build_options().into_iter().for_each(|opt| {
            let _ = command.add_sub_option(opt);
        });
        command
    }

    fn build_options(&self) -> Vec<CreateApplicationCommandOption> {
        assert!(self.options().len() <= 25);
        self.options()
            .into_iter()
            .map(|option| option.build())
            .collect()
    }
}

impl CommandOption {
    fn build(self) -> CreateApplicationCommandOption {
        let mut option = CreateApplicationCommandOption::default();
        let kind = self.option_type.api_type();
        match self.option_type {
            OptionType::String(choices) => {
                assert!(choices.len() <= 25);
                choices.into_iter().for_each(|(name, value)| {
                    option.add_string_choice(name, value);
                })
            }
            OptionType::Integer(choices) => {
                assert!(choices.len() <= 25);
                choices.into_iter().for_each(|(name, value)| {
                    option.add_int_choice(name, value);
                })
            }
            OptionType::Boolean
            | OptionType::User
            | OptionType::Channel
            | OptionType::Role
            | OptionType::Mentionable => {}
        };
        option
            .kind(kind)
            .name(self.name)
            .description(self.description)
            .required(self.required);
        option
    }
}
