#[derive(Debug, Clone)]
pub struct RequestPasswordResetCmd {
    pub username: String,
    /// Caller's IP, for the per-IP request limit.
    pub remote_ip: String,
}

#[derive(Debug, Clone)]
pub struct ResetPasswordCmd {
    pub token: String,
    pub new_password: String,
}
