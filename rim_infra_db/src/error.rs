use rim_core::repository::error::{RepoError, RepoErrorKind};
use sea_orm::DbErr;

fn classify_db_err_by_display(err: &DbErr) -> Option<RepoErrorKind> {
    // SeaORM keeps the driver error out of the error source chain, so constraint violations are
    // recognised by the message Postgres gives them.
    let s = err.to_string();
    if s.contains("violates foreign key constraint") {
        return Some(RepoErrorKind::Conflict);
    }
    if s.contains("violates unique constraint") {
        return Some(RepoErrorKind::Conflict);
    }
    None
}

pub fn map_db_err(err: DbErr) -> RepoError {
    tracing::error!(db_err = %err, "sea_orm db error");
    // First: map SeaORM variants that are meaningful across DBs.
    let base_kind = match &err {
        DbErr::RecordNotFound(_) => Some(RepoErrorKind::NotFound),
        DbErr::RecordNotInserted => Some(RepoErrorKind::Conflict),
        DbErr::RecordNotUpdated => Some(RepoErrorKind::NotFound),
        DbErr::BackendNotSupported { .. } => Some(RepoErrorKind::Unsupported),
        DbErr::Type(_) | DbErr::Json(_) | DbErr::TryIntoErr { .. } => {
            Some(RepoErrorKind::InvalidData)
        }
        DbErr::ConnectionAcquire(_) => Some(RepoErrorKind::Timeout),
        DbErr::Conn(_) | DbErr::Exec(_) | DbErr::Query(_) => None,
        _ => None,
    };

    // Second: constraint violations, recognised by their message.
    let kind = classify_db_err_by_display(&err)
        .or(base_kind)
        .unwrap_or(RepoErrorKind::Internal);
    RepoError::with_source(kind, err)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error as StdError;
    use std::fmt;
    use std::sync::Arc;

    use sea_orm::sqlx::error::{DatabaseError, ErrorKind};
    use sea_orm::{RuntimeErr, SqlxError};

    use super::*;

    /// A database error as the driver reports it: a Postgres message and its SQLSTATE code.
    #[derive(Debug)]
    struct PgError {
        message: &'static str,
        code: &'static str,
    }

    impl fmt::Display for PgError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.message)
        }
    }

    impl StdError for PgError {}

    impl DatabaseError for PgError {
        fn message(&self) -> &str {
            self.message
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
        }

        fn constraint(&self) -> Option<&str> {
            Some("a_constraint")
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            match self.code {
                "23505" => ErrorKind::UniqueViolation,
                "23503" => ErrorKind::ForeignKeyViolation,
                "23514" => ErrorKind::CheckViolation,
                _ => ErrorKind::Other,
            }
        }
    }

    fn database(message: &'static str, code: &'static str) -> SqlxError {
        SqlxError::Database(Box::new(PgError { message, code }))
    }

    fn kind_of(err: DbErr) -> RepoErrorKind {
        map_db_err(err).kind()
    }

    #[test]
    fn driver_errors_without_a_constraint_message_are_internal() {
        assert_eq!(
            kind_of(DbErr::Query(RuntimeErr::SqlxError(Arc::new(
                SqlxError::RowNotFound
            )))),
            RepoErrorKind::Internal
        );
        assert_eq!(
            kind_of(DbErr::Exec(RuntimeErr::SqlxError(Arc::new(
                SqlxError::PoolTimedOut
            )))),
            RepoErrorKind::Internal
        );
        assert_eq!(
            kind_of(DbErr::Conn(RuntimeErr::SqlxError(Arc::new(
                SqlxError::PoolClosed
            )))),
            RepoErrorKind::Internal
        );
        assert_eq!(
            kind_of(DbErr::Exec(RuntimeErr::SqlxError(Arc::new(database(
                "new row for relation \"users\" violates check constraint \"a_constraint\"",
                "23514",
            ))))),
            RepoErrorKind::Internal
        );
    }

    #[test]
    fn unique_and_foreign_key_violations_are_conflicts() {
        assert_eq!(
            kind_of(DbErr::Exec(RuntimeErr::SqlxError(Arc::new(database(
                "duplicate key value violates unique constraint \"a_constraint\"",
                "23505",
            ))))),
            RepoErrorKind::Conflict
        );
        assert_eq!(
            kind_of(DbErr::Query(RuntimeErr::SqlxError(Arc::new(database(
                "insert or update on table \"members\" violates foreign key constraint \
                 \"a_constraint\"",
                "23503",
            ))))),
            RepoErrorKind::Conflict
        );
    }

    #[test]
    fn seaorm_variants_keep_their_kinds() {
        assert_eq!(
            kind_of(DbErr::RecordNotFound("user".into())),
            RepoErrorKind::NotFound
        );
        assert_eq!(kind_of(DbErr::RecordNotInserted), RepoErrorKind::Conflict);
        assert_eq!(kind_of(DbErr::RecordNotUpdated), RepoErrorKind::NotFound);
        assert_eq!(
            kind_of(DbErr::Type("bad".into())),
            RepoErrorKind::InvalidData
        );
    }
}
