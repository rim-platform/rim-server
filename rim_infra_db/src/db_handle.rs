use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement, TransactionTrait,
};
use std::sync::Arc;

/// A clone-friendly wrapper around either a plain database connection (pool)
/// or a shared database transaction. All SeaORM repository structs store a
/// `DbHandle` so that services can run multiple repos inside the same
/// transaction when needed.
#[derive(Clone, Debug)]
pub enum DbHandle {
    /// Normal connection backed by the pool. Each query acquires its own
    /// connection from the pool.
    Connection(DatabaseConnection),
    /// A shared transaction. Multiple repos can be constructed from the same
    /// `Arc<DatabaseTransaction>` so that all their operations participate in
    /// a single atomic commit.
    Transaction(Arc<DatabaseTransaction>),
}

#[async_trait::async_trait]
impl ConnectionTrait for DbHandle {
    fn get_database_backend(&self) -> DbBackend {
        match self {
            DbHandle::Connection(c) => c.get_database_backend(),
            DbHandle::Transaction(t) => t.get_database_backend(),
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            DbHandle::Connection(c) => c.execute_raw(stmt).await,
            DbHandle::Transaction(t) => t.execute_raw(stmt).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            DbHandle::Connection(c) => c.execute_unprepared(sql).await,
            DbHandle::Transaction(t) => t.execute_unprepared(sql).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            DbHandle::Connection(c) => c.query_one_raw(stmt).await,
            DbHandle::Transaction(t) => t.query_one_raw(stmt).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            DbHandle::Connection(c) => c.query_all_raw(stmt).await,
            DbHandle::Transaction(t) => t.query_all_raw(stmt).await,
        }
    }
}

impl DbHandle {
    /// Run a closure inside a transaction. If this handle is already a
    /// transaction, a savepoint is used.
    pub async fn transaction<F, T, E>(&self, callback: F) -> Result<T, sea_orm::TransactionError<E>>
    where
        F: for<'c> FnOnce(
                &'c DatabaseTransaction,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'c>,
            > + Send,
        T: Send,
        E: std::error::Error + Send,
    {
        match self {
            DbHandle::Connection(c) => c.transaction(callback).await,
            DbHandle::Transaction(t) => t.as_ref().transaction(callback).await,
        }
    }
}

impl From<DatabaseConnection> for DbHandle {
    fn from(conn: DatabaseConnection) -> Self {
        DbHandle::Connection(conn)
    }
}

/// A transaction scope that can be committed or rolled back. Created via
/// [`begin_transaction`].
///
/// If dropped without calling [`TransactionScope::commit`], the transaction
/// is rolled back automatically (SeaORM default behavior).
pub struct TransactionScope {
    handle: DbHandle,
}

impl TransactionScope {
    /// Get the [`DbHandle`] for constructing repos that share this transaction.
    pub fn handle(&self) -> &DbHandle {
        &self.handle
    }

    /// Commit the transaction. Consumes `self`.
    pub async fn commit(self) -> Result<(), DbErr> {
        match self.handle {
            DbHandle::Transaction(txn) => {
                Arc::try_unwrap(txn)
                    .map_err(|_| {
                        DbErr::Custom(
                            "cannot commit: transaction handle is still shared by other repos"
                                .to_string(),
                        )
                    })?
                    .commit()
                    .await
            }
            DbHandle::Connection(_) => Ok(()),
        }
    }
}

/// Start a new database transaction from a [`DatabaseConnection`].
///
/// Returns a [`TransactionScope`] whose [`DbHandle`] can be used to construct
/// repos that all participate in the same transaction.
pub async fn begin_transaction(db: &DatabaseConnection) -> Result<TransactionScope, DbErr> {
    let txn = db.begin().await?;
    Ok(TransactionScope {
        handle: DbHandle::Transaction(Arc::new(txn)),
    })
}
