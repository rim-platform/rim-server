use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum AuthIntent {
    SetSession {
        session_token: String,
        csrf_token: String,
    },
    ClearSession,
}

/// Per-request container for auth side effects.
///
/// This is intentionally small and only stores high-level intent. The transport layer (Actix)
/// formats cookies and applies flags (Secure/Max-Age/etc.).
#[derive(Default)]
pub struct AuthIntentCtx {
    intent: Mutex<Option<AuthIntent>>,
}

impl AuthIntentCtx {
    pub fn set(&self, intent: AuthIntent) {
        *self.intent.lock().unwrap() = Some(intent);
    }

    pub fn take(&self) -> Option<AuthIntent> {
        self.intent.lock().unwrap().take()
    }
}

/// Inserts a new per-request `AuthIntentCtx` into the GraphQL request data.
///
/// Returns the updated request and the same context as an `Arc` so the transport layer can read
/// and consume the intent after execution.
pub fn insert_auth_intent_ctx(
    req: async_graphql::Request,
) -> (async_graphql::Request, Arc<AuthIntentCtx>) {
    let ctx = Arc::new(AuthIntentCtx::default());
    (req.data(Arc::clone(&ctx)), ctx)
}
