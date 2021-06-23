use std::sync::Arc;

use command::CommandManager;
use event::EventManager;
use serenity::{
    async_trait,
    model::{gateway::Ready, id::GuildId, interactions::Interaction},
    prelude::*,
};
use store::PersistentStoreBuilder;
use tokio::sync::OnceCell;
use tracing::{debug, error, info};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[macro_use]
mod activity;

mod command;
mod embed;
mod event;
mod store;
mod time;
mod util;

#[derive(Default)]
struct Handler {
    command_manager: OnceCell<CommandManager>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
        debug!("Ready data: {:?}", ready);
    }

    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        debug!("Cache ready! Guilds = {:?}", guilds);

        assert!(guilds.len() == 1, "Expected bot to be in a single guild");
        let guild = guilds.first().unwrap();

        // TODO: Unclear whether this should be initialized in ready or cache_ready or if it
        // matters.
        self.command_manager
            .get_or_try_init(|| CommandManager::new(&ctx, guild))
            .await
            .expect("Failed to create CommandManager");
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let command_manager = match self.command_manager.get() {
            Some(mgr) => mgr,
            None => {
                error!("Interaction created before CommandManager created!");
                return;
            }
        };
        if let Err(err) = command_manager
            .dispatch_interaction(&ctx, interaction)
            .await
        {
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

    let mut client = Client::builder(&token)
        .application_id(app_id)
        .event_handler(Handler::default())
        .await
        .expect("Error creating client");

    let event_store = std::env::var("PERSISTENT_STORE_DIR").expect("Missing $PERSISTENT_STORE_DIR");
    let store_builder = PersistentStoreBuilder::new(event_store)
        .await
        .expect("Failed to create PersistentStoreBuilder");
    let event_manager = EventManager::new(&store_builder, client.cache_and_http.clone())
        .await
        .expect("Failed to create EventManager");

    {
        let mut typemap = client.data.write().await;
        typemap.insert::<EventManager>(Arc::new(event_manager));
    }

    client.start().await.expect("Client error");
}
