use std::error::Error as StdError;

use rim_core::repository::error::{RepoError, RepoErrorKind};
use sea_orm::DbErr;

fn classify_sqlx_error(err: &sqlx_core::error::Error) -> (RepoErrorKind, Option<String>) {
    use sqlx_core::error::Error as E;

    match err {
        E::RowNotFound => (RepoErrorKind::NotFound, None),
        E::PoolTimedOut => (RepoErrorKind::Timeout, None),
        E::PoolClosed => (RepoErrorKind::Unavailable, None),
        E::Io(_) => (RepoErrorKind::Unavailable, None),
        E::Tls(_) => (RepoErrorKind::Unavailable, None),
        E::Decode(_) => (RepoErrorKind::InvalidData, None),
        E::ColumnDecode { .. } => (RepoErrorKind::InvalidData, None),
        E::ColumnNotFound(_) => (RepoErrorKind::InvalidData, None),
        E::TypeNotFound { .. } => (RepoErrorKind::InvalidData, None),
        E::ColumnIndexOutOfBounds { .. } => (RepoErrorKind::InvalidData, None),

        E::Database(db_err) => {
            // Portable, DB-agnostic mapping.
            if let Some(constraint) = db_err.constraint() {
                return (
                    RepoErrorKind::Conflict,
                    Some(format!("constraint violation: {constraint}")),
                );
            }

            if let Some(code) = db_err.code() {
                return (
                    RepoErrorKind::Conflict,
                    Some(format!("database error code: {code}")),
                );
            }

            (RepoErrorKind::Internal, None)
        }

        E::Configuration(_) => (RepoErrorKind::Internal, None),
        E::InvalidArgument(_) => (RepoErrorKind::Internal, None),
        E::Protocol(_) => (RepoErrorKind::Unavailable, None),
        E::WorkerCrashed => (RepoErrorKind::Unavailable, None),
        E::AnyDriverError(_) => (RepoErrorKind::Internal, None),
        E::BeginFailed => (RepoErrorKind::Unavailable, None),
        E::InvalidSavePointStatement => (RepoErrorKind::Internal, None),
        E::Encode(_) => (RepoErrorKind::Internal, None),

        // `sqlx_core::error::Error` is non-exhaustive.
        _ => (RepoErrorKind::Internal, None),
    }
}

fn classify_error_chain(err: &(dyn StdError + 'static)) -> Option<(RepoErrorKind, Option<String>)> {
    let mut cur: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(e) = cur {
        // SeaORM `DbErr` often wraps the SQLx error in its Display, but the StdError chain may
        // contain an intermediate `sea_orm::SqlxError` wrapper. Try to detect that by walking the
        // chain and attempting downcasts at each hop.
        if let Some(db_err) = e.downcast_ref::<DbErr>() {
            // Unwrap nested SeaORM errors that commonly wrap SQLx.
            cur = db_err.source();
            continue;
        }
        if let Some(sqlx_wrapper) = e.downcast_ref::<sea_orm::SqlxError>() {
            cur = sqlx_wrapper.source();
            continue;
        }
        if let Some(sqlx_err) = e.downcast_ref::<sqlx_core::error::Error>() {
            return Some(classify_sqlx_error(sqlx_err));
        }
        cur = e.source();
    }
    None
}

fn classify_db_err_by_display(err: &DbErr) -> Option<(RepoErrorKind, Option<String>)> {
    // Fallback for cases where the error chain doesn't expose `sqlx_core::Error` as a concrete
    // type to downcast to.
    let s = err.to_string();
    if s.contains("violates foreign key constraint") {
        return Some((RepoErrorKind::Conflict, None));
    }
    if s.contains("violates unique constraint") {
        return Some((RepoErrorKind::Conflict, None));
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

    // Second: try to map based on the wrapped SQLx error (portable, DB-agnostic).
    let (kind, message) =
        match classify_error_chain(&err).or_else(|| classify_db_err_by_display(&err)) {
            Some((kind, msg)) => (kind, msg),
            None => (base_kind.unwrap_or(RepoErrorKind::Internal), None),
        };

    if let Some(message) = message {
        RepoError::with_message_and_source(kind, message, err)
    } else {
        RepoError::with_source(kind, err)
    }
}
