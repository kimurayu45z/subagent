//! Resolves the `subagent` state root directory: `docs/design.md` section 10,
//! "The state root is selected with the platform-aware `directories` crate.
//! It uses the operating system's state directory when available and a
//! local application data directory fallback otherwise."
//!
//! `SUBAGENT_STATE_DIR` is a supported override. It is read exactly once,
//! at each command's process edge (its outer `execute` function), and
//! threaded down explicitly from there, so inner logic stays testable
//! without mutating real process environment state and tests never resolve
//! the real user state root by accident.

use std::ffi::OsStr;
use std::fmt;
use std::path::PathBuf;

pub(crate) const SUBAGENT_STATE_DIR_ENV: &str = "SUBAGENT_STATE_DIR";

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "kimurayu45z";
const APPLICATION: &str = "subagent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StateDirError {
    EmptyOverride,
    ProjectDirsUnavailable,
}

impl fmt::Display for StateDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateDirError::EmptyOverride => write!(
                f,
                "{SUBAGENT_STATE_DIR_ENV} is set but empty; unset it or point it at a directory path"
            ),
            StateDirError::ProjectDirsUnavailable => write!(
                f,
                "could not determine a state directory for this platform/user (no home \
                 directory found); set {SUBAGENT_STATE_DIR_ENV} to an explicit path instead"
            ),
        }
    }
}

impl std::error::Error for StateDirError {}

/// Resolves the state root directory without touching the filesystem.
///
/// `override_value` should be `std::env::var_os(SUBAGENT_STATE_DIR_ENV)`,
/// read once by the caller. When absent, this falls back to
/// `directories::ProjectDirs`'s OS state directory, and then to its local
/// data directory when the platform has no distinct state directory (for
/// example macOS).
pub(crate) fn resolve_state_root(override_value: Option<&OsStr>) -> Result<PathBuf, StateDirError> {
    if let Some(value) = override_value {
        return if value.is_empty() {
            Err(StateDirError::EmptyOverride)
        } else {
            Ok(PathBuf::from(value))
        };
    }

    let project_dirs: directories::ProjectDirs =
        directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
            .ok_or(StateDirError::ProjectDirsUnavailable)?;

    Ok(project_dirs
        .state_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dirs.data_local_dir().to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn explicit_override_wins_and_is_used_verbatim() {
        let resolved = resolve_state_root(Some(OsStr::new("/tmp/example-state-root"))).unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/example-state-root"));
    }

    #[test]
    fn empty_override_is_rejected_not_treated_as_absent() {
        let empty: OsString = OsString::new();
        let error = resolve_state_root(Some(&empty)).unwrap_err();
        assert_eq!(error, StateDirError::EmptyOverride);
    }

    #[test]
    fn error_messages_name_the_override_variable() {
        assert!(
            StateDirError::EmptyOverride
                .to_string()
                .contains(SUBAGENT_STATE_DIR_ENV)
        );
        assert!(
            StateDirError::ProjectDirsUnavailable
                .to_string()
                .contains(SUBAGENT_STATE_DIR_ENV)
        );
    }
}
