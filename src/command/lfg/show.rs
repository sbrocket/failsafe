use super::{get_event_from_str, opts};
use crate::{
    event::{EventEmbedMessage, EventManager},
    util::*,
};
use anyhow::{format_err, Result};
use serde_json::Value;
use serenity::{
    client::Context,
    model::interactions::{ApplicationCommandInteractionDataOption, Interaction},
};

define_leaf_command!(
    LfgShow,
    "show",
    "Display an existing event",
    lfg_show,
    options: [opts::EventId],
);

#[command_attr::hook]
async fn lfg_show(
    ctx: &Context,
    interaction: &Interaction,
    options: &Vec<ApplicationCommandInteractionDataOption>,
) -> Result<()> {
    let event_id = match options.get_value("event_id")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(format_err!("Unexpected value type: {:?}", v)),
        None => Err(format_err!("Missing required event_id value")),
    }?;

    let type_map = ctx.data.read().await;
    let event_manager = type_map.get::<EventManager>().unwrap();
    let mut event = None;
    interaction
        .create_interaction_response(&ctx, |resp| {
            resp.interaction_response_data(|msg| {
                match get_event_from_str(event_manager, &event_id) {
                    Ok(e) => {
                        let ret = msg.add_embed(e.as_embed()).components(|c| {
                            *c = e.event_buttons();
                            c
                        });
                        event = Some(e);
                        ret
                    }
                    Err(content) => msg.content(content),
                }
            })
        })
        .await?;
    if let Some(event) = event {
        let msg = interaction.get_interaction_response(&ctx).await?;
        event
            .keep_embed_updated(EventEmbedMessage::Normal(msg.channel_id, msg.id))
            .await?;
    }
    Ok(())
}
