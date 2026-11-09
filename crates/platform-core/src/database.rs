//! SQLite lives on dedicated project threads. HTTP cancellation never replays a write.
use crate::error::ApiError;
use crate::projects::ProjectHandle;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::{Value as DbValue, ValueRef},
    Connection, OpenFlags,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, OwnedRwLockReadGuard};

pub const JSON_BODY_MAX_BYTES: usize = 2_000_000;
pub const SQL_RESULT_MAX_BYTES: usize = 8_000_000;
pub const SQL_DEFAULT_MAX_ROWS: usize = 1_000;
pub const SQL_DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const TRANSACTION_MAX_STATEMENTS: usize = 100;
const QUEUE_MAX_BYTES: usize = 32_000_000;
const QUEUE_MAX_ITEMS: usize = 128;
const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);
const SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug)]
pub enum SqlCaller {
    Browser,
    Management,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqlOptions {
    pub max_rows: Option<usize>,
    pub timeout_ms: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqlRequest {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Value>,
    #[serde(default)]
    pub options: SqlOptions,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqlStatement {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionRequest {
    pub statements: Vec<SqlStatement>,
    #[serde(default)]
    pub options: SqlOptions,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SqlColumn {
    pub name: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlResult {
    pub columns: Vec<SqlColumn>,
    pub rows: Vec<Map<String, Value>>,
    pub changes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_insert_rowid: Option<Value>,
    pub changed: bool,
}
pub type SchemaInfo = Value;

pub fn sql_limits(caller: SqlCaller) -> Value {
    json!({"limitsStatus":"internalTest", "jsonBodyMaxBytes":JSON_BODY_MAX_BYTES,
      "sqlDefaultMaxRows":SQL_DEFAULT_MAX_ROWS,"sqlMaxRows":match caller {SqlCaller::Browser=>1000,SqlCaller::Management=>10000},
      "sqlResultMaxBytes":SQL_RESULT_MAX_BYTES,"sqlTimeoutMs":SQL_DEFAULT_TIMEOUT_MS,
      "sqlMaxTimeoutMs":match caller {SqlCaller::Browser=>30000,SqlCaller::Management=>120000},
      "transactionMaxStatements":TRANSACTION_MAX_STATEMENTS,"fileUploadMaxBytes":256_000_000})
}
#[derive(Clone)]
pub(crate) struct Budget {
    max_rows: usize,
    timeout: Duration,
}
impl Budget {
    pub(crate) fn new(options: &SqlOptions, caller: SqlCaller) -> Result<Self, ApiError> {
        let max_rows = options.max_rows.unwrap_or(SQL_DEFAULT_MAX_ROWS);
        let timeout_ms = options.timeout_ms.unwrap_or(SQL_DEFAULT_TIMEOUT_MS);
        let (row_ceiling, time_ceiling) = match caller {
            SqlCaller::Browser => (1000, 30000),
            SqlCaller::Management => (10000, 120000),
        };
        if max_rows == 0 || max_rows > row_ceiling || timeout_ms == 0 || timeout_ms > time_ceiling {
            return Err(ApiError::new(
                "INVALID_SQL_BUDGET",
                "SQL row/time budget is outside this caller's supported range",
            )
            .outcome("not_started"));
        }
        Ok(Self {
            max_rows,
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}
#[derive(Default)]
pub(crate) struct QueueBudget(Mutex<(usize, usize)>);
impl QueueBudget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<QueueLease, ApiError> {
        let mut budget = self.0.lock().unwrap();
        if budget.0 >= QUEUE_MAX_ITEMS || budget.1.saturating_add(bytes) > QUEUE_MAX_BYTES {
            return Err(
                ApiError::new("SQL_QUEUE_FULL", "Global SQL queue is full").outcome("not_started")
            );
        }
        budget.0 += 1;
        budget.1 += bytes;
        Ok(QueueLease {
            owner: self.clone(),
            bytes,
        })
    }
}
struct QueueLease {
    owner: Arc<QueueBudget>,
    bytes: usize,
}
impl Drop for QueueLease {
    fn drop(&mut self) {
        let mut state = self.owner.0.lock().unwrap();
        state.0 -= 1;
        state.1 -= self.bytes;
    }
}

#[derive(Clone)]
pub(crate) enum Work {
    Statements(Vec<SqlStatement>, Budget),
    Schema,
}
pub(crate) enum WorkResult {
    Statements(Vec<SqlResult>),
    Schema(SchemaInfo),
}
struct Job {
    project: ProjectHandle,
    epoch: Option<String>,
    work: Work,
    queued_at: Instant,
    response: oneshot::Sender<Result<WorkResult, ApiError>>,
    queue_lease: QueueLease,
    phase: Arc<AtomicU8>,
    _operation: OwnedRwLockReadGuard<()>,
    retain_connection: bool,
}
pub(crate) struct DatabaseAdmission {
    pub(crate) retain_connection: bool,
    pub(crate) operation: OwnedRwLockReadGuard<()>,
}
pub(crate) struct DatabaseWorker {
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
    join_error: Mutex<Option<String>>,
    finished: Arc<AtomicBool>,
}
impl DatabaseWorker {
    pub(crate) fn spawn() -> Result<Self, ApiError> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(32);
        let finished = Arc::new(AtomicBool::new(false));
        let thread_finished = finished.clone();
        let thread = thread::Builder::new()
            .name("project-sqlite".into())
            .spawn(move || {
                let mut connection: Option<(std::path::PathBuf, Connection)> = None;
                loop {
                    let job = match receiver.recv_timeout(Duration::from_secs(300)) {
                        Ok(job) => job,
                        // Reclaim both the idle connection and its dedicated worker.
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let Job {
                        project,
                        epoch,
                        work,
                        queued_at,
                        response,
                        queue_lease,
                        phase,
                        _operation,
                        retain_connection,
                    } = job;
                    drop(queue_lease);
                    let result = (|| {
                        if queued_at.elapsed() > QUEUE_TIMEOUT
                            || phase
                                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                                .is_err()
                        {
                            return Err(ApiError::new(
                                "SQL_QUEUE_TIMEOUT",
                                "SQL request expired before execution",
                            )
                            .outcome("not_started"));
                        }
                        project.validate_epoch(epoch.as_deref())?;
                        let database = project.prepare_database()?;
                        if connection
                            .as_ref()
                            .is_none_or(|(path, _)| path != &database)
                        {
                            connection = Some((database.clone(), open_database(&database)?));
                        }
                        let conn = &mut connection.as_mut().unwrap().1;
                        match work {
                            Work::Statements(statements, budget) => {
                                execute_request(conn, statements, budget)
                                    .map(WorkResult::Statements)
                            }
                            Work::Schema => schema(conn).map(WorkResult::Schema),
                        }
                    })();
                    if !retain_connection {
                        connection = None;
                    }
                    // A client may have gone away. The accepted operation still resolves and releases its lease.
                    let _ = response.send(result);
                }
                thread_finished.store(true, Ordering::Release);
            })?;
        Ok(Self {
            sender: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
            join_error: Mutex::new(None),
            finished,
        })
    }
    pub(crate) async fn submit(
        &self,
        project: ProjectHandle,
        epoch: Option<String>,
        work: Work,
        queue: Arc<QueueBudget>,
        bytes: usize,
        admission: DatabaseAdmission,
    ) -> Result<WorkResult, ApiError> {
        let DatabaseAdmission {
            retain_connection,
            operation,
        } = admission;
        project.validate_epoch(epoch.as_deref())?;
        let queue_lease = queue.reserve(bytes)?;
        let (response, mut received) = oneshot::channel();
        let phase = Arc::new(AtomicU8::new(0));
        let job = Job {
            project,
            epoch,
            work,
            queued_at: Instant::now(),
            response,
            queue_lease,
            phase: phase.clone(),
            _operation: operation,
            retain_connection,
        };
        {
            let guard = self.sender.lock().unwrap();
            let sender = guard.as_ref().ok_or_else(|| {
                ApiError::new("SERVICE_STOPPING", "Database service is stopping")
                    .outcome("not_started")
            })?;
            sender.try_send(job).map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    ApiError::new("SQL_QUEUE_FULL", "Project SQL queue is full")
                        .outcome("not_started")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    ApiError::new("DATABASE_WORKER_STOPPED", "Database worker is unavailable")
                        .outcome("not_started")
                }
            })?;
        }
        let received_result = tokio::select! {
            response=&mut received=>response,
            _=tokio::time::sleep(QUEUE_TIMEOUT)=>{
                if phase.compare_exchange(0,2,Ordering::AcqRel,Ordering::Acquire).is_ok(){
                    return Err(ApiError::new("SQL_QUEUE_TIMEOUT","SQL request expired before execution").outcome("not_started"));
                }
                received.await
            }
        };
        received_result.map_err(|_| {
            ApiError::new(
                "DATABASE_WORKER_STOPPED",
                "Database worker stopped before delivering the result",
            )
            .outcome("unknown")
        })?
    }
    /// An isolated SQLite-owning thread that deliberately panics, leaving the
    /// same disconnected sender/unfinished flag shape as a real worker panic.
    #[cfg(test)]
    pub(crate) fn fixture_panicked(database: std::path::PathBuf) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Job>(32);
        let thread = thread::spawn(move || {
            let connection = open_existing(&database).unwrap();
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap();
            drop(receiver);
            panic!("controlled artificial SQLite worker panic");
        });
        // Wait for the fault, but leave the result unjoined for cache retirement.
        while !thread.is_finished() {
            thread::yield_now();
        }
        Self {
            sender: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
            join_error: Mutex::new(None),
            finished: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(crate) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub(crate) fn stop(&self) {
        self.sender.lock().unwrap().take();
    }
    pub(crate) fn join(&self) -> Result<(), String> {
        // Serialize concurrent joins through completion; None alone is not a
        // success signal while another caller is still joining the thread.
        let mut thread = self.thread.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(thread) = thread.take() {
            if thread.join().is_err() {
                *self.join_error.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some("Database worker panicked before clean shutdown".into());
            }
        }
        match self
            .join_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
impl Drop for DatabaseWorker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.join();
    }
}

#[cfg(test)]
pub(crate) fn initialize_database(path: &std::path::Path) -> Result<(), ApiError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql_error)?;
    configure(&connection)?;
    connection
        .execute_batch("PRAGMA user_version=0; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(sql_error)?;
    Ok(())
}
fn open_database(path: &std::path::Path) -> Result<Connection, ApiError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    configure(&conn)?;
    Ok(conn)
}
#[cfg(test)]
fn open_existing(path: &std::path::Path) -> Result<Connection, ApiError> {
    // READ_WRITE without CREATE is critical: losing a registered database must never recreate it.
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql_error)?;
    configure(&conn)?;
    Ok(conn)
}
fn configure(conn: &Connection) -> Result<(), ApiError> {
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(sql_error)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(sql_error)?;
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(sql_error)?;
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(sql_error)?;
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(sql_error)?;
    if journal != "wal" || foreign_keys != 1 || synchronous != 2 {
        return Err(ApiError::new(
            "DATABASE_CONFIGURATION_ERROR",
            "SQLite could not establish WAL, foreign_keys=ON and synchronous=FULL",
        ));
    }
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, SQL_RESULT_MAX_BYTES as i32)
        .map_err(sql_error)?;
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, JSON_BODY_MAX_BYTES as i32)
        .map_err(sql_error)?;
    Ok(())
}
fn sql_error(error: rusqlite::Error) -> ApiError {
    let code = match &error {
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::AuthorizationForStatementDenied =>
        {
            "SQL_NOT_ALLOWED"
        }
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            "SQL_TIMEOUT"
        }
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::DatabaseBusy
                || code.code == rusqlite::ErrorCode::DatabaseLocked =>
        {
            "SQL_BUSY"
        }
        _ => "SQL_ERROR",
    };
    ApiError::new(code, error.to_string())
}
fn authorizer(
    context: AuthContext<'_>,
    insert: &AtomicBool,
    altering: &Mutex<Option<(String, String)>>,
) -> Authorization {
    use AuthAction::*;
    let allowed = match context.action {
        Transaction { .. } | Savepoint { .. } | Attach { .. } | Detach { .. } | Unknown { .. } => {
            false
        }
        Function { function_name } => !matches!(
            function_name.to_ascii_lowercase().as_str(),
            "load_extension" | "readfile" | "writefile" | "edit"
        ),
        AlterTable {
            database_name,
            table_name,
        } => {
            *altering.lock().unwrap_or_else(|poison| poison.into_inner()) =
                Some((database_name.into(), table_name.into()));
            true
        }
        Pragma {
            pragma_name,
            pragma_value,
        } => {
            let name = pragma_name.to_ascii_lowercase();
            // ADD COLUMN with CHECK/STRICT constraints makes SQLite issue its
            // own pragma_quick_check(table, database). Permit only the exact
            // ALTER target of this statement; callers still cannot set pragmas.
            let checking_altered_table = name == "quick_check"
                && pragma_value.is_some_and(|table| {
                    altering
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .as_ref()
                        .is_some_and(|(database, expected)| {
                            expected == table && context.database_name == Some(database.as_str())
                        })
                });
            matches!(
                name.as_str(),
                "table_info"
                    | "table_xinfo"
                    | "foreign_key_list"
                    | "index_list"
                    | "index_info"
                    | "index_xinfo"
            ) || checking_altered_table
                || (pragma_value.is_none()
                    && matches!(
                        name.as_str(),
                        "foreign_keys"
                            | "journal_mode"
                            | "synchronous"
                            | "busy_timeout"
                            | "user_version"
                            | "schema_version"
                            | "page_count"
                            | "page_size"
                            | "freelist_count"
                            | "integrity_check"
                            | "quick_check"
                            | "database_list"
                    ))
        }
        // Temp objects would create hidden cross-request state and bypass project persistence.
        CreateTempTable { .. }
        | CreateTempIndex { .. }
        | CreateTempTrigger { .. }
        | CreateTempView { .. }
        | DropTempTable { .. }
        | DropTempIndex { .. }
        | DropTempTrigger { .. }
        | DropTempView { .. } => false,
        CreateVtable { module_name, .. } => matches!(
            module_name.to_ascii_lowercase().as_str(),
            "fts5" | "fts4" | "fts3" | "rtree" | "rtree_i32"
        ),
        Insert { table_name } => {
            if !table_name.starts_with("sqlite_") {
                insert.store(true, Ordering::Relaxed);
            }
            true
        }
        _ => true,
    };
    if allowed {
        Authorization::Allow
    } else {
        Authorization::Deny
    }
}
fn execute_request(
    conn: &mut Connection,
    statements: Vec<SqlStatement>,
    budget: Budget,
) -> Result<Vec<SqlResult>, ApiError> {
    let deadline = Instant::now() + budget.timeout;
    conn.progress_handler(1000, Some(move || Instant::now() >= deadline))
        .map_err(sql_error)?;
    let begin = conn.execute_batch("BEGIN DEFERRED").map_err(sql_error);
    if let Err(error) = begin {
        let _ = conn.progress_handler(0, None::<fn() -> bool>);
        return Err(error.outcome("not_started"));
    }
    let inserted = Arc::new(AtomicBool::new(false));
    let insert_flag = inserted.clone();
    let altering = Arc::new(Mutex::new(None));
    let alter_flag = altering.clone();
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        authorizer(context, &insert_flag, &alter_flag)
    }))
    .map_err(sql_error)?;
    let result = (|| {
        let mut results = Vec::new();
        let mut rows_left = budget.max_rows;
        let mut bytes = 2usize;
        for statement in statements {
            if Instant::now() >= deadline {
                return Err(ApiError::new("SQL_TIMEOUT", "SQL execution budget expired"));
            }
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            conn.busy_timeout(remaining).map_err(sql_error)?;
            inserted.store(false, Ordering::Relaxed);
            *altering.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
            let params = statement
                .params
                .iter()
                .map(decode_value)
                .collect::<Result<Vec<_>, _>>()?;
            let mut stmt = conn.prepare(&statement.sql).map_err(sql_error)?;
            let readonly = stmt.readonly();
            let columns = stmt
                .column_names()
                .into_iter()
                .map(|name| SqlColumn {
                    name: name.to_owned(),
                })
                .collect::<Vec<_>>();
            let mut names = HashSet::new();
            if columns
                .iter()
                .any(|column| !names.insert(column.name.clone()))
            {
                return Err(ApiError::new(
                    "SQL_DUPLICATE_COLUMN",
                    "Result columns must have distinct names; use SQL aliases",
                ));
            }
            let changes_before = conn.total_changes();
            let mut rows = Vec::new();
            {
                let mut cursor = stmt
                    .query(rusqlite::params_from_iter(params))
                    .map_err(sql_error)?;
                while let Some(row) = cursor.next().map_err(sql_error)? {
                    if rows_left == 0 {
                        return Err(limit_error("rows", budget.max_rows));
                    }
                    let mut record = Map::new();
                    for (index, column) in columns.iter().enumerate() {
                        record.insert(
                            column.name.clone(),
                            encode_value(row.get_ref(index).map_err(sql_error)?)?,
                        );
                    }
                    bytes = bytes.saturating_add(serde_json::to_vec(&record)?.len() + 1);
                    if bytes > SQL_RESULT_MAX_BYTES {
                        return Err(limit_error("bytes", SQL_RESULT_MAX_BYTES));
                    }
                    rows_left -= 1;
                    rows.push(record);
                    if Instant::now() >= deadline {
                        return Err(ApiError::new("SQL_TIMEOUT", "SQL execution budget expired"));
                    }
                }
            }
            let changes = if readonly {
                0
            } else {
                conn.total_changes().saturating_sub(changes_before)
            };
            let result = SqlResult {
                columns,
                rows,
                changes,
                last_insert_rowid: if inserted.load(Ordering::Relaxed) && changes > 0 {
                    Some(encode_integer(conn.last_insert_rowid()))
                } else {
                    None
                },
                changed: !readonly,
            };
            // Count exact result object overhead too; the row loop above prevents unbounded accumulation.
            let encoded = serde_json::to_vec(&result)?.len();
            let row_bytes = result.rows.iter().try_fold(0usize, |sum, row| {
                Ok::<_, serde_json::Error>(sum + serde_json::to_vec(row)?.len() + 1)
            })?;
            bytes = bytes.saturating_add(encoded.saturating_sub(row_bytes) + 1);
            if bytes > SQL_RESULT_MAX_BYTES {
                return Err(limit_error("bytes", SQL_RESULT_MAX_BYTES));
            }
            results.push(result);
        }
        if Instant::now() >= deadline {
            return Err(ApiError::new("SQL_TIMEOUT", "SQL execution budget expired"));
        }
        Ok(results)
    })();
    // Remove hooks before our transaction controls. No delayed interrupt can affect the next job.
    let _ = conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let _ = conn.busy_timeout(
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(5)),
    );
    let result = match result {
        Ok(results) => match conn.execute_batch("COMMIT") {
            Ok(()) => Ok(results),
            Err(error) => {
                let _ = conn.progress_handler(0, None::<fn() -> bool>);
                let rollback = conn.execute_batch("ROLLBACK");
                Err(
                    sql_error(error).outcome(if rollback.is_ok() || conn.is_autocommit() {
                        "rolled_back"
                    } else {
                        "unknown"
                    }),
                )
            }
        },
        Err(mut error) => {
            let _ = conn.progress_handler(0, None::<fn() -> bool>);
            let rollback = conn.execute_batch("ROLLBACK");
            if Instant::now() >= deadline && error.code == "SQL_BUSY" {
                error.code = "SQL_TIMEOUT".into();
                error.message =
                    "SQL execution budget expired while waiting for a database lock".into();
            }
            Err(error.outcome(if rollback.is_ok() || conn.is_autocommit() {
                "rolled_back"
            } else {
                "unknown"
            }))
        }
    };
    let _ = conn.progress_handler(0, None::<fn() -> bool>);
    let _ = conn.busy_timeout(Duration::from_secs(5));
    result
}
fn limit_error(kind: &str, limit: usize) -> ApiError {
    ApiError::new(
        "SQL_RESULT_LIMIT_EXCEEDED",
        "SQL result exceeds the request budget; use LIMIT and pagination",
    )
    .detail("limitKind", kind)
    .detail("limit", limit)
}
fn decode_value(value: &Value) -> Result<DbValue, ApiError> {
    let invalid = || {
        ApiError::new("INVALID_SQL_VALUE","Expected null, boolean, string, a finite safe number, {$int64:string}, or {$bytes:base64}")
    };
    match value {
        Value::Null => Ok(DbValue::Null),
        Value::Bool(v) => Ok(DbValue::Integer(i64::from(*v))),
        Value::String(v) => Ok(DbValue::Text(v.clone())),
        Value::Number(v) => {
            if let Some(integer) = v.as_i64() {
                if integer.abs_diff(0) > SAFE_INTEGER as u64 {
                    return Err(invalid());
                }
                return Ok(DbValue::Integer(integer));
            }
            if v.is_u64() {
                return Err(invalid());
            }
            v.as_f64()
                .filter(|n| n.is_finite())
                .map(DbValue::Real)
                .ok_or_else(invalid)
        }
        Value::Object(v) if v.len() == 1 => {
            if let Some(Value::String(integer)) = v.get("$int64") {
                if integer.is_empty() || integer.trim() != integer {
                    return Err(invalid());
                }
                return integer
                    .parse::<i64>()
                    .map(DbValue::Integer)
                    .map_err(|_| invalid());
            }
            if let Some(Value::String(bytes)) = v.get("$bytes") {
                return STANDARD
                    .decode(bytes)
                    .map(DbValue::Blob)
                    .map_err(|_| invalid());
            }
            Err(invalid())
        }
        _ => Err(invalid()),
    }
}
fn encode_integer(integer: i64) -> Value {
    if (-SAFE_INTEGER..=SAFE_INTEGER).contains(&integer) {
        json!(integer)
    } else {
        json!({"$int64":integer.to_string()})
    }
}
fn encode_value(value: ValueRef<'_>) -> Result<Value, ApiError> {
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(v) => Ok(encode_integer(v)),
        ValueRef::Real(v) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| {
                ApiError::new(
                    "SQL_VALUE_NOT_SUPPORTED",
                    "Non-finite SQLite numbers cannot be represented in JSON",
                )
            }),
        ValueRef::Text(v) => String::from_utf8(v.to_vec())
            .map(Value::String)
            .map_err(|_| {
                ApiError::new(
                    "SQL_VALUE_NOT_SUPPORTED",
                    "SQLite text is not valid UTF-8; cast it to BLOB to read bytes",
                )
            }),
        ValueRef::Blob(v) => Ok(json!({"$bytes":STANDARD.encode(v)})),
    }
}
fn schema(conn: &mut Connection) -> Result<SchemaInfo, ApiError> {
    // One managed read transaction gives one snapshot and one shared row/byte/time budget.
    let queries=[
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        "SELECT s.name AS objectName,p.* FROM sqlite_schema s JOIN pragma_table_xinfo(s.name) p WHERE s.type IN ('table','view') AND s.name NOT LIKE 'sqlite_%' ORDER BY s.name,p.cid",
        "SELECT s.name AS objectName,p.* FROM sqlite_schema s JOIN pragma_foreign_key_list(s.name) p WHERE s.type='table' AND s.name NOT LIKE 'sqlite_%' ORDER BY s.name,p.id,p.seq",
        "SELECT s.name AS objectName,p.* FROM sqlite_schema s JOIN pragma_index_list(s.name) p WHERE s.type='table' AND s.name NOT LIKE 'sqlite_%' ORDER BY s.name,p.seq",
        "SELECT s.name AS objectName,p.* FROM sqlite_schema s JOIN pragma_index_xinfo(s.name) p WHERE s.type='index' ORDER BY s.name,p.seqno",
    ];
    let results = execute_request(
        conn,
        queries
            .into_iter()
            .map(|sql| SqlStatement {
                sql: sql.into(),
                params: vec![],
            })
            .collect(),
        Budget::new(
            &SqlOptions {
                max_rows: Some(10000),
                timeout_ms: None,
            },
            SqlCaller::Management,
        )?,
    )?;
    let mut tables = Vec::new();
    let mut indexes = Vec::new();
    let mut views = Vec::new();
    let mut triggers = Vec::new();
    let group = |index: usize, name: &str| {
        results[index]
            .rows
            .iter()
            .filter(|row| row["objectName"] == name)
            .map(|row| {
                let mut row = row.clone();
                row.remove("objectName");
                row
            })
            .collect::<Vec<_>>()
    };
    for object in &results[0].rows {
        let name = object["name"].as_str().unwrap_or("");
        let mut record = object.clone();
        match object["type"].as_str() {
            Some("table") => {
                record.insert("columns".into(), json!(group(1, name)));
                record.insert("foreignKeys".into(), json!(group(2, name)));
                let mut table_indexes = group(3, name);
                for index in &mut table_indexes {
                    let columns = group(4, index["name"].as_str().unwrap_or(""));
                    index.insert("columns".into(), json!(columns));
                }
                record.insert("indexes".into(), json!(table_indexes));
                tables.push(record);
            }
            Some("index") => {
                record.insert("columns".into(), json!(group(4, name)));
                indexes.push(record);
            }
            Some("view") => {
                record.insert("columns".into(), json!(group(1, name)));
                views.push(record);
            }
            Some("trigger") => triggers.push(record),
            _ => (),
        }
    }
    let value = json!({"tables":tables,"indexes":indexes,"views":views,"triggers":triggers});
    if serde_json::to_vec(&value)?.len() > SQL_RESULT_MAX_BYTES {
        return Err(limit_error("bytes", SQL_RESULT_MAX_BYTES).outcome("no_change"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_queue_budget_counts_items_and_payload_until_released() {
        let queue = Arc::new(QueueBudget::default());
        let bytes = queue.reserve(QUEUE_MAX_BYTES).unwrap();
        assert_eq!(queue.reserve(1).err().unwrap().code, "SQL_QUEUE_FULL");
        drop(bytes);
        let leases = (0..QUEUE_MAX_ITEMS)
            .map(|_| queue.reserve(0).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            queue.reserve(0).err().unwrap().details["operationOutcome"],
            "not_started"
        );
        drop(leases);
        assert!(queue.reserve(1).is_ok());
    }
    #[test]
    fn result_byte_limit_rolls_back_returning_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.sqlite");
        initialize_database(&path).unwrap();
        let mut conn = open_existing(&path).unwrap();
        let budget = Budget::new(&SqlOptions::default(), SqlCaller::Management).unwrap();
        execute_request(
            &mut conn,
            vec![SqlStatement {
                sql: "CREATE TABLE blobs(value BLOB)".into(),
                params: vec![],
            }],
            budget.clone(),
        )
        .unwrap();
        let error = execute_request(
            &mut conn,
            vec![SqlStatement {
                sql: "INSERT INTO blobs(value) VALUES(zeroblob(6000000)) RETURNING value".into(),
                params: vec![],
            }],
            budget.clone(),
        )
        .unwrap_err();
        assert_eq!(error.code, "SQL_RESULT_LIMIT_EXCEEDED");
        assert_eq!(error.details["operationOutcome"], "rolled_back");
        assert_eq!(
            execute_request(
                &mut conn,
                vec![SqlStatement {
                    sql: "SELECT count(*) AS count FROM blobs".into(),
                    params: vec![]
                }],
                budget
            )
            .unwrap()[0]
                .rows[0]["count"],
            0
        );
    }
}
