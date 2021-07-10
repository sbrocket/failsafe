use guild::GuildManager;
use serenity::{
    async_trait,
    model::{
        gateway::Ready,
        guild::{Guild, GuildUnavailable},
        id::GuildId,
        interactions::Interaction,
    },
    prelude::*,
};
use std::sync::Arc;
use store::PersistentStoreBuilder;
use tracing::{debug, error, info};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[macro_use]
mod activity;

mod command;
mod embed;
mod event;
mod guild;
mod store;
mod time;
mod util;

#[derive(Default)]
struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
        debug!("Ready data: {:?}", ready);
    }

    // This is sent in response to the "GuildCreate" event, but also indicates that the cache is
    // ready for use with the given GuildIds.
    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        debug!("Cache ready! Guilds = {:?}", guilds);

        let typemap = ctx.data.read().await;
        let guild_manager = typemap
            .get::<GuildManager>()
            .expect("GuildManager uninitialized");
        if let Err(err) = guild_manager.add_guilds(&ctx, guilds).await {
            error!("Error adding new guilds: {:?}", err);
        }
    }

    async fn guild_delete(&self, ctx: Context, guild: GuildUnavailable, _full: Option<Guild>) {
        // If this is true, the guild just went offline. Otherwise the bot was removed.
        if guild.unavailable {
            return;
        }

        let typemap = ctx.data.read().await;
        let guild_manager = typemap
            .get::<GuildManager>()
            .expect("GuildManager uninitialized");
        guild_manager.removed_from_guild(guild.id).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let typemap = ctx.data.read().await;
        let guild_manager = typemap
            .get::<GuildManager>()
            .expect("GuildManager uninitialized");
        if let Err(err) = guild_manager.dispatch_interaction(&ctx, interaction).await {
            error!("Error dispatching interaction: {:?}", err);
        }
    }
}

#[tokio::main]
async fn main() {
    // Load .env if one exists, but not required. (Environment vars could be passed directly)
    dotenv::dotenv().ok();

    // Setup tracing/logging.
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to start the logger");

    let token = std::env::var("DISCORD_BOT_TOKEN").expect("Missing $DISCORD_BOT_TOKEN");
    let app_id = std::env::var("DISCORD_APP_ID")
        .expect("Missing DISCORD_APP_ID")
        .parse()
        .expect("DISCORD_APP_ID not a valid u64");

    let event_store = std::env::var("PERSISTENT_STORE_DIR").expect("Missing $PERSISTENT_STORE_DIR");
    let store_builder = PersistentStoreBuilder::new(event_store)
        .await
        .expect("Failed to create PersistentStoreBuilder");
    let guild_manager = GuildManager::new(store_builder);

    let mut client = Client::builder(&token)
        .application_id(app_id)
        .event_handler(Handler::default())
        .type_map_insert::<GuildManager>(Arc::new(guild_manager))
        .await
        .expect("Error creating client");

    client.start().await.expect("Client error");
}
