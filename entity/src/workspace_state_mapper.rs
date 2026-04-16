use crate::workspace_state;
use rim_core::workspace_state::model::{WorkspaceState, WorkspaceStateScope};

impl From<workspace_state::Model> for WorkspaceState {
    fn from(m: workspace_state::Model) -> Self {
        let scope = match m.scope.as_str() {
            "server" => WorkspaceStateScope::Server,
            "user" => WorkspaceStateScope::User,
            other => {
                tracing::warn!("unknown workspace_state scope: {other}, defaulting to Server");
                WorkspaceStateScope::Server
            }
        };
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            key: m.key,
            value: m.value,
            scope,
            user_id: m.user_id.map(Into::into),
            created_by: m.created_by.into(),
            updated_by: m.updated_by.into(),
            version: m.version,
            expires_at: m.expires_at.map(|dt| dt.into()),
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
