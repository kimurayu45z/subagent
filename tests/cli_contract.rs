//! End-to-end CLI contract tests that exercise the compiled `subagent`
//! binary as a real subprocess, as opposed to the in-process unit tests in
//! `src/cli/*`. These specifically cover the guarantees a caller of the
//! binary relies on: exit codes, stream separation, the mandatory `--`
//! boundary, and that a managed run never spawns the child process in this
//! milestone.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;

const WRAPPER_ERROR_EXIT: i32 = 125;

fn subagent() -> Command {
    Command::cargo_bin("subagent").expect("subagent binary should build")
}

/// Every supervisor-detection environment variable this build inspects.
/// Cleared before setting any of them explicitly so the ambient shell that
/// runs the test suite (which may itself be a Codex or Claude Code session)
/// can never make supervisor resolution non-deterministic.
const SUPERVISOR_ENV_VARS: [&str; 3] = [
    "SUBAGENT_SELF_REF",
    "CODEX_THREAD_ID",
    "CLAUDE_CODE_SESSION_ID",
];

/// A `subagent()` command with every supervisor-detection environment
/// variable removed, so a test can opt back in only the variables it cares
/// about.
fn subagent_with_clean_supervisor_env() -> Command {
    let mut command = subagent();
    for var in SUPERVISOR_ENV_VARS {
        command.env_remove(var);
    }
    command
}

/// Same as [`subagent_with_clean_supervisor_env`], but with exactly one
/// resolvable native `CODEX_THREAD_ID` set, for tests that are not
/// themselves about supervisor detection.
fn subagent_with_resolvable_supervisor() -> Command {
    let mut command = subagent_with_clean_supervisor_env();
    command.env("CODEX_THREAD_ID", "contract-test-thread");
    command
}

#[test]
fn help_flag_prints_usage_to_stdout_and_succeeds() {
    subagent()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("subagent --id ID"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn version_flag_prints_version_to_stdout_and_succeeds() {
    subagent()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("subagent "))
        .stderr(predicate::str::is_empty());
}

#[test]
fn no_arguments_is_a_wrapper_error() {
    subagent()
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty());
}

#[test]
fn missing_double_dash_boundary_is_a_wrapper_error() {
    subagent()
        .args(["--id", "reviewer"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("--"));
}

#[test]
fn invalid_id_is_rejected_with_a_clear_diagnostic() {
    subagent()
        .args(["--id", "not a valid id", "--", "echo", "hi"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("invalid subagent id"));
}

#[test]
fn ordinary_managed_run_exits_125_with_backend_diagnostic_and_no_child_output() {
    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--", "echo", "should-not-run"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("backend not implemented"));
}

/// A real child, if it were spawned, would leave evidence behind on disk.
/// This proves the wrapper never reaches a spawn step for an ordinary
/// managed run, rather than merely asserting on the diagnostic text.
#[cfg(unix)]
#[test]
fn fake_child_is_never_spawned_for_an_ordinary_managed_run() {
    let temp_dir = tempfile::tempdir().unwrap();
    let canary_path: PathBuf = temp_dir.path().join("canary");
    let script_path: PathBuf = write_canary_script(temp_dir.path(), &canary_path);

    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--", script_path.to_str().unwrap()])
        .assert()
        .code(WRAPPER_ERROR_EXIT);

    assert!(
        !canary_path.exists(),
        "child script ran and wrote the canary file"
    );
}

#[cfg(unix)]
#[test]
fn fake_child_is_never_spawned_during_dry_run() {
    let temp_dir = tempfile::tempdir().unwrap();
    let canary_path: PathBuf = temp_dir.path().join("canary");
    let script_path: PathBuf = write_canary_script(temp_dir.path(), &canary_path);

    subagent_with_resolvable_supervisor()
        .args([
            "--id",
            "reviewer",
            "--dry-run",
            "--",
            script_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(
        !canary_path.exists(),
        "child script ran and wrote the canary file"
    );
}

#[cfg(unix)]
fn write_canary_script(dir: &Path, canary_path: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("fake-child.sh");
    fs::write(
        &script_path,
        format!("#!/bin/sh\ntouch '{}'\n", canary_path.display()),
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();
    script_path
}

#[test]
fn dry_run_writes_a_json_plan_report_preserving_child_arguments_verbatim() {
    let temp_dir = tempfile::tempdir().unwrap();
    let report_path: PathBuf = temp_dir.path().join("plan.json");

    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--dry-run", "--report"])
        .arg(&report_path)
        .args([
            "--",
            "claude",
            "-p",
            "--id",
            "sneaky",
            "--dry-run",
            "--",
            "prompt with spaces",
        ])
        .assert()
        .success();

    let report_text = fs::read_to_string(&report_path).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["kind"], "run_plan");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["body"]["id"], "reviewer");
    assert_eq!(report["body"]["program"]["encoding"], "utf8");
    assert_eq!(report["body"]["program"]["value"], "claude");
    assert_eq!(report["body"]["supervisor"]["provider"], "codex");
    assert_eq!(
        report["body"]["supervisor"]["session_id"],
        "contract-test-thread"
    );
    assert_eq!(report["body"]["supervisor"]["detected_via"], "native_env");
    assert_eq!(report["body"]["supervisor"]["confidence"], "exact");
    assert!(report["body"]["supervisor_override"].is_null());

    let args = report["body"]["args"].as_array().unwrap();
    let expected = [
        "-p",
        "--id",
        "sneaky",
        "--dry-run",
        "--",
        "prompt with spaces",
    ];
    assert_eq!(args.len(), expected.len());
    for (actual, expected_value) in args.iter().zip(expected.iter()) {
        assert_eq!(actual["encoding"], "utf8");
        assert_eq!(actual["value"], *expected_value);
    }
}

#[test]
fn dry_run_report_reflects_an_explicit_supervisor_override() {
    let temp_dir = tempfile::tempdir().unwrap();
    let report_path: PathBuf = temp_dir.path().join("plan.json");

    subagent_with_clean_supervisor_env()
        .args(["--id", "reviewer", "--supervisor", "claude:abc123"])
        .args(["--dry-run", "--report"])
        .arg(&report_path)
        .args(["--", "claude", "-p", "hello"])
        .assert()
        .success();

    let report_text = fs::read_to_string(&report_path).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert_eq!(report["body"]["supervisor"]["provider"], "claude");
    assert_eq!(report["body"]["supervisor"]["session_id"], "abc123");
    assert_eq!(report["body"]["supervisor"]["detected_via"], "explicit");
    assert_eq!(report["body"]["supervisor_override"], "claude:abc123");
}

#[cfg(unix)]
#[test]
fn plan_report_is_written_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let report_path: PathBuf = temp_dir.path().join("plan.json");
    fs::write(&report_path, "old").unwrap();
    fs::set_permissions(&report_path, fs::Permissions::from_mode(0o644)).unwrap();

    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--dry-run", "--report"])
        .arg(&report_path)
        .args(["--", "claude", "-p", "hello"])
        .assert()
        .success();

    let mode: u32 = fs::metadata(report_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn ordinary_run_report_reflects_backend_unavailable_status() {
    let temp_dir = tempfile::tempdir().unwrap();
    let report_path: PathBuf = temp_dir.path().join("plan.json");

    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--report"])
        .arg(&report_path)
        .args(["--", "claude", "-p", "hello"])
        .assert()
        .code(WRAPPER_ERROR_EXIT);

    let report_text = fs::read_to_string(&report_path).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert_eq!(report["kind"], "run_backend_unavailable");
    assert_eq!(report["status"], "error");
    assert_eq!(report["body"]["supervisor"]["provider"], "codex");
}

#[cfg(unix)]
#[test]
fn non_utf8_child_arguments_are_preserved_and_reported_without_lossy_replacement() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let report_path: PathBuf = temp_dir.path().join("plan.json");
    let non_utf8_bytes: [u8; 4] = [0x66, 0x6f, 0xff, 0x6f];
    let non_utf8_arg = OsStr::from_bytes(&non_utf8_bytes);

    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--dry-run", "--report"])
        .arg(&report_path)
        .arg("--")
        .arg("echo")
        .arg(non_utf8_arg)
        .assert()
        .success();

    let report_text = fs::read_to_string(&report_path).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    let args = report["body"]["args"].as_array().unwrap();
    assert_eq!(args.len(), 1);
    assert_eq!(args[0]["encoding"], "bytes");
    let bytes: Vec<u8> = args[0]["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(bytes, non_utf8_bytes);
}

#[test]
fn doctor_text_output_reports_child_spawn_as_unavailable() {
    subagent()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("child-spawn"))
        .stdout(predicate::str::contains("unavailable"));
}

#[test]
fn doctor_json_output_is_a_valid_versioned_report() {
    let output = subagent()
        .args(["doctor", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["kind"], "doctor");
    assert_eq!(report["status"], "ok");
    assert!(
        !report["body"]["capabilities"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stateful_placeholder_commands_report_unavailable_without_creating_files() {
    let temp_dir = tempfile::tempdir().unwrap();

    let cases: Vec<Vec<&str>> = vec![
        vec!["context"],
        vec!["log", "--pair", "p1"],
        vec!["pairs"],
        vec!["forget", "--pair", "p1"],
        vec!["agent", "list"],
        vec!["agent", "remove", "reviewer"],
        vec!["agent", "add", "reviewer", "--", "claude", "-p"],
    ];

    for case in cases {
        subagent()
            .current_dir(temp_dir.path())
            .args(&case)
            .assert()
            .code(WRAPPER_ERROR_EXIT)
            .stdout(predicate::str::is_empty());

        let entries: Vec<_> = fs::read_dir(temp_dir.path()).unwrap().collect();
        assert!(
            entries.is_empty(),
            "command {case:?} created files: {entries:?}"
        );
    }
}

#[test]
fn agent_add_requires_double_dash_boundary() {
    subagent()
        .args(["agent", "add", "reviewer", "claude", "-p"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("--"));
}

#[test]
fn agent_help_lists_available_profile_commands() {
    subagent()
        .args(["agent", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent add"))
        .stdout(predicate::str::contains("agent remove"))
        .stdout(predicate::str::contains("agent list"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn child_stdout_is_never_mixed_with_wrapper_diagnostics() {
    // Even in the failure path, wrapper diagnostics must land on stderr
    // only; stdout is reserved for eventual child output and machine
    // reports explicitly requested via `--report`.
    subagent_with_resolvable_supervisor()
        .args(["--id", "reviewer", "--", "echo", "hi"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty());
}

#[test]
fn explicit_supervisor_takes_precedence_over_a_conflicting_native_env() {
    subagent_with_clean_supervisor_env()
        .env("CODEX_THREAD_ID", "thread-from-env")
        .args([
            "--id",
            "reviewer",
            "--supervisor",
            "claude:override-session",
            "--dry-run",
            "--",
            "echo",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "claude:override-session (via explicit)",
        ));
}

#[test]
fn native_codex_thread_id_is_detected_when_unambiguous() {
    subagent_with_clean_supervisor_env()
        .env("CODEX_THREAD_ID", "thread-123")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "codex:thread-123 (via native-env)",
        ));
}

#[test]
fn native_claude_session_id_is_detected_when_unambiguous() {
    subagent_with_clean_supervisor_env()
        .env("CLAUDE_CODE_SESSION_ID", "session-123")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "claude:session-123 (via native-env)",
        ));
}

#[test]
fn both_native_ids_present_is_rejected_as_ambiguous() {
    subagent_with_clean_supervisor_env()
        .env("CODEX_THREAD_ID", "thread-123")
        .env("CLAUDE_CODE_SESSION_ID", "session-123")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("cannot be inferred safely"))
        .stderr(predicate::str::contains("thread-123").not())
        .stderr(predicate::str::contains("session-123").not());
}

#[test]
fn missing_supervisor_identity_fails_with_an_actionable_diagnostic() {
    subagent_with_clean_supervisor_env()
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("no supervisor identity found"))
        .stderr(predicate::str::contains("--supervisor"));
}

#[test]
fn present_but_empty_native_id_is_rejected_not_silently_accepted() {
    subagent_with_clean_supervisor_env()
        .env("CODEX_THREAD_ID", "")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("CODEX_THREAD_ID"))
        .stderr(predicate::str::contains("set but empty"));
}

#[cfg(unix)]
#[test]
fn non_utf8_native_id_is_rejected_instead_of_ignoring_ambiguity() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let non_utf8_id: &OsStr = OsStr::from_bytes(&[0xff]);
    subagent_with_clean_supervisor_env()
        .env("CODEX_THREAD_ID", non_utf8_id)
        .env("CLAUDE_CODE_SESSION_ID", "session-123")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("CODEX_THREAD_ID"))
        .stderr(predicate::str::contains("non-UTF-8"));
}

#[test]
fn managed_ref_present_fails_closed_instead_of_falling_through_to_native_env() {
    subagent_with_clean_supervisor_env()
        .env("SUBAGENT_SELF_REF", "/tmp/does-not-matter.json")
        .env("CODEX_THREAD_ID", "thread-123")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("SUBAGENT_SELF_REF"))
        .stderr(predicate::str::contains("not implemented"));
}
