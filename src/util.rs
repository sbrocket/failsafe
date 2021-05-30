use anyhow::{format_err, Result};
use serenity::model::{id::UserId, interactions::Interaction};

pub trait InteractionExt {
    fn get_user_id(&self) -> Result<UserId>;
}

impl InteractionExt for Interaction {
    fn get_user_id(&self) -> Result<UserId> {
        self.member
            .as_ref()
            .map(|m| m.user.id)
            .or(self.user.as_ref().map(|u| u.id))
            .ok_or(format_err!("Interaction from no user?! {:?}", self))
    }
}
