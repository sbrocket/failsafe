use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::model::{
    interactions::{ApplicationCommandInteractionDataOption, Interaction},
    prelude::*,
};

pub trait InteractionExt {
    fn get_user(&self) -> Result<&User>;
}

pub trait OptionsExt {
    fn get_value(&self, name: impl AsRef<str>) -> Result<&Value>;
}

impl InteractionExt for Interaction {
    fn get_user(&self) -> Result<&User> {
        self.member
            .as_ref()
            .map(|m| &m.user)
            .or(self.user.as_ref())
            .ok_or(format_err!("Interaction from no user?! {:?}", self))
    }
}

impl OptionsExt for &Vec<ApplicationCommandInteractionDataOption> {
    fn get_value(&self, name: impl AsRef<str>) -> Result<&Value> {
        let name = name.as_ref();
        let option = self
            .iter()
            .find(|opt| opt.name == name)
            .ok_or_else(|| format_err!("No option '{}' in data", name))?;
        option
            .value
            .as_ref()
            .ok_or_else(|| format_err!("No value for option '{}'", name))
    }
}
