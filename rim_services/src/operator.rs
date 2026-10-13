//! Instance operators: the accounts that run this server's moderation queue from the client,
//! and who runs an operator's action.

use std::fmt;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::relay::dto::AccountSummaryDto;
use crate::{Actor, ServiceResult};

/// Who runs an instance operator's action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorCaller {
    /// The `rim_server` command line, run by whoever runs the server.
    CommandLine,
    /// A signed-in user; the action is refused unless they are an instance operator.
    User(Actor),
}

impl fmt::Display for OperatorCaller {
    /// `cli`, or the user's id, as the audit line's `actor_id`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandLine => f.write_str("cli"),
            Self::User(actor) => write!(f, "{}", actor.user_id),
        }
    }
}

/// What granting or revoking the operator role did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorChangeDto {
    pub account: AccountSummaryDto,
    /// `false` when the account already held, or already lacked, the role.
    pub changed: bool,
}

/// Grants and checks of the instance operator role.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait InstanceOperatorService: Send + Sync {
    /// Whether the actor is an instance operator.
    async fn is_operator(&self, actor: Actor) -> ServiceResult<bool>;

    /// Make the account named `username` an instance operator. For the command line only.
    ///
    /// # Errors
    /// `NotFound` for no such account; `Validation` when the account is blocked or scheduled
    /// for deletion.
    async fn grant(&self, username: String) -> ServiceResult<OperatorChangeDto>;

    /// Take the instance operator role from the account named `username`. For the command
    /// line only.
    ///
    /// # Errors
    /// `NotFound` for no such account.
    async fn revoke(&self, username: String) -> ServiceResult<OperatorChangeDto>;

    /// The instance operators, by username.
    async fn operators(&self) -> ServiceResult<Vec<AccountSummaryDto>>;
}
