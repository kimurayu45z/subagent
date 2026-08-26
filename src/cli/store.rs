//! The SQLite pair-identity metadata store: `docs/design.md` section 10.
//!
//! Scope in this build covers the `workspaces`, `supervisor_sessions`, and
//! `pairs` tables from the design's minimum schema, plus the invocation and
//! exchange ledger (`invocations`, `exchange_messages`) needed to record one
//! managed run's request/response pair transactionally. `workspace_memories`,
//! `child_sessions`, and `summaries` are not implemented yet.
//!
//! Security posture, per `docs/design.md` section 10 and section 15:
//! directories this build owns are created `0700`; the database file (and
//! its WAL/SHM/journal sidecars) are created `0600`; a symlink, a
//! non-directory/non-file, a path owned by another user, or a path with
//! group- or other-accessible permissions at any of those locations is
//! rejected rather than silently used or "fixed up" by following it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use super::id::SubagentId;
use super::pair_key::PairKey;
use super::supervisor::Provider;
use super::workspace::WorkspaceRef;

/// The on-disk ledger schema version, tracked independently of
/// [`super::pair_key::PAIR_KEY_SCHEMA_VERSION`] and
/// [`super::report::REPORT_SCHEMA_VERSION`] via `PRAGMA user_version`. A
/// mismatch means this build cannot safely interpret an existing ledger
/// file.
///
/// Version 1 introduced `workspaces`, `supervisor_sessions`, and `pairs`.
/// Version 2 additively introduces `invocations` and `exchange_messages`
/// without altering any version 1 table or row.
pub(crate) const LEDGER_SCHEMA_VERSION: i64 = 2;

const DB_FILE_NAME: &str = "ledger.sqlite3";
const BUSY_TIMEOUT_MS: u64 = 5_000;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

const SCHEMA_SQL_V1: &str = "
CREATE TABLE workspaces (
    id INTEGER PRIMARY KEY,
    canonical_path BLOB NOT NULL UNIQUE,
    identity_kind TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE supervisor_sessions (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    native_id TEXT NOT NULL,
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id),
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    UNIQUE (workspace_id, provider, native_id)
);

CREATE TABLE pairs (
    id INTEGER PRIMARY KEY,
    pair_key BLOB NOT NULL UNIQUE,
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id),
    supervisor_session_id INTEGER NOT NULL REFERENCES supervisor_sessions(id),
    subagent_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    UNIQUE (workspace_id, supervisor_session_id, subagent_id)
);

CREATE INDEX pairs_workspace_id_idx ON pairs (workspace_id);
";

/// Additive version 1 -> 2 migration: one invocation ledger row for each
/// managed run of a pair, and the request/response bodies exchanged during
/// that run. Applied both when bootstrapping a brand-new database (after
/// [`SCHEMA_SQL_V1`]) and when migrating an existing version 1 database, so
/// the two starting points always converge on the same version 2 schema.
///
/// `invocations.pair_id` cascades on delete so [`Store::delete_pair`] can
/// remove a pair's invocations (and, transitively, their exchange messages)
/// without a separate statement. `pairs.workspace_id` and
/// `pairs.supervisor_session_id` are untouched by this migration and still
/// have no cascade action, so deleting a pair never touches the workspace or
/// supervisor-session rows it referenced.
const SCHEMA_SQL_V2_ADDITIONS: &str = "
CREATE TABLE invocations (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    pair_id INTEGER NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'completed', 'spawn_failed', 'abandoned')),
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    wrapper_pid INTEGER NOT NULL CHECK (wrapper_pid > 0),
    command_digest BLOB NOT NULL CHECK (length(command_digest) = 32),
    program_name TEXT NOT NULL,
    child_kind TEXT NOT NULL CHECK (child_kind IN ('claude', 'codex')),
    exit_kind TEXT CHECK (exit_kind IS NULL OR exit_kind IN ('exited', 'signaled')),
    exit_code INTEGER,
    signal INTEGER,
    capsule_path BLOB,
    capsule_digest BLOB CHECK (capsule_digest IS NULL OR length(capsule_digest) = 32),
    context_provenance TEXT NOT NULL,
    UNIQUE (pair_id, sequence),
    CHECK (
        (status = 'pending'
            AND completed_at IS NULL
            AND exit_kind IS NULL AND exit_code IS NULL AND signal IS NULL)
        OR (status IN ('spawn_failed', 'abandoned')
            AND completed_at IS NOT NULL
            AND exit_kind IS NULL AND exit_code IS NULL AND signal IS NULL)
        OR (status = 'completed'
            AND completed_at IS NOT NULL
            AND (
                (exit_kind = 'exited' AND exit_code IS NOT NULL AND signal IS NULL)
                OR (exit_kind = 'signaled' AND signal IS NOT NULL AND exit_code IS NULL)
            ))
    )
);

CREATE INDEX invocations_pair_id_idx ON invocations (pair_id);
CREATE INDEX invocations_pair_status_sequence_idx ON invocations (pair_id, status, sequence);

CREATE TABLE exchange_messages (
    id INTEGER PRIMARY KEY,
    invocation_id TEXT NOT NULL REFERENCES invocations(id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK (direction IN ('request', 'response')),
    body BLOB NOT NULL,
    body_encoding TEXT NOT NULL CHECK (body_encoding IN ('utf8', 'bytes')),
    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    redaction_count INTEGER NOT NULL CHECK (redaction_count >= 0),
    redaction_classes TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (invocation_id, direction)
);
";

#[derive(Debug)]
pub(crate) enum StoreError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Insecure(InsecurePath),
    SchemaVersionMismatch {
        found: i64,
        expected: i64,
    },
    CorruptPairKey,
    CorruptProvider(String),
    /// No `pairs` row matches the given [`PairKey`]. Every invocation
    /// method is scoped to an already-ensured pair, so this means the
    /// caller raced a deletion or passed a stale key.
    PairNotFound,
    /// An invocation method expected exactly one matching row with
    /// `status = 'pending'` (to update or complete) and found none, either
    /// because the invocation id does not exist or because it already left
    /// the pending state.
    InvocationNotPending(String),
    CorruptInvocationStatus(String),
    CorruptChildKind(String),
    CorruptExitKind(String),
    CorruptDirection(String),
    CorruptBodyEncoding(String),
    CorruptBoolean(i64),
    CorruptRedactionCount(i64),
    CorruptRedactionClasses(String),
    CorruptDigest(&'static str),
    CorruptInvocationId(String),
    CorruptExitValue {
        field: &'static str,
        value: i64,
    },
}

#[derive(Debug)]
pub(crate) struct InsecurePath {
    pub path: PathBuf,
    pub reason: InsecureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsecureReason {
    Symlink,
    NotADirectory,
    NotAFile,
    WrongOwner,
    InsecureMode,
}

impl fmt::Display for InsecureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            InsecureReason::Symlink => "path is a symlink",
            InsecureReason::NotADirectory => "path exists but is not a directory",
            InsecureReason::NotAFile => "path exists but is not a regular file",
            InsecureReason::WrongOwner => "path is not owned by the current user",
            InsecureReason::InsecureMode => "path has group- or other-accessible permissions",
        };
        f.write_str(text)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(err) => write!(f, "state store I/O error: {err}"),
            StoreError::Sqlite(err) => write!(f, "state store database error: {err}"),
            StoreError::Insecure(insecure) => write!(
                f,
                "refusing to use insecure state path {}: {}",
                insecure.path.display(),
                insecure.reason
            ),
            StoreError::SchemaVersionMismatch { found, expected } => write!(
                f,
                "state store schema version mismatch: found {found}, expected {expected}; this \
                 build cannot read a ledger created by a different schema version"
            ),
            StoreError::CorruptPairKey => {
                write!(f, "state store contains a malformed pair key")
            }
            StoreError::CorruptProvider(text) => write!(
                f,
                "state store contains an unrecognized supervisor provider {text:?}"
            ),
            StoreError::PairNotFound => {
                write!(f, "no pair matches the given pair key")
            }
            StoreError::InvocationNotPending(invocation_id) => write!(
                f,
                "no pending invocation {invocation_id:?} found to update; it may not exist or \
                 may have already left the pending state"
            ),
            StoreError::CorruptInvocationStatus(text) => write!(
                f,
                "state store contains an unrecognized invocation status {text:?}"
            ),
            StoreError::CorruptChildKind(text) => write!(
                f,
                "state store contains an unrecognized child kind {text:?}"
            ),
            StoreError::CorruptExitKind(text) => {
                write!(f, "state store contains an unrecognized exit kind {text:?}")
            }
            StoreError::CorruptDirection(text) => write!(
                f,
                "state store contains an unrecognized exchange direction {text:?}"
            ),
            StoreError::CorruptBodyEncoding(text) => write!(
                f,
                "state store contains an unrecognized body encoding {text:?}"
            ),
            StoreError::CorruptBoolean(value) => write!(
                f,
                "state store contains a non-boolean integer {value} where a boolean flag was expected"
            ),
            StoreError::CorruptRedactionCount(value) => {
                write!(f, "state store contains a negative redaction count {value}")
            }
            StoreError::CorruptRedactionClasses(text) => write!(
                f,
                "state store contains malformed redaction classes {text:?}"
            ),
            StoreError::CorruptDigest(column) => write!(
                f,
                "state store contains a malformed {column} (expected a 32-byte digest)"
            ),
            StoreError::CorruptInvocationId(value) => write!(
                f,
                "state store contains a malformed invocation id {value:?}"
            ),
            StoreError::CorruptExitValue { field, value } => write!(
                f,
                "state store contains an out-of-range {field} value {value}"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        StoreError::Io(err)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        StoreError::Sqlite(err)
    }
}

fn insecure(path: &Path, reason: InsecureReason) -> StoreError {
    StoreError::Insecure(InsecurePath {
        path: path.to_path_buf(),
        reason,
    })
}

enum PathPresence {
    Missing,
    Ready,
}

#[cfg(unix)]
fn secure_path(
    path: &Path,
    is_dir: bool,
    required_mode: u32,
    create_if_missing: bool,
) -> Result<PathPresence, StoreError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(insecure(path, InsecureReason::Symlink));
            }
            if is_dir && !meta.is_dir() {
                return Err(insecure(path, InsecureReason::NotADirectory));
            }
            if !is_dir && !meta.is_file() {
                return Err(insecure(path, InsecureReason::NotAFile));
            }
            let current_uid: u32 = unsafe { libc::geteuid() };
            if meta.uid() != current_uid {
                return Err(insecure(path, InsecureReason::WrongOwner));
            }
            let mode: u32 = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(insecure(path, InsecureReason::InsecureMode));
            }
            Ok(PathPresence::Ready)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if !create_if_missing {
                return Ok(PathPresence::Missing);
            }
            if is_dir {
                let mut builder: std::fs::DirBuilder = std::fs::DirBuilder::new();
                builder.recursive(true).mode(required_mode).create(path)?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(required_mode))?;
            } else {
                let open_result: Result<std::fs::File, std::io::Error> =
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(required_mode)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(path);
                match open_result {
                    Ok(file) => {
                        file.set_permissions(std::fs::Permissions::from_mode(required_mode))?;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        // Another writer may have created the database after
                        // our initial metadata check. Validate the winner
                        // rather than treating a normal first-open race as a
                        // failure.
                        return secure_path(path, is_dir, required_mode, false);
                    }
                    Err(err) => return Err(StoreError::Io(err)),
                }
            }
            Ok(PathPresence::Ready)
        }
        Err(err) => Err(StoreError::Io(err)),
    }
}

#[cfg(not(unix))]
fn secure_path(
    path: &Path,
    is_dir: bool,
    _required_mode: u32,
    create_if_missing: bool,
) -> Result<PathPresence, StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(insecure(path, InsecureReason::Symlink));
            }
            if is_dir && !meta.is_dir() {
                return Err(insecure(path, InsecureReason::NotADirectory));
            }
            if !is_dir && !meta.is_file() {
                return Err(insecure(path, InsecureReason::NotAFile));
            }
            Ok(PathPresence::Ready)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if !create_if_missing {
                return Ok(PathPresence::Missing);
            }
            if is_dir {
                std::fs::create_dir_all(path)?;
            } else {
                std::fs::File::create(path)?;
            }
            Ok(PathPresence::Ready)
        }
        Err(err) => Err(StoreError::Io(err)),
    }
}

/// Re-secures the WAL/SHM/journal sidecar files SQLite creates lazily next
/// to the main database file. These are created by SQLite itself (subject
/// to the process umask), not by [`secure_path`], so they are checked and
/// tightened separately after every point where they might have appeared.
#[cfg(unix)]
fn secure_sidecars(db_path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar_name = db_path.as_os_str().to_owned();
        sidecar_name.push(suffix);
        let sidecar_path = PathBuf::from(sidecar_name);
        match std::fs::symlink_metadata(&sidecar_path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(insecure(&sidecar_path, InsecureReason::Symlink));
                }
                if !meta.is_file() {
                    return Err(insecure(&sidecar_path, InsecureReason::NotAFile));
                }
                let current_uid: u32 = unsafe { libc::geteuid() };
                if meta.uid() != current_uid {
                    return Err(insecure(&sidecar_path, InsecureReason::WrongOwner));
                }
                let mode: u32 = meta.permissions().mode() & 0o777;
                if mode != FILE_MODE {
                    std::fs::set_permissions(
                        &sidecar_path,
                        std::fs::Permissions::from_mode(FILE_MODE),
                    )?;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(StoreError::Io(err)),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn secure_sidecars(_db_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

/// Validates sidecars that predate this open without changing them. Sidecars
/// created by the connection after this check are tightened by
/// [`secure_sidecars`]; an already-present insecure sidecar is untrusted input
/// and must be rejected instead of silently repaired.
fn validate_existing_sidecars(db_path: &Path) -> Result<(), StoreError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar_name = db_path.as_os_str().to_owned();
        sidecar_name.push(suffix);
        let sidecar_path: PathBuf = PathBuf::from(sidecar_name);
        secure_path(&sidecar_path, false, FILE_MODE, false)?;
    }
    Ok(())
}

fn configure_write_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))?;
    set_wal_mode_with_retry(conn)?;
    Ok(())
}

/// `PRAGMA journal_mode=WAL` can report `SQLITE_BUSY` immediately while a
/// concurrent first opener is migrating the schema, even with a busy timeout
/// installed. Retry only transient lock errors within that same bounded
/// timeout.
fn set_wal_mode_with_retry(conn: &Connection) -> Result<(), StoreError> {
    let started: Instant = Instant::now();
    let timeout: Duration = Duration::from_millis(BUSY_TIMEOUT_MS);
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(sqlite_error, _))
                if matches!(
                    sqlite_error.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) && started.elapsed() < timeout =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(StoreError::Sqlite(error)),
        }
    }
}

fn configure_read_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))?;
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    // Acquire the write reservation before observing `user_version`. This
    // makes two processes racing on the first open serialize cleanly: the
    // second process sees the committed version instead of attempting the
    // same CREATE TABLE batch again.
    let transaction: rusqlite::Transaction<'_> =
        conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let user_version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match user_version {
        0 => {
            // A brand-new database reaches version 2 in one transaction:
            // there is no observable intermediate state at version 1.
            transaction.execute_batch(SCHEMA_SQL_V1)?;
            transaction.execute_batch(SCHEMA_SQL_V2_ADDITIONS)?;
            transaction.pragma_update(None, "user_version", LEDGER_SCHEMA_VERSION)?;
            transaction.commit()?;
        }
        1 => {
            // An existing version 1 database is migrated additively: every
            // version 1 row is untouched, and only the new tables appear.
            transaction.execute_batch(SCHEMA_SQL_V2_ADDITIONS)?;
            transaction.pragma_update(None, "user_version", LEDGER_SCHEMA_VERSION)?;
            transaction.commit()?;
        }
        version if version == LEDGER_SCHEMA_VERSION => {
            transaction.commit()?;
        }
        found => {
            // Covers both a newer schema this build predates and any other
            // value a corrupt or foreign file might contain; either way,
            // this build cannot safely interpret the ledger and must fail
            // closed rather than guess.
            return Err(StoreError::SchemaVersionMismatch {
                found,
                expected: LEDGER_SCHEMA_VERSION,
            });
        }
    }
    Ok(())
}

fn verify_schema_version(conn: &Connection) -> Result<(), StoreError> {
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version != LEDGER_SCHEMA_VERSION {
        return Err(StoreError::SchemaVersionMismatch {
            found: user_version,
            expected: LEDGER_SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn path_to_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_to_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_encoded_bytes().to_vec()
}

#[cfg(unix)]
fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    // Safety: these bytes were produced by this same platform's
    // `path_to_bytes`, via `OsStr::as_encoded_bytes`, satisfying
    // `from_encoded_bytes_unchecked`'s safety contract.
    PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(bytes) })
}

/// Whether `body` happens to be valid UTF-8, stored purely as a read-side
/// hint in `exchange_messages.body_encoding`. The raw bytes are always
/// stored and returned verbatim regardless of this tag; nothing here ever
/// performs a lossy UTF-8 conversion.
fn body_encoding_for(body: &[u8]) -> &'static str {
    if std::str::from_utf8(body).is_ok() {
        "utf8"
    } else {
        "bytes"
    }
}

fn validate_body_encoding(raw: &str, body: &[u8]) -> Result<(), StoreError> {
    match (raw, std::str::from_utf8(body).is_ok()) {
        ("utf8", true) | ("bytes", false) => Ok(()),
        _ => Err(StoreError::CorruptBodyEncoding(raw.to_string())),
    }
}

fn validate_invocation_id(raw: &str) -> Result<(), StoreError> {
    Uuid::parse_str(raw)
        .map(|_uuid: Uuid| ())
        .map_err(|_error: uuid::Error| StoreError::CorruptInvocationId(raw.to_string()))
}

fn i32_from_db(field: &'static str, value: i64) -> Result<i32, StoreError> {
    i32::try_from(value)
        .map_err(|_error: std::num::TryFromIntError| StoreError::CorruptExitValue { field, value })
}

fn parse_bool_flag(raw: i64) -> Result<bool, StoreError> {
    match raw {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(StoreError::CorruptBoolean(other)),
    }
}

fn encode_redaction_classes(classes: &[String]) -> String {
    serde_json::to_string(classes).expect("a string vector always serializes to JSON")
}

fn decode_redaction_classes(raw: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(raw).map_err(|_| StoreError::CorruptRedactionClasses(raw.to_string()))
}

/// One idempotently-ensured pair identity row, as reported by a run plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnsuredPair {
    pub pair_key: PairKey,
    pub workspace: PathBuf,
    pub subagent_id: String,
    pub provider: Provider,
    pub created_at_unix: i64,
    pub last_seen_unix: i64,
}

/// One pair identity row as listed by `subagent pairs`. Deliberately omits
/// the raw supervisor session id; see `docs/design.md` section 15's general
/// posture against exposing more than a command needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairSummary {
    pub pair_key: PairKey,
    pub subagent_id: String,
    pub provider: Provider,
    pub created_at_unix: i64,
    pub last_seen_unix: i64,
}

/// The runtime kind of the spawned child for an invocation row. Distinct
/// from [`Provider`] (the resolved *supervisor*): the two currently share a
/// claude/codex vocabulary, but a child kind describes what `subagent`
/// spawned, not what invoked `subagent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildKind {
    Claude,
    Codex,
}

impl ChildKind {
    fn parse(raw: &str) -> Option<ChildKind> {
        match raw {
            "claude" => Some(ChildKind::Claude),
            "codex" => Some(ChildKind::Codex),
            _ => None,
        }
    }
}

impl fmt::Display for ChildKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            ChildKind::Claude => "claude",
            ChildKind::Codex => "codex",
        };
        f.write_str(text)
    }
}

/// An `invocations.status` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvocationStatus {
    Pending,
    Completed,
    SpawnFailed,
    Abandoned,
}

impl InvocationStatus {
    fn parse(raw: &str) -> Option<InvocationStatus> {
        match raw {
            "pending" => Some(InvocationStatus::Pending),
            "completed" => Some(InvocationStatus::Completed),
            "spawn_failed" => Some(InvocationStatus::SpawnFailed),
            "abandoned" => Some(InvocationStatus::Abandoned),
            _ => None,
        }
    }
}

impl fmt::Display for InvocationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            InvocationStatus::Pending => "pending",
            InvocationStatus::Completed => "completed",
            InvocationStatus::SpawnFailed => "spawn_failed",
            InvocationStatus::Abandoned => "abandoned",
        };
        f.write_str(text)
    }
}

/// How a completed child process ended, per `docs/design.md` section 7 step
/// 13. Maps to the mutually exclusive `(exit_kind, exit_code, signal)`
/// column group: exactly one of `exit_code`/`signal` is set, matching which
/// variant this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitOutcome {
    Exited { code: i32 },
    Signaled { signal: i32 },
}

/// An `exchange_messages.direction` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExchangeDirection {
    Request,
    Response,
}

impl ExchangeDirection {
    fn parse(raw: &str) -> Option<ExchangeDirection> {
        match raw {
            "request" => Some(ExchangeDirection::Request),
            "response" => Some(ExchangeDirection::Response),
            _ => None,
        }
    }
}

impl fmt::Display for ExchangeDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            ExchangeDirection::Request => "request",
            ExchangeDirection::Response => "response",
        };
        f.write_str(text)
    }
}

/// One request or response body to persist for an invocation. `body` is
/// stored and returned verbatim as bytes; `body_encoding` is derived
/// automatically (never chosen by the caller) from whether `body` happens
/// to be valid UTF-8, so this never performs a lossy conversion in either
/// direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExchangeBody {
    pub body: Vec<u8>,
    pub truncated: bool,
    pub redaction_count: u32,
    pub redaction_classes: Vec<String>,
}

/// The pending invocation allocated by [`Store::begin_invocation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BegunInvocation {
    pub invocation_id: String,
    pub sequence: i64,
    pub started_at_unix: i64,
}

/// One invocation row, fully validated on read. Deliberately omits the
/// numeric `pair_id` foreign key and every supervisor/workspace field; a
/// caller that already holds the [`PairKey`] it queried with does not need
/// the raw internal id, per `docs/design.md` section 15's posture against
/// exposing more than a command needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvocationRecord {
    pub invocation_id: String,
    pub sequence: i64,
    pub status: InvocationStatus,
    pub started_at_unix: i64,
    pub completed_at_unix: Option<i64>,
    pub command_digest: [u8; 32],
    pub program_name: String,
    pub child_kind: ChildKind,
    pub exit: Option<ExitOutcome>,
    pub capsule_path: Option<PathBuf>,
    pub capsule_digest: Option<[u8; 32]>,
    pub context_provenance: String,
}

/// One completed exchange message, as returned by
/// [`Store::list_completed_exchanges`], oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedExchange {
    pub invocation_id: String,
    pub sequence: i64,
    pub direction: ExchangeDirection,
    pub body: Vec<u8>,
    pub truncated: bool,
    pub redaction_count: u32,
    pub redaction_classes: Vec<String>,
    pub created_at_unix: i64,
}

/// The result of a read-only store open, since a missing state root or
/// database is a normal "nothing recorded yet" state, not an error.
pub(crate) enum OpenForRead {
    Ready(Store),
    Absent,
}

/// A validated, migrated connection to the pair-identity ledger.
#[derive(Debug)]
pub(crate) struct Store {
    conn: Connection,
    db_path: PathBuf,
}

impl Store {
    /// Opens the store for writing, creating the state root directory and
    /// database file (with owner-only permissions) if either is missing,
    /// and rejecting either if it already exists but is insecure.
    pub(crate) fn open_for_write(state_root: &Path) -> Result<Store, StoreError> {
        secure_path(state_root, true, DIR_MODE, true)?;
        let db_path: PathBuf = state_root.join(DB_FILE_NAME);
        secure_path(&db_path, false, FILE_MODE, true)?;
        validate_existing_sidecars(&db_path)?;

        let mut conn: Connection = Connection::open(&db_path)?;
        configure_write_connection(&conn)?;
        secure_sidecars(&db_path)?;
        migrate(&mut conn)?;
        secure_sidecars(&db_path)?;

        Ok(Store { conn, db_path })
    }

    /// Opens the store read-only for `subagent pairs`. Returns
    /// [`OpenForRead::Absent`] without creating anything when the state
    /// root or the database file does not exist yet.
    pub(crate) fn open_for_read(state_root: &Path) -> Result<OpenForRead, StoreError> {
        match secure_path(state_root, true, DIR_MODE, false)? {
            PathPresence::Missing => return Ok(OpenForRead::Absent),
            PathPresence::Ready => {}
        }
        let db_path: PathBuf = state_root.join(DB_FILE_NAME);
        match secure_path(&db_path, false, FILE_MODE, false)? {
            PathPresence::Missing => return Ok(OpenForRead::Absent),
            PathPresence::Ready => {}
        }
        validate_existing_sidecars(&db_path)?;

        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure_read_connection(&conn)?;
        verify_schema_version(&conn)?;

        Ok(OpenForRead::Ready(Store { conn, db_path }))
    }

    /// Idempotently ensures the workspace, supervisor-session, and pair
    /// identity rows exist for the given scope, in one transaction, and
    /// returns the resulting pair. Calling this again with the same inputs
    /// updates only the `last_seen` timestamps and returns the same
    /// `pair_key`.
    pub(crate) fn ensure_pair(
        &mut self,
        workspace: &WorkspaceRef,
        provider: Provider,
        supervisor_session_id: &str,
        subagent_id: &SubagentId,
    ) -> Result<EnsuredPair, StoreError> {
        let now: i64 = unix_now();
        let identity_bytes: Vec<u8> = workspace.identity_bytes();
        let pair_key: PairKey = PairKey::compute(
            &identity_bytes,
            provider,
            supervisor_session_id,
            subagent_id,
        );
        let provider_text: String = provider.to_string();

        let tx = self.conn.transaction()?;

        tx.execute(
            "INSERT INTO workspaces (canonical_path, identity_kind, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (canonical_path) DO NOTHING",
            params![identity_bytes, workspace.identity_kind(), now],
        )?;
        let workspace_id: i64 = tx.query_row(
            "SELECT id FROM workspaces WHERE canonical_path = ?1",
            params![identity_bytes],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO supervisor_sessions (provider, native_id, workspace_id, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT (workspace_id, provider, native_id) DO UPDATE SET
                 last_seen = MAX(supervisor_sessions.last_seen, excluded.last_seen)",
            params![provider_text, supervisor_session_id, workspace_id, now],
        )?;
        let supervisor_session_row_id: i64 = tx.query_row(
            "SELECT id FROM supervisor_sessions WHERE workspace_id = ?1 AND provider = ?2 AND native_id = ?3",
            params![workspace_id, provider_text, supervisor_session_id],
            |row| row.get(0),
        )?;

        let pair_key_bytes: &[u8] = pair_key.as_bytes();
        tx.execute(
            "INSERT INTO pairs (pair_key, workspace_id, supervisor_session_id, subagent_id, created_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (pair_key) DO UPDATE SET
                 last_seen = MAX(pairs.last_seen, excluded.last_seen)",
            params![
                pair_key_bytes,
                workspace_id,
                supervisor_session_row_id,
                subagent_id.as_str(),
                now
            ],
        )?;
        let (created_at, last_seen): (i64, i64) = tx.query_row(
            "SELECT created_at, last_seen FROM pairs WHERE pair_key = ?1",
            params![pair_key_bytes],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        tx.commit()?;
        secure_sidecars(&self.db_path)?;

        Ok(EnsuredPair {
            pair_key,
            workspace: workspace.canonical_path().to_path_buf(),
            subagent_id: subagent_id.as_str().to_string(),
            provider,
            created_at_unix: created_at,
            last_seen_unix: last_seen,
        })
    }

    /// Lists every pair recorded for the given workspace, oldest first.
    /// Returns an empty list when the workspace itself has never been
    /// recorded.
    pub(crate) fn list_pairs_for_workspace(
        &self,
        workspace: &WorkspaceRef,
    ) -> Result<Vec<PairSummary>, StoreError> {
        let identity_bytes: Vec<u8> = workspace.identity_bytes();
        let workspace_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM workspaces WHERE canonical_path = ?1",
                params![identity_bytes],
                |row| row.get(0),
            )
            .optional()?;
        let Some(workspace_id) = workspace_id else {
            return Ok(Vec::new());
        };

        let mut statement = self.conn.prepare(
            "SELECT p.pair_key, p.subagent_id, s.provider, p.created_at, p.last_seen
             FROM pairs p
             JOIN supervisor_sessions s ON s.id = p.supervisor_session_id
             WHERE p.workspace_id = ?1
             ORDER BY p.created_at ASC, p.id ASC",
        )?;
        let rows = statement.query_map(params![workspace_id], |row| {
            let pair_key_bytes: Vec<u8> = row.get(0)?;
            let subagent_id: String = row.get(1)?;
            let provider_text: String = row.get(2)?;
            let created_at: i64 = row.get(3)?;
            let last_seen: i64 = row.get(4)?;
            Ok((
                pair_key_bytes,
                subagent_id,
                provider_text,
                created_at,
                last_seen,
            ))
        })?;

        let mut summaries: Vec<PairSummary> = Vec::new();
        for row in rows {
            let (pair_key_bytes, subagent_id, provider_text, created_at, last_seen) = row?;
            let pair_key_array: [u8; 32] = pair_key_bytes
                .try_into()
                .map_err(|_| StoreError::CorruptPairKey)?;
            let provider: Provider = Provider::parse(&provider_text)
                .ok_or(StoreError::CorruptProvider(provider_text))?;
            summaries.push(PairSummary {
                pair_key: PairKey::from_bytes(pair_key_array),
                subagent_id,
                provider,
                created_at_unix: created_at,
                last_seen_unix: last_seen,
            });
        }
        Ok(summaries)
    }

    /// Begins a pending invocation for the pair identified by `pair_key`:
    /// allocates a monotonic per-pair `sequence` and a UUIDv7 run id under
    /// one `IMMEDIATE` transaction, and stores `request` as that
    /// invocation's single request message. `IMMEDIATE` serializes
    /// concurrent callers on the same pair so two invocations can never be
    /// allocated the same sequence.
    ///
    /// The caller spawns the child only after this returns, and calls
    /// [`Store::complete_invocation`] or [`Store::mark_spawn_failed`]
    /// afterward; no transaction is held open while the child runs.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_invocation(
        &mut self,
        pair_key: &PairKey,
        wrapper_pid: u32,
        command_digest: [u8; 32],
        program_name: &str,
        child_kind: ChildKind,
        context_provenance: &str,
        request: ExchangeBody,
    ) -> Result<BegunInvocation, StoreError> {
        let now: i64 = unix_now();
        let invocation_id: String = Uuid::now_v7().to_string();
        let body_encoding: &str = body_encoding_for(&request.body);
        let redaction_classes_json: String = encode_redaction_classes(&request.redaction_classes);

        let tx: rusqlite::Transaction<'_> = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pair_id: i64 = find_pair_id(&tx, pair_key)?.ok_or(StoreError::PairNotFound)?;

        let sequence: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM invocations WHERE pair_id = ?1",
            params![pair_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO invocations (
                 id, pair_id, sequence, status, started_at, wrapper_pid,
                 command_digest, program_name, child_kind, context_provenance
             ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                invocation_id,
                pair_id,
                sequence,
                now,
                wrapper_pid as i64,
                digest_to_blob(command_digest),
                program_name,
                child_kind.to_string(),
                context_provenance,
            ],
        )?;
        tx.execute(
            "INSERT INTO exchange_messages (
                 invocation_id, direction, body, body_encoding, truncated,
                 redaction_count, redaction_classes, created_at
             ) VALUES (?1, 'request', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                invocation_id,
                request.body,
                body_encoding,
                request.truncated as i64,
                request.redaction_count,
                redaction_classes_json,
                now,
            ],
        )?;
        tx.commit()?;
        secure_sidecars(&self.db_path)?;

        Ok(BegunInvocation {
            invocation_id,
            sequence,
            started_at_unix: now,
        })
    }

    /// Attaches a context capsule's path and content digest to an
    /// invocation that is still pending. Fails with
    /// [`StoreError::InvocationNotPending`] if `invocation_id` does not name
    /// a currently-pending invocation.
    pub(crate) fn attach_capsule(
        &mut self,
        invocation_id: &str,
        capsule_path: &Path,
        capsule_digest: [u8; 32],
    ) -> Result<(), StoreError> {
        let capsule_path_bytes: Vec<u8> = path_to_bytes(capsule_path);
        let rows: usize = self.conn.execute(
            "UPDATE invocations SET capsule_path = ?1, capsule_digest = ?2
             WHERE id = ?3 AND status = 'pending'",
            params![
                capsule_path_bytes,
                digest_to_blob(capsule_digest),
                invocation_id
            ],
        )?;
        secure_sidecars(&self.db_path)?;
        if rows == 0 {
            return Err(StoreError::InvocationNotPending(invocation_id.to_string()));
        }
        Ok(())
    }

    /// Completes a pending invocation with its exit outcome and stores
    /// `response` as that invocation's single response message, atomically:
    /// the status/exit update and the response insert either both apply or
    /// neither does. `completed_at` is clamped to be no earlier than
    /// `started_at`, keeping timestamps monotonic under concurrent updates.
    ///
    /// Fails with [`StoreError::InvocationNotPending`] if `invocation_id` is
    /// not currently pending.
    pub(crate) fn complete_invocation(
        &mut self,
        invocation_id: &str,
        exit: ExitOutcome,
        response: ExchangeBody,
    ) -> Result<(), StoreError> {
        let now: i64 = unix_now();
        let (exit_kind, exit_code, signal): (&str, Option<i64>, Option<i64>) = match exit {
            ExitOutcome::Exited { code } => ("exited", Some(code as i64), None),
            ExitOutcome::Signaled { signal } => ("signaled", None, Some(signal as i64)),
        };
        let body_encoding: &str = body_encoding_for(&response.body);
        let redaction_classes_json: String = encode_redaction_classes(&response.redaction_classes);

        let tx: rusqlite::Transaction<'_> = self.conn.transaction()?;
        let rows: usize = tx.execute(
            "UPDATE invocations SET
                 status = 'completed',
                 completed_at = MAX(?1, started_at),
                 exit_kind = ?2,
                 exit_code = ?3,
                 signal = ?4
             WHERE id = ?5 AND status = 'pending'",
            params![now, exit_kind, exit_code, signal, invocation_id],
        )?;
        if rows == 0 {
            return Err(StoreError::InvocationNotPending(invocation_id.to_string()));
        }
        tx.execute(
            "INSERT INTO exchange_messages (
                 invocation_id, direction, body, body_encoding, truncated,
                 redaction_count, redaction_classes, created_at
             ) VALUES (?1, 'response', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                invocation_id,
                response.body,
                body_encoding,
                response.truncated as i64,
                response.redaction_count,
                redaction_classes_json,
                now,
            ],
        )?;
        tx.commit()?;
        secure_sidecars(&self.db_path)?;
        Ok(())
    }

    /// Marks a pending invocation as `spawn_failed`: the child never ran,
    /// so there is no response message to store. `completed_at` is clamped
    /// to be no earlier than `started_at`.
    ///
    /// Fails with [`StoreError::InvocationNotPending`] if `invocation_id` is
    /// not currently pending.
    pub(crate) fn mark_spawn_failed(&mut self, invocation_id: &str) -> Result<(), StoreError> {
        let now: i64 = unix_now();
        let rows: usize = self.conn.execute(
            "UPDATE invocations SET status = 'spawn_failed', completed_at = MAX(?1, started_at)
             WHERE id = ?2 AND status = 'pending'",
            params![now, invocation_id],
        )?;
        secure_sidecars(&self.db_path)?;
        if rows == 0 {
            return Err(StoreError::InvocationNotPending(invocation_id.to_string()));
        }
        Ok(())
    }

    /// Marks every invocation for `pair_key` that is still `pending` and
    /// started before `started_before_unix` as `abandoned`, for crash
    /// recovery of invocations a previous wrapper process never completed.
    /// Returns the number of rows changed. `completed_at` is clamped to be
    /// no earlier than `started_at`.
    pub(crate) fn abandon_stale_pending_invocations(
        &mut self,
        pair_key: &PairKey,
        started_before_unix: i64,
    ) -> Result<u64, StoreError> {
        let now: i64 = unix_now();
        let tx: rusqlite::Transaction<'_> = self.conn.transaction()?;
        let pair_id: i64 = find_pair_id(&tx, pair_key)?.ok_or(StoreError::PairNotFound)?;
        let affected: usize = tx.execute(
            "UPDATE invocations SET status = 'abandoned', completed_at = MAX(?1, started_at)
             WHERE pair_id = ?2 AND status = 'pending' AND started_at < ?3",
            params![now, pair_id, started_before_unix],
        )?;
        tx.commit()?;
        secure_sidecars(&self.db_path)?;
        Ok(affected as u64)
    }

    /// Lists every `completed` exchange message for `pair_key`, oldest
    /// first, ordered by invocation sequence and then by message insertion
    /// order (so a request always precedes its own invocation's response).
    /// `before_sequence` excludes any invocation at or after that sequence;
    /// `None` returns the full completed history. A pending invocation's
    /// messages are always excluded, since a pending invocation is not yet
    /// part of history.
    pub(crate) fn list_completed_exchanges(
        &self,
        pair_key: &PairKey,
        before_sequence: Option<i64>,
    ) -> Result<Vec<CompletedExchange>, StoreError> {
        let Some(pair_id) = find_pair_id(&self.conn, pair_key)? else {
            return Ok(Vec::new());
        };

        let mut statement: rusqlite::Statement<'_> = self.conn.prepare(
            "SELECT i.id, i.sequence, m.direction, m.body, m.body_encoding, m.truncated,
                    m.redaction_count, m.redaction_classes, m.created_at
             FROM invocations i
             JOIN exchange_messages m ON m.invocation_id = i.id
             WHERE i.pair_id = ?1 AND i.status = 'completed'
               AND (?2 IS NULL OR i.sequence < ?2)
             ORDER BY i.sequence ASC, m.id ASC",
        )?;
        let rows = statement.query_map(params![pair_id, before_sequence], |row| {
            let invocation_id: String = row.get(0)?;
            let sequence: i64 = row.get(1)?;
            let direction_text: String = row.get(2)?;
            let body: Vec<u8> = row.get(3)?;
            let body_encoding_text: String = row.get(4)?;
            let truncated_raw: i64 = row.get(5)?;
            let redaction_count_raw: i64 = row.get(6)?;
            let redaction_classes_text: String = row.get(7)?;
            let created_at: i64 = row.get(8)?;
            Ok((
                invocation_id,
                sequence,
                direction_text,
                body,
                body_encoding_text,
                truncated_raw,
                redaction_count_raw,
                redaction_classes_text,
                created_at,
            ))
        })?;

        let mut exchanges: Vec<CompletedExchange> = Vec::new();
        for row in rows {
            let (
                invocation_id,
                sequence,
                direction_text,
                body,
                body_encoding_text,
                truncated_raw,
                redaction_count_raw,
                redaction_classes_text,
                created_at,
            ) = row?;
            let direction: ExchangeDirection = ExchangeDirection::parse(&direction_text)
                .ok_or(StoreError::CorruptDirection(direction_text))?;
            validate_invocation_id(&invocation_id)?;
            validate_body_encoding(&body_encoding_text, &body)?;
            let truncated: bool = parse_bool_flag(truncated_raw)?;
            let redaction_count: u32 = u32::try_from(redaction_count_raw)
                .map_err(|_| StoreError::CorruptRedactionCount(redaction_count_raw))?;
            let redaction_classes: Vec<String> = decode_redaction_classes(&redaction_classes_text)?;
            exchanges.push(CompletedExchange {
                invocation_id,
                sequence,
                direction,
                body,
                truncated,
                redaction_count,
                redaction_classes,
                created_at_unix: created_at,
            });
        }
        Ok(exchanges)
    }

    /// Reads and fully validates one invocation row, or `None` if
    /// `invocation_id` does not exist. Never returns the internal `pair_id`
    /// foreign key or any supervisor/workspace field.
    pub(crate) fn invocation(
        &self,
        invocation_id: &str,
    ) -> Result<Option<InvocationRecord>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT sequence, status, started_at, completed_at, command_digest,
                        program_name, child_kind, exit_kind, exit_code, signal,
                        capsule_path, capsule_digest, context_provenance
                 FROM invocations WHERE id = ?1",
                params![invocation_id],
                |row| {
                    let sequence: i64 = row.get(0)?;
                    let status_text: String = row.get(1)?;
                    let started_at: i64 = row.get(2)?;
                    let completed_at: Option<i64> = row.get(3)?;
                    let command_digest_bytes: Vec<u8> = row.get(4)?;
                    let program_name: String = row.get(5)?;
                    let child_kind_text: String = row.get(6)?;
                    let exit_kind_text: Option<String> = row.get(7)?;
                    let exit_code: Option<i64> = row.get(8)?;
                    let signal: Option<i64> = row.get(9)?;
                    let capsule_path_bytes: Option<Vec<u8>> = row.get(10)?;
                    let capsule_digest_bytes: Option<Vec<u8>> = row.get(11)?;
                    let context_provenance: String = row.get(12)?;
                    Ok((
                        sequence,
                        status_text,
                        started_at,
                        completed_at,
                        command_digest_bytes,
                        program_name,
                        child_kind_text,
                        exit_kind_text,
                        exit_code,
                        signal,
                        capsule_path_bytes,
                        capsule_digest_bytes,
                        context_provenance,
                    ))
                },
            )
            .optional()?;

        let Some((
            sequence,
            status_text,
            started_at,
            completed_at,
            command_digest_bytes,
            program_name,
            child_kind_text,
            exit_kind_text,
            exit_code,
            signal,
            capsule_path_bytes,
            capsule_digest_bytes,
            context_provenance,
        )) = row
        else {
            return Ok(None);
        };

        validate_invocation_id(invocation_id)?;
        let status: InvocationStatus = InvocationStatus::parse(&status_text)
            .ok_or(StoreError::CorruptInvocationStatus(status_text))?;
        let child_kind: ChildKind = ChildKind::parse(&child_kind_text)
            .ok_or(StoreError::CorruptChildKind(child_kind_text))?;
        let command_digest: [u8; 32] = command_digest_bytes
            .try_into()
            .map_err(|_| StoreError::CorruptDigest("command_digest"))?;
        let capsule_digest: Option<[u8; 32]> = capsule_digest_bytes
            .map(|bytes| {
                bytes
                    .try_into()
                    .map_err(|_| StoreError::CorruptDigest("capsule_digest"))
            })
            .transpose()?;
        let exit: Option<ExitOutcome> = match (exit_kind_text, exit_code, signal) {
            (None, None, None) => None,
            (Some(kind), Some(code), None) if kind == "exited" => Some(ExitOutcome::Exited {
                code: i32_from_db("exit_code", code)?,
            }),
            (Some(kind), None, Some(signal)) if kind == "signaled" => Some(ExitOutcome::Signaled {
                signal: i32_from_db("signal", signal)?,
            }),
            (Some(kind), ..) => return Err(StoreError::CorruptExitKind(kind)),
            _ => return Err(StoreError::CorruptExitKind(String::new())),
        };

        Ok(Some(InvocationRecord {
            invocation_id: invocation_id.to_string(),
            sequence,
            status,
            started_at_unix: started_at,
            completed_at_unix: completed_at,
            command_digest,
            program_name,
            child_kind,
            exit,
            capsule_path: capsule_path_bytes.map(bytes_to_path),
            capsule_digest,
            context_provenance,
        }))
    }

    /// Lists this pair's invocation metadata newest first. Bodies remain in
    /// `exchange_messages` and are intentionally returned only by
    /// [`Store::list_completed_exchanges`].
    pub(crate) fn list_invocations(
        &self,
        pair_key: &PairKey,
        limit: Option<u32>,
    ) -> Result<Vec<InvocationRecord>, StoreError> {
        let Some(pair_id) = find_pair_id(&self.conn, pair_key)? else {
            return Ok(Vec::new());
        };
        let sql: &str =
            "SELECT id FROM invocations WHERE pair_id = ?1 ORDER BY sequence DESC LIMIT ?2";
        let effective_limit: i64 = limit.map(i64::from).unwrap_or(i64::MAX);
        let mut statement: rusqlite::Statement<'_> = self.conn.prepare(sql)?;
        let ids = statement.query_map(params![pair_id, effective_limit], |row| {
            row.get::<_, String>(0)
        })?;
        let mut records: Vec<InvocationRecord> = Vec::new();
        for id in ids {
            let invocation_id: String = id?;
            let record: InvocationRecord = self
                .invocation(&invocation_id)?
                .ok_or_else(|| StoreError::CorruptInvocationId(invocation_id.clone()))?;
            records.push(record);
        }
        Ok(records)
    }

    /// Deletes one pair and all of its dependent invocations and exchange
    /// messages, transactionally. The pair's `workspace_id` and
    /// `supervisor_session_id` rows are never touched by this: cascading
    /// delete flows only from `pairs` down to its own invocations and their
    /// messages, never up to the workspace or supervisor session it
    /// belonged to. Returns `false` if `pair_key` did not name an existing
    /// pair.
    pub(crate) fn delete_pair(&mut self, pair_key: &PairKey) -> Result<bool, StoreError> {
        let tx: rusqlite::Transaction<'_> = self.conn.transaction()?;
        let rows: usize = tx.execute(
            "DELETE FROM pairs WHERE pair_key = ?1",
            params![pair_key.as_bytes().as_slice()],
        )?;
        tx.commit()?;
        secure_sidecars(&self.db_path)?;
        Ok(rows > 0)
    }
}

fn find_pair_id(conn: &Connection, pair_key: &PairKey) -> Result<Option<i64>, StoreError> {
    conn.query_row(
        "SELECT id FROM pairs WHERE pair_key = ?1",
        params![pair_key.as_bytes().as_slice()],
        |row| row.get(0),
    )
    .optional()
    .map_err(StoreError::from)
}

fn digest_to_blob(digest: [u8; 32]) -> Vec<u8> {
    digest.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> SubagentId {
        SubagentId::parse(raw).unwrap()
    }

    fn workspace(dir: &Path) -> WorkspaceRef {
        WorkspaceRef::from_dir(dir).unwrap()
    }

    fn sample_body(bytes: &[u8]) -> ExchangeBody {
        ExchangeBody {
            body: bytes.to_vec(),
            truncated: false,
            redaction_count: 0,
            redaction_classes: Vec::new(),
        }
    }

    #[test]
    fn open_for_write_creates_state_root_and_database() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let _store = Store::open_for_write(&state_root).unwrap();
        assert!(state_root.is_dir());
        assert!(state_root.join(DB_FILE_NAME).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn open_for_write_creates_directory_and_file_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let _store = Store::open_for_write(&state_root).unwrap();

        let dir_mode = std::fs::metadata(&state_root).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let file_mode = std::fs::metadata(state_root.join(DB_FILE_NAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn wal_sidecars_are_owner_only_while_the_store_is_open() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let mut store = Store::open_for_write(&state_root).unwrap();
        store
            .ensure_pair(
                &workspace(workspace_dir.path()),
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap();

        for suffix in ["-wal", "-shm"] {
            let mut sidecar_name = store.db_path.as_os_str().to_owned();
            sidecar_name.push(suffix);
            let sidecar_path = PathBuf::from(sidecar_name);
            let mode: u32 = std::fs::metadata(&sidecar_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "unexpected mode for {sidecar_path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_a_symlinked_state_root() {
        let root = tempfile::tempdir().unwrap();
        let real_dir = root.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let link = root.path().join("state-link");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();

        let error = Store::open_for_write(&link).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Insecure(InsecurePath {
                reason: InsecureReason::Symlink,
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_an_existing_state_root_with_group_readable_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        std::fs::create_dir(&state_root).unwrap();
        std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o750)).unwrap();

        let error = Store::open_for_write(&state_root).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Insecure(InsecurePath {
                reason: InsecureReason::InsecureMode,
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_an_existing_group_readable_wal_sidecar() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let state_root: PathBuf = root.path().join("state");
        drop(Store::open_for_write(&state_root).unwrap());

        let wal_path: PathBuf = state_root.join(format!("{DB_FILE_NAME}-wal"));
        std::fs::write(&wal_path, b"").unwrap();
        std::fs::set_permissions(&wal_path, std::fs::Permissions::from_mode(0o640)).unwrap();

        let error: StoreError = Store::open_for_write(&state_root).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Insecure(InsecurePath {
                reason: InsecureReason::InsecureMode,
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_a_state_root_that_is_a_regular_file() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        std::fs::write(&state_root, b"not a directory").unwrap();

        let error = Store::open_for_write(&state_root).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Insecure(InsecurePath {
                reason: InsecureReason::NotADirectory,
                ..
            })
        ));
    }

    #[test]
    fn ensure_pair_is_idempotent_and_reuses_the_same_pair_key() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());

        let first = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap();
        let second = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap();

        assert_eq!(first.pair_key, second.pair_key);
        assert_eq!(first.created_at_unix, second.created_at_unix);

        let pairs = store.list_pairs_for_workspace(&workspace_ref).unwrap();
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn ensure_pair_distinguishes_different_subagent_ids_in_the_same_scope() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());

        store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap();
        store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("implementer"),
            )
            .unwrap();

        let pairs = store.list_pairs_for_workspace(&workspace_ref).unwrap();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn concurrent_writers_ensure_one_pair_row() {
        use std::sync::{Arc, Barrier};

        const WRITER_COUNT: usize = 8;

        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let state_root: PathBuf = root.path().join("state");
        let workspace_path: PathBuf = workspace_dir.path().to_path_buf();
        drop(Store::open_for_write(&state_root).unwrap());

        let barrier: Arc<Barrier> = Arc::new(Barrier::new(WRITER_COUNT));
        let mut handles: Vec<std::thread::JoinHandle<()>> = Vec::new();
        for _ in 0..WRITER_COUNT {
            let thread_barrier: Arc<Barrier> = Arc::clone(&barrier);
            let thread_state_root: PathBuf = state_root.clone();
            let thread_workspace_path: PathBuf = workspace_path.clone();
            handles.push(std::thread::spawn(move || {
                let workspace_ref: WorkspaceRef = workspace(&thread_workspace_path);
                thread_barrier.wait();
                let mut store: Store = Store::open_for_write(&thread_state_root).unwrap();
                store
                    .ensure_pair(
                        &workspace_ref,
                        Provider::Codex,
                        "session-1",
                        &id("reviewer"),
                    )
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let result: OpenForRead = Store::open_for_read(&state_root).unwrap();
        let OpenForRead::Ready(store) = result else {
            panic!("expected the store to exist");
        };
        let workspace_ref: WorkspaceRef = workspace(&workspace_path);
        assert_eq!(
            store
                .list_pairs_for_workspace(&workspace_ref)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn concurrent_first_open_initializes_one_store() {
        use std::sync::{Arc, Barrier};

        const WRITER_COUNT: usize = 8;

        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let state_root: PathBuf = root.path().join("new-state");
        let workspace_path: PathBuf = workspace_dir.path().to_path_buf();
        let barrier: Arc<Barrier> = Arc::new(Barrier::new(WRITER_COUNT));
        let mut handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

        for _ in 0..WRITER_COUNT {
            let thread_barrier: Arc<Barrier> = Arc::clone(&barrier);
            let thread_state_root: PathBuf = state_root.clone();
            let thread_workspace_path: PathBuf = workspace_path.clone();
            handles.push(std::thread::spawn(move || {
                let workspace_ref: WorkspaceRef = workspace(&thread_workspace_path);
                thread_barrier.wait();
                let mut store: Store = Store::open_for_write(&thread_state_root).unwrap();
                store
                    .ensure_pair(
                        &workspace_ref,
                        Provider::Codex,
                        "session-1",
                        &id("reviewer"),
                    )
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let result: OpenForRead = Store::open_for_read(&state_root).unwrap();
        let OpenForRead::Ready(store) = result else {
            panic!("expected the store to exist");
        };
        let workspace_ref: WorkspaceRef = workspace(&workspace_path);
        assert_eq!(
            store
                .list_pairs_for_workspace(&workspace_ref)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn list_pairs_is_scoped_to_the_matching_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace_a_dir = tempfile::tempdir().unwrap();
        let workspace_b_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_a = workspace(workspace_a_dir.path());
        let workspace_b = workspace(workspace_b_dir.path());

        store
            .ensure_pair(&workspace_a, Provider::Codex, "session-1", &id("reviewer"))
            .unwrap();

        assert_eq!(
            store.list_pairs_for_workspace(&workspace_a).unwrap().len(),
            1
        );
        assert!(
            store
                .list_pairs_for_workspace(&workspace_b)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn open_for_read_returns_absent_without_creating_a_missing_root() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");

        let result = Store::open_for_read(&state_root).unwrap();
        assert!(matches!(result, OpenForRead::Absent));
        assert!(!state_root.exists());
    }

    #[test]
    fn open_for_read_returns_absent_without_creating_a_missing_database() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        std::fs::create_dir(&state_root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let result = Store::open_for_read(&state_root).unwrap();
        assert!(matches!(result, OpenForRead::Absent));
        assert!(!state_root.join(DB_FILE_NAME).exists());
    }

    #[test]
    fn open_for_read_lists_previously_ensured_pairs() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_dir = tempfile::tempdir().unwrap();
        let workspace_ref = workspace(workspace_dir.path());

        {
            let mut store = Store::open_for_write(&state_root).unwrap();
            store
                .ensure_pair(
                    &workspace_ref,
                    Provider::Claude,
                    "session-1",
                    &id("reviewer"),
                )
                .unwrap();
        }

        let result = Store::open_for_read(&state_root).unwrap();
        let OpenForRead::Ready(store) = result else {
            panic!("expected the store to be present");
        };
        let pairs = store.list_pairs_for_workspace(&workspace_ref).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].subagent_id, "reviewer");
        assert_eq!(pairs[0].provider, Provider::Claude);
    }

    #[test]
    fn schema_version_mismatch_is_rejected_on_write_open() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        {
            let store = Store::open_for_write(&state_root).unwrap();
            store
                .conn
                .pragma_update(None, "user_version", LEDGER_SCHEMA_VERSION + 1)
                .unwrap();
        }

        let error = Store::open_for_write(&state_root).unwrap_err();
        assert!(matches!(
            error,
            StoreError::SchemaVersionMismatch {
                found,
                expected
            } if found == LEDGER_SCHEMA_VERSION + 1 && expected == LEDGER_SCHEMA_VERSION
        ));
    }

    #[test]
    fn fresh_database_reaches_version_2_with_a_usable_invocation_ledger() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let store = Store::open_for_write(&state_root).unwrap();

        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, LEDGER_SCHEMA_VERSION);
        assert_eq!(LEDGER_SCHEMA_VERSION, 2);

        let invocation_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(invocation_count, 0);
        let exchange_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM exchange_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(exchange_count, 0);
    }

    /// Builds an on-disk version 1 database by hand (the exact schema the
    /// pair-identity-store increment shipped, `SCHEMA_SQL_V1`) with one
    /// pre-existing pair, so the migration path exercised here is the real
    /// version 1 -> 2 upgrade rather than a stand-in.
    #[test]
    fn opening_a_v1_fixture_migrates_to_v2_and_preserves_its_pair() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_dir = tempfile::tempdir().unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let identity_bytes: Vec<u8> = workspace_ref.identity_bytes();
        let pair_key = PairKey::compute(
            &identity_bytes,
            Provider::Codex,
            "session-1",
            &id("reviewer"),
        );

        std::fs::create_dir(&state_root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db_path = state_root.join(DB_FILE_NAME);
        {
            let fixture_conn = Connection::open(&db_path).unwrap();
            fixture_conn.execute_batch(SCHEMA_SQL_V1).unwrap();
            fixture_conn
                .execute(
                    "INSERT INTO workspaces (canonical_path, identity_kind, created_at)
                     VALUES (?1, 'path', 1000)",
                    params![identity_bytes],
                )
                .unwrap();
            fixture_conn
                .execute(
                    "INSERT INTO supervisor_sessions
                         (provider, native_id, workspace_id, first_seen, last_seen)
                     VALUES ('codex', 'session-1', 1, 1000, 1000)",
                    [],
                )
                .unwrap();
            fixture_conn
                .execute(
                    "INSERT INTO pairs
                         (pair_key, workspace_id, supervisor_session_id, subagent_id,
                          created_at, last_seen)
                     VALUES (?1, 1, 1, 'reviewer', 1000, 1000)",
                    params![pair_key.as_bytes().as_slice()],
                )
                .unwrap();
            fixture_conn
                .pragma_update(None, "user_version", 1i64)
                .unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let mut store = Store::open_for_write(&state_root).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, LEDGER_SCHEMA_VERSION);

        let pairs = store.list_pairs_for_workspace(&workspace_ref).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].pair_key, pair_key);
        assert_eq!(pairs[0].subagent_id, "reviewer");

        // The migration is genuinely additive: the new tables work.
        let begun = store
            .begin_invocation(
                &pair_key,
                123,
                [7u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"hi"),
            )
            .unwrap();
        assert_eq!(begun.sequence, 1);
    }

    #[test]
    fn unknown_newer_schema_version_still_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        {
            let store = Store::open_for_write(&state_root).unwrap();
            store
                .conn
                .pragma_update(None, "user_version", LEDGER_SCHEMA_VERSION + 1)
                .unwrap();
        }

        let error = Store::open_for_write(&state_root).unwrap_err();
        assert!(matches!(
            error,
            StoreError::SchemaVersionMismatch { found, expected }
                if found == LEDGER_SCHEMA_VERSION + 1 && expected == LEDGER_SCHEMA_VERSION
        ));
    }

    #[test]
    fn concurrent_begin_invocation_allocates_a_monotonic_per_pair_sequence() {
        use std::sync::{Arc, Barrier};

        const WRITER_COUNT: usize = 8;

        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let state_root: PathBuf = root.path().join("state");
        let workspace_ref = workspace(workspace_dir.path());

        let pair_key: PairKey = {
            let mut store = Store::open_for_write(&state_root).unwrap();
            store
                .ensure_pair(
                    &workspace_ref,
                    Provider::Codex,
                    "session-1",
                    &id("reviewer"),
                )
                .unwrap()
                .pair_key
        };

        let barrier: Arc<Barrier> = Arc::new(Barrier::new(WRITER_COUNT));
        let mut handles: Vec<std::thread::JoinHandle<i64>> = Vec::new();
        for _ in 0..WRITER_COUNT {
            let thread_barrier: Arc<Barrier> = Arc::clone(&barrier);
            let thread_state_root: PathBuf = state_root.clone();
            handles.push(std::thread::spawn(move || {
                thread_barrier.wait();
                let mut store: Store = Store::open_for_write(&thread_state_root).unwrap();
                store
                    .begin_invocation(
                        &pair_key,
                        1,
                        [1u8; 32],
                        "codex",
                        ChildKind::Codex,
                        "provenance",
                        sample_body(b"x"),
                    )
                    .unwrap()
                    .sequence
            }));
        }
        let mut sequences: Vec<i64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        sequences.sort();
        assert_eq!(sequences, (1..=WRITER_COUNT as i64).collect::<Vec<i64>>());
    }

    #[test]
    fn exchange_bodies_round_trip_non_utf8_bytes_without_lossy_conversion() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let pair_key: PairKey = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap()
            .pair_key;

        let request_body: Vec<u8> = vec![0xff, 0xfe, 0x00, 0x80, b'h', b'i'];
        let response_body: Vec<u8> = vec![0x00, 0xc0, 0xc1, 0xf5];
        let begun = store
            .begin_invocation(
                &pair_key,
                42,
                [3u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(&request_body),
            )
            .unwrap();
        store
            .complete_invocation(
                &begun.invocation_id,
                ExitOutcome::Exited { code: 0 },
                sample_body(&response_body),
            )
            .unwrap();

        let exchanges = store.list_completed_exchanges(&pair_key, None).unwrap();
        assert_eq!(exchanges.len(), 2);
        assert_eq!(exchanges[0].direction, ExchangeDirection::Request);
        assert_eq!(exchanges[0].body, request_body);
        assert_eq!(exchanges[1].direction, ExchangeDirection::Response);
        assert_eq!(exchanges[1].body, response_body);
    }

    #[test]
    fn pending_invocation_is_excluded_from_completed_history_until_completed() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let pair_key: PairKey = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap()
            .pair_key;

        let begun = store
            .begin_invocation(
                &pair_key,
                7,
                [2u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"request"),
            )
            .unwrap();
        assert!(
            store
                .list_completed_exchanges(&pair_key, None)
                .unwrap()
                .is_empty()
        );

        store
            .complete_invocation(
                &begun.invocation_id,
                ExitOutcome::Exited { code: 0 },
                sample_body(b"response"),
            )
            .unwrap();
        let exchanges = store.list_completed_exchanges(&pair_key, None).unwrap();
        assert_eq!(exchanges.len(), 2);
        assert!(
            exchanges
                .iter()
                .all(|exchange| exchange.sequence == begun.sequence)
        );
    }

    #[test]
    fn complete_invocation_is_atomic_and_rejects_a_second_completion() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let pair_key: PairKey = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap()
            .pair_key;

        let begun = store
            .begin_invocation(
                &pair_key,
                7,
                [4u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"request"),
            )
            .unwrap();
        store
            .complete_invocation(
                &begun.invocation_id,
                ExitOutcome::Signaled { signal: 9 },
                sample_body(b"response"),
            )
            .unwrap();

        let record = store.invocation(&begun.invocation_id).unwrap().unwrap();
        assert_eq!(record.status, InvocationStatus::Completed);
        assert_eq!(record.exit, Some(ExitOutcome::Signaled { signal: 9 }));
        assert!(record.completed_at_unix.is_some());

        let error = store
            .complete_invocation(
                &begun.invocation_id,
                ExitOutcome::Exited { code: 0 },
                sample_body(b"second response"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvocationNotPending(id) if id == begun.invocation_id
        ));

        let response_rows: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM exchange_messages
                 WHERE invocation_id = ?1 AND direction = 'response'",
                params![begun.invocation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(response_rows, 1);
    }

    #[test]
    fn abandon_stale_pending_invocations_only_affects_old_enough_pending_rows() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let pair_key: PairKey = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap()
            .pair_key;

        let stale = store
            .begin_invocation(
                &pair_key,
                7,
                [5u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"stale request"),
            )
            .unwrap();
        let fresh = store
            .begin_invocation(
                &pair_key,
                8,
                [6u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"fresh request"),
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE invocations SET started_at = 100 WHERE id = ?1",
                params![stale.invocation_id],
            )
            .unwrap();

        let affected = store
            .abandon_stale_pending_invocations(&pair_key, 200)
            .unwrap();
        assert_eq!(affected, 1);

        let stale_record = store.invocation(&stale.invocation_id).unwrap().unwrap();
        assert_eq!(stale_record.status, InvocationStatus::Abandoned);
        assert!(stale_record.completed_at_unix.is_some());

        let fresh_record = store.invocation(&fresh.invocation_id).unwrap().unwrap();
        assert_eq!(fresh_record.status, InvocationStatus::Pending);
    }

    #[test]
    fn mark_spawn_failed_transitions_a_pending_invocation_and_is_excluded_from_history() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let pair_key: PairKey = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap()
            .pair_key;

        let begun = store
            .begin_invocation(
                &pair_key,
                7,
                [8u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"request"),
            )
            .unwrap();
        store.mark_spawn_failed(&begun.invocation_id).unwrap();

        let record = store.invocation(&begun.invocation_id).unwrap().unwrap();
        assert_eq!(record.status, InvocationStatus::SpawnFailed);
        assert!(record.completed_at_unix.is_some());
        assert!(record.exit.is_none());
        assert!(
            store
                .list_completed_exchanges(&pair_key, None)
                .unwrap()
                .is_empty()
        );

        let error = store.mark_spawn_failed(&begun.invocation_id).unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvocationNotPending(id) if id == begun.invocation_id
        ));
    }

    #[test]
    fn attach_capsule_records_a_non_utf8_path_and_digest_on_a_pending_invocation() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());
        let pair_key: PairKey = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap()
            .pair_key;

        let begun = store
            .begin_invocation(
                &pair_key,
                7,
                [1u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"request"),
            )
            .unwrap();

        #[cfg(unix)]
        let capsule_path: PathBuf = {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(OsStr::from_bytes(&[0x2f, 0x66, 0xff, 0x6f]))
        };
        #[cfg(not(unix))]
        let capsule_path: PathBuf = PathBuf::from("/capsule/path");
        let capsule_digest: [u8; 32] = [42u8; 32];

        store
            .attach_capsule(&begun.invocation_id, &capsule_path, capsule_digest)
            .unwrap();

        let record = store.invocation(&begun.invocation_id).unwrap().unwrap();
        assert_eq!(record.capsule_path, Some(capsule_path.clone()));
        assert_eq!(record.capsule_digest, Some(capsule_digest));
        assert_eq!(record.status, InvocationStatus::Pending);

        store
            .complete_invocation(
                &begun.invocation_id,
                ExitOutcome::Exited { code: 0 },
                sample_body(b"response"),
            )
            .unwrap();

        let error = store
            .attach_capsule(&begun.invocation_id, &capsule_path, capsule_digest)
            .unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvocationNotPending(id) if id == begun.invocation_id
        ));
    }

    #[test]
    fn delete_pair_removes_only_its_own_invocations_and_leaves_siblings_intact() {
        let root = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_for_write(&root.path().join("state")).unwrap();
        let workspace_ref = workspace(workspace_dir.path());

        let pair_a = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("reviewer"),
            )
            .unwrap();
        let pair_b = store
            .ensure_pair(
                &workspace_ref,
                Provider::Codex,
                "session-1",
                &id("implementer"),
            )
            .unwrap();

        let begun = store
            .begin_invocation(
                &pair_a.pair_key,
                7,
                [9u8; 32],
                "codex",
                ChildKind::Codex,
                "provenance",
                sample_body(b"request"),
            )
            .unwrap();
        store
            .complete_invocation(
                &begun.invocation_id,
                ExitOutcome::Exited { code: 0 },
                sample_body(b"response"),
            )
            .unwrap();

        assert!(store.delete_pair(&pair_a.pair_key).unwrap());

        assert!(store.invocation(&begun.invocation_id).unwrap().is_none());
        let remaining_invocations: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining_invocations, 0);
        let remaining_messages: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM exchange_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining_messages, 0);

        let pairs = store.list_pairs_for_workspace(&workspace_ref).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].pair_key, pair_b.pair_key);

        let workspace_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
            .unwrap();
        assert_eq!(workspace_count, 1);
        let supervisor_session_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM supervisor_sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(supervisor_session_count, 1);

        assert!(!store.delete_pair(&pair_a.pair_key).unwrap());
    }
}
