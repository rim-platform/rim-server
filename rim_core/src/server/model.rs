use chrono::{DateTime, Utc};
use crate::model_id;

model_id!(ServerId);

pub struct Server {
    pub id: ServerId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

