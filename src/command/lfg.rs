use super::{CommandOption, LeafCommand};
use crate::{
    activity::{Activity, ActivityType},
    command::OptionType,
    util::*,
};
use anyhow::{format_err, Result};
use paste::paste;
use serde_json::Value;
use serenity::{
    async_trait,
    client::Context,
    model::interactions::{
        ApplicationCommandInteractionDataOption, Interaction, InteractionResponseType,
    },
    utils::MessageBuilder,
};
use std::concat;

define_command!(Lfg, "lfg", "Command for interacting with scheduled events",
                Subcommands: [LfgJoin, LfgCreate]);
define_command!(LfgJoin, "join", "Join an existing event", Leaf);

macro_rules! define_create_commands {
    ($($enum_name:ident: ($name:literal, $cmd:literal)),+ $(,)?) => {
        paste! {
            define_command!(LfgCreate, "create", "Create a new event",
                            Subcommands: [
                                $(
                                    [<LfgCreate $enum_name>]
                                ),+
                            ]);

            $(
                define_command!([<LfgCreate $enum_name>], $cmd,
                                concat!("Create a new ", $name, "event"), Leaf);

                impl LfgCreateActivity for [<LfgCreate $enum_name>] {
                    const ACTIVITY: ActivityType = ActivityType::$enum_name;
                }
            )+
        }
    }
}

with_activity_types! { define_create_commands }

#[async_trait]
impl LeafCommand for LfgJoin {
    fn options(&self) -> Vec<CommandOption> {
        vec![]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        _options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
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

pub trait LfgCreateActivity: LeafCommand {
    const ACTIVITY: ActivityType;
}

#[async_trait]
impl<T: LfgCreateActivity> LeafCommand for T {
    fn options(&self) -> Vec<CommandOption> {
        let activities = Activity::activities_with_type(T::ACTIVITY)
            .map(|a| (a.name().to_string(), a.id_prefix().to_string()))
            .collect();
        vec![CommandOption {
            name: "activity",
            description: "Activity for this event",
            required: true,
            option_type: OptionType::String(activities),
        }]
    }

    async fn handle_interaction(
        &self,
        ctx: &Context,
        interaction: &Interaction,
        options: &Vec<ApplicationCommandInteractionDataOption>,
    ) -> Result<()> {
        let user = interaction.get_user_id()?;
        let activity = match options.get_value("activity")? {
            Value::String(v) => Ok(v),
            v => Err(format_err!("Unexpected value type: {:?}", v)),
        }?;

        interaction
            .create_interaction_response(&ctx, |resp| {
                let message = MessageBuilder::new()
                    .push("Hi ")
                    .mention(&user)
                    .push("! I'm still repairing myself, but I'll be able to create your ")
                    .push(activity)
                    .push(" event soon...")
                    .build();
                resp.kind(InteractionResponseType::ChannelMessageWithSource)
                    .interaction_response_data(|msg| msg.content(message))
            })
            .await?;
        Ok(())
    }
}
