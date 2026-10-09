use crate::Actor;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct GetMeQuery {
    pub actor: Actor,
}

#[derive(Debug, Clone)]
pub struct GetUserQuery {
    pub actor: Actor,
    pub user_id: UserId,
}

#[derive(Debug, Clone)]
pub struct UpdateMyProfileCmd {
    pub actor: Actor,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
}
