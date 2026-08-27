//! `subagent pairs [--format text|json]`: a real, read-only listing of every
//! pair (`docs/design.md` section 3.4) recorded for the current workspace.
//!
//! A missing state root or database means "nothing recorded yet" and
//! produces an empty list; this command never creates a directory or file.
//! The listing never includes the raw supervisor session id -- the pair key
//! already binds it irreversibly, and nothing this build implements yet
//! needs to display it back.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::report::{OsStringJson, Report, ReportStatus};
use super::state_dir;
use super::store::{self, OpenForRead};
use super::supervisor::Provider;
use super::workspace::WorkspaceRef;
use super::{OutputFormat, handle_clap_error, wrapper_error_exit};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-pairs", no_binary_name = true)]
struct PairsArgs {
    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize)]
struct PairEntry {
    pair_key: String,
    subagent_id: String,
    inherited_from: Option<String>,
    provider: Provider,
    created_at_unix: i64,
    last_seen_unix: i64,
}

impl From<store::PairSummary> for PairEntry {
    fn from(summary: store::PairSummary) -> Self {
        PairEntry {
            pair_key: summary.pair_key.to_hex(),
            subagent_id: summary.subagent_id,
            inherited_from: summary.inherited_from,
            provider: summary.provider,
            created_at_unix: summary.created_at_unix,
            last_seen_unix: summary.last_seen_unix,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct PairsBody {
    workspace: OsStringJson,
    pairs: Vec<PairEntry>,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let state_dir_override: Option<OsString> = std::env::var_os(state_dir::SUBAGENT_STATE_DIR_ENV);
    let cwd: PathBuf = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(io_error) => {
            let _ = writeln!(
                err,
                "subagent: failed to determine the current working directory: {io_error}"
            );
            return wrapper_error_exit();
        }
    };
    execute_with_env(args, out, err, &cwd, state_dir_override.as_deref())
}

/// Same as [`execute`], but with the working directory and the
/// `SUBAGENT_STATE_DIR` override injected explicitly -- each resolved
/// exactly once, at this process edge -- so tests never touch the real user
/// state root.
fn execute_with_env(
    args: &[OsString],
    out: &mut dyn Write,
    err: &mut dyn Write,
    cwd: &Path,
    state_dir_override: Option<&OsStr>,
) -> ExitCode {
    let pairs_args: PairsArgs = match PairsArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };

    let workspace_ref: WorkspaceRef = match WorkspaceRef::from_dir(cwd) {
        Ok(workspace_ref) => workspace_ref,
        Err(io_error) => {
            let _ = writeln!(
                err,
                "subagent: failed to resolve the workspace identity for {}: {io_error}",
                cwd.display()
            );
            return wrapper_error_exit();
        }
    };

    let state_root: PathBuf = match state_dir::resolve_state_root(state_dir_override) {
        Ok(state_root) => state_root,
        Err(resolution_error) => {
            let _ = writeln!(err, "subagent: {resolution_error}");
            return wrapper_error_exit();
        }
    };

    let pairs: Vec<store::PairSummary> = match store::Store::open_for_read(&state_root) {
        Ok(OpenForRead::Absent) => Vec::new(),
        Ok(OpenForRead::Ready(pair_store)) => {
            match pair_store.list_pairs_for_workspace(&workspace_ref) {
                Ok(pairs) => pairs,
                Err(store_error) => {
                    let _ = writeln!(
                        err,
                        "subagent: failed to read the pair-identity state store at {}: {store_error}",
                        state_root.display()
                    );
                    return wrapper_error_exit();
                }
            }
        }
        Err(store_error) => {
            let _ = writeln!(
                err,
                "subagent: failed to open the pair-identity state store at {}: {store_error}",
                state_root.display()
            );
            return wrapper_error_exit();
        }
    };

    let workspace_display: String = workspace_ref.canonical_path().display().to_string();
    let body = PairsBody {
        workspace: OsStringJson::from_os_str(workspace_ref.canonical_path().as_os_str()),
        pairs: pairs.into_iter().map(PairEntry::from).collect(),
    };

    match pairs_args.format {
        OutputFormat::Json => {
            let report = Report::new("pairs", ReportStatus::Ok, body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            if body.pairs.is_empty() {
                let _ = writeln!(out, "subagent: no pairs recorded for {workspace_display}");
            } else {
                let _ = writeln!(out, "subagent pairs ({workspace_display})");
                for pair in &body.pairs {
                    let _ = writeln!(
                        out,
                        "  {}  subagent={:<20} inherited_from={:<20} provider={:<6} created_at={} last_seen={}",
                        pair.pair_key,
                        pair.subagent_id,
                        pair.inherited_from.as_deref().unwrap_or("-"),
                        pair.provider,
                        pair.created_at_unix,
                        pair.last_seen_unix
                    );
                }
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::id::SubagentId;

    fn ensure(
        state_root: &Path,
        workspace_dir: &Path,
        provider: Provider,
        session_id: &str,
        subagent_id: &str,
    ) -> store::EnsuredPair {
        let workspace_ref = WorkspaceRef::from_dir(workspace_dir).unwrap();
        let mut pair_store = store::Store::open_for_write(state_root).unwrap();
        pair_store
            .ensure_pair(
                &workspace_ref,
                provider,
                session_id,
                &SubagentId::parse(subagent_id).unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn missing_state_root_reports_an_empty_list_and_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_dir = tempfile::tempdir().unwrap();

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(
            &[],
            &mut out,
            &mut err,
            workspace_dir.path(),
            Some(state_root.as_os_str()),
        );

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!state_root.exists());
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("no pairs recorded")
        );
    }

    #[test]
    fn json_format_reports_an_empty_pairs_array_for_a_missing_root() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_dir = tempfile::tempdir().unwrap();

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(
            &[OsString::from("--format"), OsString::from("json")],
            &mut out,
            &mut err,
            workspace_dir.path(),
            Some(state_root.as_os_str()),
        );

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!state_root.exists());
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["status"], "ok");
        assert!(value["body"]["pairs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn lists_a_previously_ensured_pair_without_the_raw_session_id() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_dir = tempfile::tempdir().unwrap();

        let ensured = ensure(
            &state_root,
            workspace_dir.path(),
            Provider::Codex,
            "super-secret-session-id",
            "reviewer",
        );

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(
            &[OsString::from("--format"), OsString::from("json")],
            &mut out,
            &mut err,
            workspace_dir.path(),
            Some(state_root.as_os_str()),
        );

        assert_eq!(code, ExitCode::SUCCESS);
        let raw_output = String::from_utf8(out).unwrap();
        assert!(!raw_output.contains("super-secret-session-id"));

        let value: serde_json::Value = serde_json::from_str(&raw_output).unwrap();
        let pairs = value["body"]["pairs"].as_array().unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0]["pair_key"], ensured.pair_key.to_hex());
        assert_eq!(pairs[0]["subagent_id"], "reviewer");
        assert_eq!(pairs[0]["provider"], "codex");
        assert_eq!(pairs[0]["created_at_unix"], ensured.created_at_unix);
        assert_eq!(pairs[0]["last_seen_unix"], ensured.last_seen_unix);
    }

    #[test]
    fn text_format_lists_the_full_pair_key_and_subagent_id() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_dir = tempfile::tempdir().unwrap();

        let ensured = ensure(
            &state_root,
            workspace_dir.path(),
            Provider::Claude,
            "session-xyz",
            "implementer",
        );

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(
            &[],
            &mut out,
            &mut err,
            workspace_dir.path(),
            Some(state_root.as_os_str()),
        );

        assert_eq!(code, ExitCode::SUCCESS);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(&ensured.pair_key.to_hex()));
        assert!(text.contains("implementer"));
        assert!(text.contains("claude"));
        assert!(!text.contains("session-xyz"));
    }

    #[test]
    fn listing_is_isolated_to_the_current_workspace() {
        let root = tempfile::tempdir().unwrap();
        let state_root = root.path().join("state");
        let workspace_a = tempfile::tempdir().unwrap();
        let workspace_b = tempfile::tempdir().unwrap();

        ensure(
            &state_root,
            workspace_a.path(),
            Provider::Codex,
            "session-a",
            "reviewer",
        );

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(
            &[OsString::from("--format"), OsString::from("json")],
            &mut out,
            &mut err,
            workspace_b.path(),
            Some(state_root.as_os_str()),
        );

        assert_eq!(code, ExitCode::SUCCESS);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(value["body"]["pairs"].as_array().unwrap().is_empty());
    }

    // macOS filesystem APIs reject creation of this invalid UTF-8 name with
    // EILSEQ. The byte-preserving conversion itself has Unix-wide in-memory
    // coverage in workspace.rs; this on-disk integration is Linux-specific.
    #[cfg(target_os = "linux")]
    #[test]
    fn json_preserves_a_non_utf8_workspace_as_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let root = tempfile::tempdir().unwrap();
        let state_root: PathBuf = root.path().join("state");
        let workspace_parent = tempfile::tempdir().unwrap();
        let non_utf8_name: OsString = OsString::from_vec(vec![b'w', b's', b'-', 0xff]);
        let workspace_dir: PathBuf = workspace_parent.path().join(non_utf8_name);
        std::fs::create_dir(&workspace_dir).unwrap();

        ensure(
            &state_root,
            &workspace_dir,
            Provider::Codex,
            "session-a",
            "reviewer",
        );

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code: ExitCode = execute_with_env(
            &[OsString::from("--format"), OsString::from("json")],
            &mut out,
            &mut err,
            &workspace_dir,
            Some(state_root.as_os_str()),
        );

        assert_eq!(code, ExitCode::SUCCESS);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["body"]["workspace"]["encoding"], "bytes");
        assert!(value["body"]["workspace"]["value"].is_array());
    }
}
