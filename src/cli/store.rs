//! The SQLite pair-identity metadata store: `docs/design.md` section 10.
//!
//! Scope in this build is deliberately narrow: only the `workspaces`,
//! `supervisor_sessions`, and `pairs` tables from the design's minimum
//! schema exist. `workspace_memories`, `child_sessions`, `invocations`,
//! `exchange_messages`, and `summaries` are not implemented yet.
//!
//! Security posture, per `docs/design.md` section 10 and section 15:
//! directories this build owns are created `0700`; the database file (and
//! its WAL/SHM/journal sidecars) are created `0600`; a symlink, a
//! non-directory/non-file, a path owned by another user, or a path with
//! group- or other-accessible permissions at any of those locations is
//! rejected rather than silently used or "fixed up" by following it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use super::id::SubagentId;
use super::pair_key::PairKey;
use super::supervisor::Provider;
use super::workspace::WorkspaceRef;

/// The on-disk ledger schema version, tracked independently of
/// [`super::pair_key::PAIR_KEY_SCHEMA_VERSION`] and
/// [`super::report::REPORT_SCHEMA_VERSION`] via `PRAGMA user_version`. A
/// mismatch means this build cannot safely interpret an existing ledger
/// file.
pub(crate) const LEDGER_SCHEMA_VERSION: i64 = 1;

const DB_FILE_NAME: &str = "ledger.sqlite3";
const BUSY_TIMEOUT_MS: u64 = 5_000;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

const SCHEMA_SQL: &str = "
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

#[derive(Debug)]
pub(crate) enum StoreError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Insecure(InsecurePath),
    SchemaVersionMismatch { found: i64, expected: i64 },
    CorruptPairKey,
    CorruptProvider(String),
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
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
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
    if user_version == 0 {
        transaction.execute_batch(SCHEMA_SQL)?;
        transaction.pragma_update(None, "user_version", LEDGER_SCHEMA_VERSION)?;
        transaction.commit()?;
    } else if user_version != LEDGER_SCHEMA_VERSION {
        return Err(StoreError::SchemaVersionMismatch {
            found: user_version,
            expected: LEDGER_SCHEMA_VERSION,
        });
    } else {
        transaction.commit()?;
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
}
