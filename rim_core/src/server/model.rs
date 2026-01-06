use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(ServerId);

pub struct Server {
    pub id: ServerId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
