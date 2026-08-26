//! The small, versioned machine-report shape used by every command that
//! offers `--format json` or that writes an explicit `--report PATH` file.
//!
//! See `docs/design.md` section 14: "Machine-readable wrapper reports use
//! an explicit file path or dedicated file descriptor. They are never mixed
//! with child stdout or stderr" and "A machine report encodes non-UTF-8
//! bytes explicitly rather than using lossy replacement."

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub(crate) const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReportStatus {
    Ok,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Report<T: Serialize> {
    pub schema_version: u32,
    pub kind: String,
    pub status: ReportStatus,
    pub generated_at_unix: u64,
    pub body: T,
}

impl<T: Serialize> Report<T> {
    pub(crate) fn new(kind: impl Into<String>, status: ReportStatus, body: T) -> Self {
        let generated_at_unix: u64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        Report {
            schema_version: REPORT_SCHEMA_VERSION,
            kind: kind.into(),
            status,
            generated_at_unix,
            body,
        }
    }

    pub(crate) fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serialization must not fail")
    }
}

/// Atomically writes a report beside its destination before replacing the
/// destination. Reports may contain prompts or command arguments, so Unix
/// permissions are forced to owner-read/write regardless of the caller's
/// umask or an existing destination's mode.
pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, report: &Report<T>) -> io::Result<()> {
    let parent: &Path = path
        .parent()
        .filter(|candidate: &&Path| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary: tempfile::NamedTempFile = tempfile::NamedTempFile::new_in(parent)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }

    let json: String = report.to_json_pretty();
    temporary.write_all(json.as_bytes())?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// A non-lossy JSON encoding of an `OsStr`. Arguments that are valid UTF-8
/// are encoded as plain strings; arguments that are not are encoded as an
/// explicit byte array so no information is discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub(crate) enum OsStringJson {
    Utf8(String),
    Bytes(Vec<u8>),
}

impl OsStringJson {
    pub(crate) fn from_os_str(value: &OsStr) -> Self {
        match value.to_str() {
            Some(text) => OsStringJson::Utf8(text.to_string()),
            None => OsStringJson::Bytes(os_str_bytes(value)),
        }
    }
}

#[cfg(unix)]
fn os_str_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_str_bytes(value: &OsStr) -> Vec<u8> {
    // Non-UTF-8 OS strings on non-Unix platforms cannot be inspected as raw
    // bytes through `std` alone. `docs/design.md` targets Linux and macOS
    // only, so this path only affects incidental non-target builds.
    value.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_os_string_encodes_as_utf8_variant() {
        let value = std::ffi::OsString::from("hello");
        assert_eq!(
            OsStringJson::from_os_str(&value),
            OsStringJson::Utf8("hello".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_os_string_encodes_as_bytes_variant() {
        use std::os::unix::ffi::OsStringExt;
        let value = std::ffi::OsString::from_vec(vec![0x66, 0x6f, 0xff, 0x6f]);
        assert_eq!(
            OsStringJson::from_os_str(&value),
            OsStringJson::Bytes(vec![0x66, 0x6f, 0xff, 0x6f])
        );
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = Report::new("test", ReportStatus::Ok, vec![1, 2, 3]);
        let json = report.to_json_pretty();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"kind\": \"test\""));
        assert!(json.contains("\"status\": \"ok\""));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_report_replaces_an_existing_file_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory: tempfile::TempDir = tempfile::tempdir().unwrap();
        let path: std::path::PathBuf = directory.path().join("report.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let report: Report<Vec<u8>> = Report::new("test", ReportStatus::Ok, vec![1, 2, 3]);
        write_json_atomic(&path, &report).unwrap();

        let mode: u32 = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["kind"], "test");
    }
}
