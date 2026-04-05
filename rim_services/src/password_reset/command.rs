#[derive(Debug, Clone)]
pub struct RequestPasswordResetCmd {
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct ResetPasswordCmd {
    pub token: String,
    pub new_password: String,
}
