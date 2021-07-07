use anyhow::{ensure, format_err, Context as _, Result};
use futures::future::BoxFuture;
use lazy_static::lazy_static;
use serenity::{
    builder::{CreateApplicationCommand, CreateApplicationCommandOption},
    client::Context,
    http::Http,
    model::{
        id::GuildId,
        interactions::{
            application_command::{
                ApplicationCommandInteraction, ApplicationCommandInteractionData,
                ApplicationCommandInteractionDataOption, ApplicationCommandOptionType,
            },
            Interaction,
        },
    },
};
use tracing::debug;

#[macro_use]
mod macros;

mod lfg;

/// Definition of a command.
pub struct Command {
    name: &'static str,
    description: &'static str,
    command_type: CommandType,
}

/// Type of the command, which differs depending on the number of nested layers.
pub enum CommandType {
    /// A leaf command that handles user interactions.
    Leaf(&'static LeafCommand),
    /// A top level command that contains subcommands and/or subcommand groups, or a subcommand
    /// group that contains subcommands. In other words, only two levels of nesting are supported.
    Group(&'static [&'static Command]),
}

type CommandHandler = for<'fut> fn(
    &'fut Context,
    &'fut ApplicationCommandInteraction,
    &'fut Vec<ApplicationCommandInteractionDataOption>,
) -> BoxFuture<'fut, Result<()>>;

/// Definition of a leaf command, which has user-facing options and handles user interactions.
pub struct LeafCommand {
    options: &'static [&'static CommandOption],
    handler: CommandHandler,
}

/// Definition of a single command option.
#[derive(Debug, Clone)]
pub struct CommandOption {
    name: &'static str,
    description: &'static str,
    required: bool,
    option_type: OptionType,
}

/// The value type for a CommandOption, including any choices if the type supports them.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum OptionType {
    /// String options can have [0,25] choices.
    String(&'static [(&'static str, &'static str)]),
    /// Integer options can have [0,25] choices.
    Integer(&'static [(&'static str, i32)]),
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

// List of all known top-level commands; add new commands here as they're created.
lazy_static! {
    static ref COMMANDS: Vec<&'static Command> = vec![&*lfg::Lfg::COMMAND];
}

/// Manages the bot's slash commands, handling creating the commands on startup and dispatching
/// interactions as they're received.
#[derive(Debug)]
pub struct CommandManager;

impl CommandManager {
    pub fn new() -> CommandManager {
        Self
    }

    /// Set up a newly ready GuildId, creating guild application commands as needed.
    pub async fn add_guild(&self, http: impl AsRef<Http>, guild: &GuildId) -> Result<()> {
        // There's a rate limit on creating commands (200 per day per guild) that could get hit if
        // restarting the bot frequently, unclear if replacing/updating commands counts against that
        // limit.
        let http = http.as_ref();
        guild
            .set_application_commands(http, |commands| {
                let app_commands = COMMANDS.iter().map(|command| command.build()).collect();
                commands.set_application_commands(app_commands)
            })
            .await
            .with_context(|| format!("Failed to set commands for guild {}", guild))?;
        Ok(())
    }

    /// Dispatch the given interaction to the appropriate command.
    pub async fn dispatch_interaction(
        &self,
        ctx: &Context,
        interaction: Interaction,
    ) -> Result<()> {
        debug!("Received interaction: {:?}", interaction);

        match interaction {
            Interaction::ApplicationCommand(interaction) => {
                // TODO: Parse the options into an easier to consume form.
                let (cmd_name, leaf, options) = self.find_leaf_command(&interaction.data)?;

                debug!("'{}' handling command interaction", cmd_name);
                (leaf.handler)(ctx, &interaction, options).await
            }
            Interaction::MessageComponent(interaction) => {
                lfg::handle_component_interaction(ctx, &interaction).await
            }
            Interaction::Ping(i) => Err(format_err!("Unexpected Ping interaction: {:?}", i)),
        }
    }

    fn find_leaf_command<'a>(
        &self,
        data: &'a ApplicationCommandInteractionData,
    ) -> Result<(
        String,
        &'a LeafCommand,
        &'a Vec<ApplicationCommandInteractionDataOption>,
    )> {
        let name1 = data.name.as_str();
        let first = &COMMANDS
            .iter()
            .find(|cmd| cmd.name == name1)
            .ok_or_else(|| format_err!("Unknown command '{}'", &data.name))?;

        // TODO: This works but pretty clearly could be shortened with looping or recursion.
        Ok(match first.command_type {
            CommandType::Leaf(leaf) => (name1.to_string(), leaf, &data.options),
            CommandType::Group(subcommands) => {
                ensure!(
                    data.options.len() == 1,
                    "Expected 1 option to identify subcommand: {:?}",
                    data
                );
                let sub_data = data.options.first().unwrap();
                let name2 = data.options.first().unwrap().name.as_str();
                let cmd = &subcommands
                    .iter()
                    .find(|cmd| cmd.name == name2)
                    .ok_or_else(|| format_err!("Unknown subcommand '{} {}'", name1, name2))?;

                // Check if this is a leaf or if there's a 2nd layer of nesting.
                match cmd.command_type {
                    CommandType::Leaf(leaf) => ([name1, name2].join("."), leaf, &sub_data.options),
                    CommandType::Group(cmds) => {
                        ensure!(
                            sub_data.options.len() == 1,
                            "Expected 1 option to identify subcommand: {:?}",
                            sub_data
                        );
                        let group_data = sub_data.options.first().unwrap();
                        let name3 = group_data.name.as_str();
                        let cmd = &cmds.iter().find(|cmd| cmd.name == name3).ok_or_else(|| {
                            format_err!(
                                "Unknown subcommand in group '{} {} {}'",
                                name1,
                                name2,
                                name3
                            )
                        })?;

                        if let CommandType::Leaf(leaf) = cmd.command_type {
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

impl Command {
    fn build(&self) -> CreateApplicationCommand {
        let mut command = CreateApplicationCommand::default();
        let options = match self.command_type {
            CommandType::Leaf(leaf) => leaf.build_options(),
            CommandType::Group(cmds) => {
                assert!(cmds.len() <= 25);
                cmds.iter()
                    .map(|cmd| match cmd.command_type {
                        CommandType::Leaf(leaf) => cmd.build_as_subcommand(leaf),
                        CommandType::Group(_) => cmd.build_subcommand_group(),
                    })
                    .collect()
            }
        };
        command
            .name(self.name)
            .description(self.description)
            .set_options(options);
        command
    }

    fn build_as_subcommand(&self, leaf: &LeafCommand) -> CreateApplicationCommandOption {
        let mut command = CreateApplicationCommandOption::default();
        command
            .kind(ApplicationCommandOptionType::SubCommand)
            .name(self.name)
            .description(self.description);
        leaf.build_options().into_iter().for_each(|opt| {
            let _ = command.add_sub_option(opt);
        });
        command
    }

    fn build_subcommand_group(&self) -> CreateApplicationCommandOption {
        let mut command = CreateApplicationCommandOption::default();
        command
            .kind(ApplicationCommandOptionType::SubCommandGroup)
            .name(self.name)
            .description(self.description);
        if let CommandType::Group(cmds) = self.command_type {
            assert!(cmds.len() <= 25);
            cmds.iter().for_each(|cmd| {
                if let CommandType::Leaf(leaf) = cmd.command_type {
                    let _ = command.add_sub_option(cmd.build_as_subcommand(leaf));
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

impl LeafCommand {
    fn build_options(&self) -> Vec<CreateApplicationCommandOption> {
        assert!(self.options.len() <= 25);
        self.options.iter().map(|option| option.build()).collect()
    }
}

impl CommandOption {
    fn build(&self) -> CreateApplicationCommandOption {
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
                    option.add_int_choice(name, *value);
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
