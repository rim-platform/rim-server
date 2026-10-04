use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement, TransactionTrait,
};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared transaction state that allows multiple repos to participate in the
/// same database transaction while keeping commit/rollback independent of
/// reference counting.
///
/// The inner `Option` is `Some` while the transaction is active. Calling
/// [`SharedTransaction::commit`] takes the transaction out (setting it to
/// `None`) and commits it. After that, any further queries through handles
/// that reference this transaction will fail with a clear error.
///
/// If the `SharedTransaction` is dropped without committing, the inner
/// `DatabaseTransaction` is dropped too, which triggers an automatic rollback
/// (SeaORM default behavior).
pub struct SharedTransaction {
    inner: Mutex<Option<DatabaseTransaction>>,
    backend: DbBackend,
}

impl std::fmt::Debug for SharedTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedTransaction")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl SharedTransaction {
    fn new(txn: DatabaseTransaction) -> Self {
        let backend = txn.get_database_backend();
        Self {
            inner: Mutex::new(Some(txn)),
            backend,
        }
    }

    /// Commit the transaction. Can be called even when other `DbHandle` clones
    /// still reference this `SharedTransaction` — they will receive an error on
    /// subsequent queries.
    pub async fn commit(&self) -> Result<(), DbErr> {
        let txn =
            self.inner.lock().await.take().ok_or_else(|| {
                DbErr::Custom("transaction already committed or rolled back".into())
            })?;
        txn.commit().await
    }
}

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
    /// `Arc<SharedTransaction>` so that all their operations participate in
    /// a single atomic commit.
    Transaction(Arc<SharedTransaction>),
}

macro_rules! txn_op {
    ($shared:expr, $method:ident, $($arg:expr),*) => {{
        let guard = $shared.inner.lock().await;
        let txn = guard.as_ref().ok_or_else(|| {
            DbErr::Custom("transaction already committed or rolled back".into())
        })?;
        txn.$method($($arg),*).await
    }};
}

#[async_trait::async_trait]
impl ConnectionTrait for DbHandle {
    fn get_database_backend(&self) -> DbBackend {
        match self {
            DbHandle::Connection(c) => c.get_database_backend(),
            DbHandle::Transaction(t) => t.backend,
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            DbHandle::Connection(c) => c.execute_raw(stmt).await,
            DbHandle::Transaction(shared) => txn_op!(shared, execute_raw, stmt),
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            DbHandle::Connection(c) => c.execute_unprepared(sql).await,
            DbHandle::Transaction(shared) => txn_op!(shared, execute_unprepared, sql),
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            DbHandle::Connection(c) => c.query_one_raw(stmt).await,
            DbHandle::Transaction(shared) => txn_op!(shared, query_one_raw, stmt),
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            DbHandle::Connection(c) => c.query_all_raw(stmt).await,
            DbHandle::Transaction(shared) => txn_op!(shared, query_all_raw, stmt),
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
            DbHandle::Transaction(shared) => {
                let guard = shared.inner.lock().await;
                let txn =
                    guard
                        .as_ref()
                        .ok_or(sea_orm::TransactionError::Connection(DbErr::Custom(
                            "transaction already committed or rolled back".into(),
                        )))?;
                txn.transaction(callback).await
            }
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
    shared: Arc<SharedTransaction>,
}

impl TransactionScope {
    /// Get the [`DbHandle`] for constructing repos that share this transaction.
    pub fn handle(&self) -> DbHandle {
        DbHandle::Transaction(Arc::clone(&self.shared))
    }

    /// Commit the transaction. This works regardless of how many `DbHandle`
    /// clones still reference the underlying transaction — they will simply
    /// receive an error on any subsequent query.
    pub async fn commit(self) -> Result<(), DbErr> {
        self.shared.commit().await
    }
}

/// Start a new database transaction from a [`DatabaseConnection`].
///
/// Returns a [`TransactionScope`] whose [`DbHandle`] can be used to construct
/// repos that all participate in the same transaction.
pub async fn begin_transaction(db: &DatabaseConnection) -> Result<TransactionScope, DbErr> {
    let txn = db.begin().await?;
    Ok(TransactionScope {
        shared: Arc::new(SharedTransaction::new(txn)),
    })
}
