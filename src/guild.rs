use crate::{command::CommandManager, event::EventManager, store::PersistentStoreBuilder};
use anyhow::{format_err, Context as _, Result};
use derivative::Derivative;
use itertools::Itertools;
use serenity::{
    model::{id::GuildId, interactions::Interaction},
    prelude::*,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct GuildManager {
    store_builder: PersistentStoreBuilder,
    #[derivative(Debug = "ignore")]
    event_managers: RwLock<HashMap<GuildId, Arc<EventManager>>>,
    command_manager: CommandManager,
}

impl GuildManager {
    pub fn new(store_builder: PersistentStoreBuilder) -> Self {
        GuildManager {
            store_builder,
            event_managers: Default::default(),
            command_manager: CommandManager::new(),
        }
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
        let event_manager = EventManager::new(
            ctx,
            self.store_builder
                .new_scoped(guild_id.as_u64().to_string())
                .await
                .with_context(|| format!("Failed to create guild {} store", guild_id))?,
        )
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
