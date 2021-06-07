use anyhow::{format_err, Result};
use rand::{distributions::Alphanumeric, prelude::*};
use serde_json::Value;
use serenity::model::{
    interactions::{ApplicationCommandInteractionDataOption, Interaction},
    prelude::*,
};
use std::{io::ErrorKind, path::PathBuf};
use tokio::fs::File;

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

pub mod serialize_arc_rwlock {
    use std::sync::Arc;

    use super::*;
    use futures::executor::block_on;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use tokio::sync::RwLock;

    pub fn serialize<S, V>(lock: &Arc<RwLock<V>>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        V: Serialize,
    {
        let value = block_on(lock.read());
        Serialize::serialize(&*value, s)
    }

    pub fn deserialize<'de, D, V>(d: D) -> Result<Arc<RwLock<V>>, D::Error>
    where
        D: Deserializer<'de>,
        V: Deserialize<'de>,
    {
        let value: V = Deserialize::deserialize(d)?;
        Ok(Arc::new(RwLock::new(value)))
    }
}
