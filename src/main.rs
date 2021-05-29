use serenity::{async_trait, model::gateway::Ready, prelude::*};
use tracing::{debug, info};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
        debug!("Ready data: {:#?}", ready);
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
    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    client.start().await.expect("Client error");
}
