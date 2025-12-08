use rim_core::server::model::{Server};
use crate::{server, user};

impl From<server::Model> for Server {
    fn from(m: server::Model) -> Self {
        Self {
            id: m.id.into(),
            name: m.name,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
