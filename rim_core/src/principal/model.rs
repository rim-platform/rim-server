use crate::model_id;
use crate::user::model::UserId;

model_id!(PrincipalId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrincipalKind {
    User,
    App,
    Bot,
    Webhook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Principal {
    pub id: PrincipalId,
    pub kind: PrincipalKind,
    pub user_id: Option<UserId>,
}

impl Principal {
    pub fn user(user_id: UserId) -> Self {
        Self {
            id: PrincipalId::from(uuid::Uuid::from(user_id)),
            kind: PrincipalKind::User,
            user_id: Some(user_id),
        }
    }

    pub fn app(id: PrincipalId) -> Self {
        Self {
            id,
            kind: PrincipalKind::App,
            user_id: None,
        }
    }

    pub fn bot(id: PrincipalId) -> Self {
        Self {
            id,
            kind: PrincipalKind::Bot,
            user_id: None,
        }
    }

    pub fn webhook(id: PrincipalId) -> Self {
        Self {
            id,
            kind: PrincipalKind::Webhook,
            user_id: None,
        }
    }
}
