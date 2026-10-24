use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum AuthIntent {
    SetSession {
        session_token: String,
        csrf_token: String,
        /// "Keep me signed in": persistent cookies. Otherwise browser-session cookies, which
        /// the browser drops when it closes.
        kept: bool,
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
        *self.intent.lock().unwrap_or_else(|e| e.into_inner()) = Some(intent);
    }

    pub fn take(&self) -> Option<AuthIntent> {
        self.intent.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}
