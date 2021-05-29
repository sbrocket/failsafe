use serenity::{async_trait, model::gateway::Ready, prelude::*};

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
        println!("Ready data: {:#?}", ready);
    }
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    let token = std::env::var("DISCORD_BOT_TOKEN").expect("Missing $DISCORD_BOT_TOKEN");

    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    client.start().await.expect("Client error");
}
