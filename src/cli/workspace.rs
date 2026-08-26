//! Canonical workspace identity: `docs/design.md` section 3.1's
//! `workspace_root` and section 10's `workspaces(canonical_path,
//! identity_kind, ...)`.
//!
//! The only identity kind implemented in this build is `path`: the
//! canonicalized (symlink-resolved, absolute) current working directory.
//! The identity bytes fed into [`crate::cli::pair_key`] are the raw
//! operating-system bytes of that canonical path, never a lossy UTF-8
//! projection, so a workspace whose path is not valid UTF-8 on Unix still
//! gets a stable, correct identity instead of a mangled or ambiguous one.

use std::io;
use std::path::{Path, PathBuf};

/// The only workspace identity mechanism implemented in this build. See
/// `docs/design.md` section 10's `workspaces.identity_kind` column.
pub(crate) const IDENTITY_KIND_PATH: &str = "path";

/// A canonical workspace identity resolved from a directory on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceRef {
    canonical_path: PathBuf,
}

impl WorkspaceRef {
    /// Resolves the workspace identity from an explicit directory,
    /// canonicalizing it (resolving symlinks and `.`/`..` components) so two
    /// different paths that refer to the same directory produce the same
    /// identity.
    pub(crate) fn from_dir(path: &Path) -> io::Result<WorkspaceRef> {
        let canonical_path: PathBuf = std::fs::canonicalize(path)?;
        Ok(WorkspaceRef { canonical_path })
    }

    pub(crate) fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub(crate) fn identity_kind(&self) -> &'static str {
        IDENTITY_KIND_PATH
    }

    /// The raw bytes hashed into a [`crate::cli::pair_key::PairKey`] and
    /// stored in the `workspaces.canonical_path` column. On Unix this is the
    /// exact `OsStr` byte sequence of the canonical path, never a lossy
    /// `to_string_lossy` projection, so non-UTF-8 workspace paths still hash
    /// deterministically and distinctly.
    pub(crate) fn identity_bytes(&self) -> Vec<u8> {
        os_str_bytes(self.canonical_path.as_os_str())
    }
}

#[cfg(unix)]
fn os_str_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_str_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    // `docs/design.md` targets Linux and macOS only. `as_encoded_bytes`
    // still returns the platform's raw, round-trippable encoding rather
    // than a lossy UTF-8 projection, so this fallback keeps the "never
    // lossy" guarantee on incidental non-Unix builds.
    value.as_encoded_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_canonical_path_for_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = WorkspaceRef::from_dir(dir.path()).unwrap();
        assert_eq!(
            workspace.canonical_path(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert_eq!(workspace.identity_kind(), IDENTITY_KIND_PATH);
    }

    #[test]
    fn missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(WorkspaceRef::from_dir(&missing).is_err());
    }

    #[test]
    fn identity_bytes_match_the_canonical_path_raw_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = WorkspaceRef::from_dir(dir.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(
                workspace.identity_bytes(),
                workspace.canonical_path().as_os_str().as_bytes()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn two_symlinks_to_the_same_directory_produce_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link_a = dir.path().join("link-a");
        let link_b = dir.path().join("link-b");
        std::os::unix::fs::symlink(&target, &link_a).unwrap();
        std::os::unix::fs::symlink(&target, &link_b).unwrap();

        let via_a = WorkspaceRef::from_dir(&link_a).unwrap();
        let via_b = WorkspaceRef::from_dir(&link_b).unwrap();
        let via_target = WorkspaceRef::from_dir(&target).unwrap();

        assert_eq!(via_a, via_b);
        assert_eq!(via_a, via_target);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_component_is_hashed_as_raw_bytes_not_lossy_text() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let raw_name: &OsStr = OsStr::from_bytes(&[0x66, 0x6f, 0xff, 0x6f]);
        let child = dir.path().join(raw_name);
        std::fs::create_dir(&child).unwrap();

        let workspace = WorkspaceRef::from_dir(&child).unwrap();
        let canonical_bytes: Vec<u8> = workspace.canonical_path().as_os_str().as_bytes().to_vec();
        assert_eq!(workspace.identity_bytes(), canonical_bytes);
        // The raw byte sequence must survive verbatim; a lossy projection
        // would have replaced 0xff with U+FFFD's UTF-8 encoding instead.
        assert!(workspace.identity_bytes().windows(1).any(|w| w == [0xff]));
    }

    #[test]
    fn different_directories_produce_different_identity_bytes() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let workspace_a = WorkspaceRef::from_dir(dir_a.path()).unwrap();
        let workspace_b = WorkspaceRef::from_dir(dir_b.path()).unwrap();
        assert_ne!(workspace_a.identity_bytes(), workspace_b.identity_bytes());
    }
}
