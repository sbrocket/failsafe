use crate::{
    activity::ActivityType,
    command::CommandManager,
    embed::{EmbedManagerConfig, EventChannelFilterFn},
    event::{Event, EventManager},
    store::PersistentStoreBuilder,
};
use anyhow::{format_err, Context as _, Result};
use derivative::Derivative;
use itertools::Itertools;
use serde::Deserialize;
use serenity::{
    model::{
        id::{ChannelId, GuildId},
        interactions::Interaction,
    },
    prelude::*,
};
use std::{collections::HashMap, path::Path, sync::Arc};
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Debug, Default)]
pub struct GuildConfig {
    pub embed_config: EmbedManagerConfig,
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct GuildManager {
    store_builder: PersistentStoreBuilder,
    config: GuildConfigToml,
    #[derivative(Debug = "ignore")]
    event_managers: RwLock<HashMap<GuildId, Arc<EventManager>>>,
    command_manager: CommandManager,
}

impl GuildManager {
    pub fn new(
        store_builder: PersistentStoreBuilder,
        config_file: impl AsRef<Path>,
    ) -> Result<Self> {
        let config_file = config_file.as_ref();
        let config = std::fs::read_to_string(config_file).with_context(|| {
            format!(
                "Failed to read guild config file ({})",
                config_file.display()
            )
        })?;
        let config = toml::from_str(&config).context("Failed to deserialize guild config")?;
        Ok(GuildManager {
            store_builder,
            config,
            event_managers: Default::default(),
            command_manager: CommandManager::new(),
        })
    }

    pub async fn add_guilds(&self, ctx: &Context, guild_ids: Vec<GuildId>) -> Result<()> {
        let mut managers = self.event_managers.write().await;

        let mut errors = Vec::new();
        for guild_id in guild_ids {
            // This is expected; existing guild IDs will be passed in each time.
            if managers.contains_key(&guild_id) {
                continue;
            }

            info!("Added to guild {}", guild_id);
            match self.add_guild(ctx.clone(), guild_id).await {
                Ok(mgr) => {
                    managers.insert(guild_id, mgr);
                }
                Err(err) => errors.push(err),
            }
        }

        if !errors.is_empty() {
            return Err(format_err!(
                "Errors occurred adding guilds: {}",
                errors.iter().map(|e| format!("{:?}", e)).join(", ")
            ));
        }
        Ok(())
    }

    async fn add_guild(&self, ctx: Context, guild_id: GuildId) -> Result<Arc<EventManager>> {
        let http = ctx.http.clone();
        let guild_store = self
            .store_builder
            .new_scoped(guild_id.as_u64().to_string())
            .await
            .with_context(|| format!("Failed to create guild {} store", guild_id))?;
        let event_manager =
            EventManager::new(ctx, guild_store, self.config.config_for_guild(guild_id))
                .await
                .with_context(|| format!("Failed to create EventManager for guild {}", guild_id))?;

        self.command_manager.add_guild(&http, &guild_id).await?;
        Ok(event_manager)
    }

    pub async fn removed_from_guild(&self, guild_id: GuildId) {
        let mut managers = self.event_managers.write().await;
        info!("Removed from guild {}", guild_id);
        match managers.remove(&guild_id) {
            Some(mgr) => mgr.removed_from_guild(),
            None => error!("No EventManager exists for removed guild {}", guild_id),
        }
    }

    pub async fn get_event_manager(&self, guild_id: GuildId) -> Result<Arc<EventManager>> {
        let managers = self.event_managers.read().await;
        if let Some(mgr) = managers.get(&guild_id) {
            return Ok(mgr.clone());
        }
        Err(format_err!("No EventManager exists for guild {}", guild_id))
    }

    pub async fn dispatch_interaction(
        &self,
        ctx: &Context,
        interaction: Interaction,
    ) -> Result<()> {
        self.command_manager
            .dispatch_interaction(ctx, interaction)
            .await
    }
}

impl TypeMapKey for GuildManager {
    type Value = Arc<GuildManager>;
}

// TODO: Add guild admin configuration commands to replace the fixed config
#[derive(Debug, Deserialize)]
struct GuildConfigToml {
    guilds: HashMap<GuildId, SingleGuildConfigToml>,
}

#[derive(Debug, Deserialize)]
struct SingleGuildConfigToml {
    raid_lfg: ChannelId,
    pve_lfg: ChannelId,
    pvp_lfg: ChannelId,
    special_lfg: ChannelId,
    all_lfg: ChannelId,
}

impl GuildConfigToml {
    pub fn config_for_guild(&self, guild_id: GuildId) -> GuildConfig {
        self.guilds
            .get(&guild_id)
            .map(GuildConfig::from)
            .unwrap_or_default()
    }
}

impl From<&SingleGuildConfigToml> for GuildConfig {
    fn from(cfg: &SingleGuildConfigToml) -> Self {
        let v: Vec<(_, EventChannelFilterFn)> = vec![
            (
                cfg.raid_lfg,
                Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Raid),
            ),
            (
                cfg.pve_lfg,
                Box::new(|e: &Event| match e.activity.activity_type() {
                    ActivityType::Dungeon
                    | ActivityType::Gambit
                    | ActivityType::ExoticQuest
                    | ActivityType::Seasonal
                    | ActivityType::Other => true,
                    _ => false,
                }),
            ),
            (
                cfg.pvp_lfg,
                Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Crucible),
            ),
            (
                cfg.special_lfg,
                Box::new(|e: &Event| e.activity.activity_type() == ActivityType::Custom),
            ),
            (cfg.all_lfg, Box::new(|_: &Event| true)),
        ];
        let event_channels = v.into_iter().collect();
        GuildConfig {
            embed_config: EmbedManagerConfig { event_channels },
        }
    }
}
