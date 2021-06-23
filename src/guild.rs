use crate::{event::EventManager, store::PersistentStoreBuilder};
use serenity::{model::id::GuildId, prelude::*, CacheAndHttp};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

pub struct GuildManager {
    store_builder: PersistentStoreBuilder,
    http: Arc<CacheAndHttp>,
    event_managers: RwLock<HashMap<GuildId, Arc<EventManager>>>,
}

impl GuildManager {
    pub fn new(store_builder: PersistentStoreBuilder, http: Arc<CacheAndHttp>) -> Self {
        GuildManager {
            store_builder,
            http,
            event_managers: Default::default(),
        }
    }

    pub async fn get_event_manager(&self, guild_id: GuildId) -> Arc<EventManager> {
        // Try the quick, read-lock only path first.
        {
            let managers = self.event_managers.read().await;
            if let Some(mgr) = managers.get(&guild_id) {
                return mgr.clone();
            }
        }

        // Slower path, need to create EventManager for new guild.
        let mut managers = self.event_managers.write().await;
        // Check whether the manager was created while we were grabbing the write lock.
        if let Some(mgr) = managers.get(&guild_id) {
            return mgr.clone();
        }

        // Create a new EventManager.
        let event_manager = Arc::new(
            EventManager::new(
                &self
                    .store_builder
                    .new_scoped(guild_id.as_u64().to_string())
                    .await
                    .expect("Failed to create guild-scoped store"),
                self.http.clone(),
            )
            .await
            .expect("Failed to create new guild EventManager"),
        );
        managers.insert(guild_id, event_manager.clone());
        event_manager
    }
}

impl TypeMapKey for GuildManager {
    type Value = GuildManager;
}
