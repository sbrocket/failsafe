use anyhow::{format_err, Result};
use rand::{distributions::Alphanumeric, prelude::*};
use serde_json::Value;
use serenity::model::{
    interactions::{
        ApplicationCommandInteractionDataOption,
        ApplicationCommandInteractionDataOptionValue as OptionValue, Interaction,
    },
    prelude::*,
};
use std::{io::ErrorKind, path::PathBuf};
use tokio::fs::File;

pub trait InteractionExt {
    fn get_user(&self) -> Result<&User>;
}

pub trait OptionsExt {
    fn get_value(&self, name: impl AsRef<str>) -> Result<Option<&Value>>;

    fn get_resolved(&self, name: impl AsRef<str>) -> Result<Option<&OptionValue>>;
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
    fn get_value(&self, name: impl AsRef<str>) -> Result<Option<&Value>> {
        let name = name.as_ref();
        let option = if let Some(option) = self.iter().find(|opt| opt.name == name) {
            option
        } else {
            return Ok(None);
        };
        option.value.as_ref().map_or_else(
            || Err(format_err!("No value for option '{}'", name)),
            |v| Ok(Some(v)),
        )
    }

    fn get_resolved(&self, name: impl AsRef<str>) -> Result<Option<&OptionValue>> {
        let name = name.as_ref();
        let option = if let Some(option) = self.iter().find(|opt| opt.name == name) {
            option
        } else {
            return Ok(None);
        };
        option.resolved.as_ref().map_or_else(
            || Err(format_err!("No resolved value for option '{}'", name)),
            |v| Ok(Some(v)),
        )
    }
}

pub async fn tempfile() -> Result<(PathBuf, File)> {
    const TEMP_PREFIX: &str = "tmpfile_";
    const RAND_LEN: usize = 10;
    const RETRIES: usize = 4;

    for _ in 0..RETRIES {
        let mut tempname = String::with_capacity(TEMP_PREFIX.len() + RAND_LEN);
        tempname.push_str(TEMP_PREFIX);
        tempname.extend(
            thread_rng()
                .sample_iter(Alphanumeric)
                .take(RAND_LEN)
                .map(char::from),
        );

        let mut path = std::env::temp_dir();
        path.push(tempname);
        match File::create(&path).await {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            file => return Ok((path, file?)),
        };
    }
    Err(format_err!("Failed to create tempfile"))
}
