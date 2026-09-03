//! End-to-end CLI contract tests that exercise the compiled `subagent`
//! binary as a real subprocess, as opposed to the in-process unit tests in
//! `src/cli/*`. These specifically cover the guarantees a caller of the
//! binary relies on: exit codes, stream separation, the mandatory `--`
//! boundary, managed child execution and continuity, and that persistence is isolated to an explicit
//! `SUBAGENT_STATE_DIR` so no test ever touches the real user state root.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;

const WRAPPER_ERROR_EXIT: i32 = 125;

fn subagent() -> Command {
    Command::cargo_bin("subagent").expect("subagent binary should build")
}

/// Creates a fresh temporary directory secured to owner-only permissions
/// (`0700` on Unix). `tempfile::tempdir` alone does not guarantee this: the
/// directory it creates is subject to the process umask, and this build
/// deliberately rejects a state root with group- or other-accessible
/// permissions, so every test that uses a temporary directory as
/// `SUBAGENT_STATE_DIR` (or as the parent of one) needs this instead of a
/// bare `tempfile::tempdir()`.
fn isolated_state_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
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
/// variable removed and `SUBAGENT_STATE_DIR` pointed at `state_dir`, so a
/// test can opt back in only the supervisor variables it cares about and
/// never persists pair identity outside its own temporary directory.
fn subagent_with_clean_supervisor_env(state_dir: &Path) -> Command {
    let mut command = subagent();
    for var in SUPERVISOR_ENV_VARS {
        command.env_remove(var);
    }
    command.env("SUBAGENT_STATE_DIR", state_dir);
    command
}

/// Same as [`subagent_with_clean_supervisor_env`], but with exactly one
/// resolvable native `CODEX_THREAD_ID` set, for tests that are not
/// themselves about supervisor detection.
fn subagent_with_resolvable_supervisor(state_dir: &Path) -> Command {
    let mut command = subagent_with_clean_supervisor_env(state_dir);
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
fn unsupported_managed_program_exits_125_without_child_output() {
    let state_dir = isolated_state_dir();
    subagent_with_resolvable_supervisor(state_dir.path())
        .args(["--id", "reviewer", "--", "echo", "should-not-run"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("supports only"));
}

/// A real child, if it were spawned, would leave evidence behind on disk.
/// This proves the wrapper never reaches a spawn step for an ordinary
/// managed run, rather than merely asserting on the diagnostic text.
#[cfg(unix)]
#[test]
fn fake_child_is_never_spawned_for_an_ordinary_managed_run() {
    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let canary_path: PathBuf = temp_dir.path().join("canary");
    let script_path: PathBuf = write_canary_script(temp_dir.path(), &canary_path);

    subagent_with_resolvable_supervisor(state_dir.path())
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
    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let canary_path: PathBuf = temp_dir.path().join("canary");
    let script_path: PathBuf = write_canary_script(temp_dir.path(), &canary_path);

    subagent_with_resolvable_supervisor(state_dir.path())
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
#[test]
fn ambiguous_claude_prompt_after_provider_option_fails_before_spawn() {
    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let canary_path: PathBuf = temp_dir.path().join("canary");
    let script_path: PathBuf = write_named_canary_script(temp_dir.path(), &canary_path, "claude");

    subagent_with_resolvable_supervisor(state_dir.path())
        .arg("--id")
        .arg("claude-haiku-reviewer")
        .arg("--")
        .arg(&script_path)
        .args([
            "-p",
            "--model",
            "haiku",
            "--allowedTools",
            "Read",
            "review the current diff",
        ])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("immediately after `-p`"));

    assert!(
        !canary_path.exists(),
        "ambiguous Claude invocation reached the child process"
    );
}

#[cfg(unix)]
#[test]
fn fresh_without_workstream_fails_closed_without_state_or_child_spawn() {
    let temp_dir: tempfile::TempDir = isolated_state_dir();
    let state_root: PathBuf = temp_dir.path().join("state");
    let canary_path: PathBuf = temp_dir.path().join("canary");
    let script_path: PathBuf = write_named_canary_script(temp_dir.path(), &canary_path, "claude");

    subagent_with_resolvable_supervisor(&state_root)
        .args(["--id", "claude-haiku-reviewer", "--fresh", "--"])
        .arg(&script_path)
        .args(["-p", "review the current diff"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("--fresh requires --workstream"));

    assert!(!state_root.exists(), "--fresh created wrapper state");
    assert!(!canary_path.exists(), "--fresh reached the child process");
}

#[cfg(unix)]
#[test]
fn managed_claude_workstream_fresh_then_resume_reuses_the_exact_session() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    let session_file: PathBuf = workspace.path().join("assigned-session");
    fs::write(
        &claude_path,
        "#!/bin/sh\ncase \"$1\" in\n  --session-id)\n    test \"$3\" = '-p' || exit 70\n    test \"$4\" = 'first task' || exit 71\n    printf '%s' \"$2\" > \"$SESSION_FILE\"\n    printf 'FRESH_OK\\n'\n    ;;\n  --resume)\n    test \"$2\" = \"$(cat \"$SESSION_FILE\")\" || exit 72\n    test \"$3\" = '-p' || exit 73\n    test \"$4\" = 'second task' || exit 74\n    printf 'RESUME_OK\\n'\n    ;;\n  *) exit 75 ;;\nesac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    let run = |continuity_flag: &str, task: &str| -> std::process::Output {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .env("SESSION_FILE", &session_file)
            .args([
                "--id",
                "claude-haiku-implementer",
                "--supervisor",
                "codex:workstream-contract",
                "--context",
                "pair",
                "--workstream",
                "issue-42",
                continuity_flag,
                "--quiet",
                "--",
            ])
            .arg(&claude_path)
            .args(["-p", task, "--model", "haiku"])
            .output()
            .unwrap()
    };

    let fresh: std::process::Output = run("--fresh", "first task");
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert_eq!(fresh.stdout, b"FRESH_OK\n");
    let assigned_id: String = fs::read_to_string(&session_file).unwrap();
    assert!(uuid::Uuid::parse_str(&assigned_id).is_ok());

    let resumed: std::process::Output = run("--resume", "second task");
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(resumed.stdout, b"RESUME_OK\n");

    let ledger_path: PathBuf = state_dir.path().join("ledger.sqlite3");
    let connection: rusqlite::Connection = rusqlite::Connection::open(ledger_path).unwrap();
    let (native_id, status, workstream): (String, String, String) = connection
        .query_row(
            "SELECT native_id, status, workstream_id FROM child_sessions WHERE status = 'active'",
            [],
            |row: &rusqlite::Row<'_>| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(native_id, assigned_id);
    assert_eq!(status, "active");
    assert_eq!(workstream, "issue-42");
    let linked_invocations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM invocations WHERE child_session_id = (SELECT id FROM child_sessions WHERE native_id = ?1)",
            [&assigned_id],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(linked_invocations, 2);
}

#[cfg(unix)]
#[test]
fn managed_codex_workstream_observes_and_resumes_the_exact_thread() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let codex_path: PathBuf = workspace.path().join("codex");
    let thread_id: &str = "019d300d-5f1b-7000-8000-000000000042";
    fs::write(
        &codex_path,
        "#!/bin/sh\n\
         test -z \"$CODEX_THREAD_ID\" || exit 69\n\
         test \"$1\" = exec || exit 70\n\
         case \"$2\" in\n\
           --json) test \"$3\" = 'first task' || exit 71; message=FRESH_CODEX ;;\n\
           resume) test \"$3\" = --json || exit 72; test \"$4\" = \"$THREAD_ID\" || exit 73; test \"$5\" = 'second task' || exit 74; message=RESUME_CODEX ;;\n\
           *) exit 75 ;;\n\
         esac\n\
         printf '{\"type\":\"thread.started\",\"thread_id\":\"%s\"}\\n' \"$THREAD_ID\"\n\
         printf '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"%s\"}}\\n' \"$message\"\n\
         printf '{\"type\":\"turn.completed\"}\\n'\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&codex_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex_path, permissions).unwrap();

    let run = |flag: &str, task: &str| -> std::process::Output {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .env("CODEX_THREAD_ID", "must-be-scrubbed")
            .env("THREAD_ID", thread_id)
            .args([
                "--id",
                "gpt-luna-implementer",
                "--supervisor",
                "claude:codex-workstream-contract",
                "--context",
                "pair",
                "--workstream",
                "issue-84",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&codex_path)
            .args(["exec", task, "--model", "gpt-luna"])
            .output()
            .unwrap()
    };

    let fresh: std::process::Output = run("--fresh", "first task");
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert_eq!(fresh.stdout, b"FRESH_CODEX\n");

    let resumed: std::process::Output = run("--resume", "second task");
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(resumed.stdout, b"RESUME_CODEX\n");

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let (native_id, kind, status): (String, String, String) = connection
        .query_row(
            "SELECT native_id, child_kind, status FROM child_sessions WHERE workstream_id = 'issue-84'",
            [],
            |row: &rusqlite::Row<'_>| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(native_id, thread_id);
    assert_eq!(kind, "codex");
    assert_eq!(status, "active");
    let linked_invocations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM invocations WHERE child_session_id = (SELECT id FROM child_sessions WHERE native_id = ?1)",
            [thread_id],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(linked_invocations, 2);
}

#[cfg(unix)]
#[test]
fn managed_opencode_workstream_observes_and_resumes_the_exact_session() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let opencode_path: PathBuf = workspace.path().join("opencode");
    let session_id: &str = "ses_contract-child-001";
    fs::write(
        &opencode_path,
        "#!/bin/sh\n\
         cat >/dev/null\n\
         test \"$1\" = run || exit 70\n\
         test \"$2\" = --format || exit 71\n\
         test \"$3\" = json || exit 72\n\
         case \"$4\" in\n\
           'first task') test \"$5\" = --model || exit 73; test \"$6\" = opencode/big-pickle || exit 74; message=FRESH_OPENCODE ;;\n\
           --session) test \"$5\" = \"$SESSION_ID\" || exit 75; test \"$6\" = 'second task' || exit 76; test \"$7\" = --model || exit 77; test \"$8\" = opencode/big-pickle || exit 78; message=RESUME_OPENCODE ;;\n\
           *) exit 79 ;;\n\
         esac\n\
         printf '{\"type\":\"step_start\",\"sessionID\":\"%s\"}\\n' \"$SESSION_ID\"\n\
         printf '{\"type\":\"text\",\"sessionID\":\"%s\",\"part\":{\"text\":\"%s\"}}\\n' \"$SESSION_ID\" \"$message\"\n\
         printf '{\"type\":\"step_finish\",\"sessionID\":\"%s\"}\\n' \"$SESSION_ID\"\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&opencode_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&opencode_path, permissions).unwrap();

    let run = |flag: &str, task: &str| -> std::process::Output {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .env("SESSION_ID", session_id)
            .args([
                "--id",
                "big-pickle-implementer",
                "--supervisor",
                "opencode:ses_supervisor-contract",
                "--context",
                "pair",
                "--workstream",
                "issue-oc-1",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&opencode_path)
            .args(["run", task, "--model", "opencode/big-pickle"])
            .output()
            .unwrap()
    };

    let fresh: std::process::Output = run("--fresh", "first task");
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert_eq!(fresh.stdout, b"FRESH_OPENCODE\n");

    let resumed: std::process::Output = run("--resume", "second task");
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(resumed.stdout, b"RESUME_OPENCODE\n");

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let (native_id, kind, status): (String, String, String) = connection
        .query_row(
            "SELECT native_id, child_kind, status FROM child_sessions WHERE workstream_id = 'issue-oc-1'",
            [],
            |row: &rusqlite::Row<'_>| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(native_id, session_id);
    assert_eq!(kind, "opencode");
    assert_eq!(status, "active");
    let linked_invocations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM invocations WHERE child_session_id = (SELECT id FROM child_sessions WHERE native_id = ?1)",
            [session_id],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(linked_invocations, 2);
}

#[cfg(unix)]
#[test]
fn managed_antigravity_uses_stream_json_and_resumes_the_exact_conversation() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let agy_path: PathBuf = workspace.path().join("agy");
    let input_path: PathBuf = workspace.path().join("last-input.jsonl");
    let conversation_id: &str = "0222067a-9e42-4b76-9649-66b84fd6bb26";
    fs::write(
        &agy_path,
        "#!/bin/sh\n\
         cat > \"$INPUT_PATH\"\n\
         test \"$1\" = --model || exit 70\n\
         test \"$2\" = gemini-3.8-flash-high || exit 71\n\
         test \"$3\" = --print= || exit 72\n\
         test \"$4\" = --input-format || exit 73\n\
         test \"$5\" = stream-json || exit 74\n\
         test \"$6\" = --output-format || exit 75\n\
         test \"$7\" = stream-json || exit 76\n\
         grep -F '\"event\":\"user\"' \"$INPUT_PATH\" >/dev/null || exit 77\n\
         grep -F 'CURRENT AUTHORITATIVE REQUEST' \"$INPUT_PATH\" >/dev/null || exit 78\n\
         if test \"$8\" = --conversation; then\n\
           test \"$9\" = \"$CONVERSATION_ID\" || exit 79\n\
           grep -F 'second task' \"$INPUT_PATH\" >/dev/null || exit 80\n\
           response=RESUME_AGY\n\
         else\n\
           test -z \"$8\" || exit 81\n\
           grep -F 'first task' \"$INPUT_PATH\" >/dev/null || exit 82\n\
           response=FRESH_AGY\n\
         fi\n\
         printf '{\"event\":\"init\",\"conversation_id\":\"%s\"}\\n' \"$CONVERSATION_ID\"\n\
         printf '{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"%s\",\"step_type\":\"future-compatible\"}}\\n' \"$CONVERSATION_ID\"\n\
         printf '{\"event\":\"result\",\"result\":{\"conversation_id\":\"%s\",\"status\":\"SUCCESS\",\"response\":\"%s\\\\n\"}}\\n' \"$CONVERSATION_ID\" \"$response\"\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&agy_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&agy_path, permissions).unwrap();

    let run = |flag: &str, task: &str| -> std::process::Output {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .env("INPUT_PATH", &input_path)
            .env("CONVERSATION_ID", conversation_id)
            .args([
                "--id",
                "gemini-flash-implementer",
                "--supervisor",
                "antigravity:849c7c61-7baf-4c6b-8767-5704603f08ff",
                "--context",
                "pair",
                "--workstream",
                "issue-agy-1",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&agy_path)
            .args(["-p", task, "--model", "gemini-3.8-flash-high"])
            .output()
            .unwrap()
    };

    let fresh: std::process::Output = run("--fresh", "first task");
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert_eq!(fresh.stdout, b"FRESH_AGY\n");

    let resumed: std::process::Output = run("--resume", "second task");
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(resumed.stdout, b"RESUME_AGY\n");

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let (native_id, kind, status): (String, String, String) = connection
        .query_row(
            "SELECT native_id, child_kind, status FROM child_sessions WHERE workstream_id = 'issue-agy-1'",
            [],
            |row: &rusqlite::Row<'_>| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(native_id, conversation_id);
    assert_eq!(kind, "antigravity");
    assert_eq!(status, "active");
    let linked_invocations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM invocations WHERE child_session_id = (SELECT id FROM child_sessions WHERE native_id = ?1)",
            [conversation_id],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(linked_invocations, 2);
}

#[cfg(unix)]
#[test]
fn managed_opencode_conflicting_resume_invalidates_the_stored_session() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let opencode_path: PathBuf = workspace.path().join("opencode");
    let calls_path: PathBuf = workspace.path().join("calls");
    let session_id: &str = "ses_contract-good";
    let other_id: &str = "ses_contract-conflict";
    fs::write(
        &opencode_path,
        "#!/bin/sh\n\
         cat >/dev/null\n\
         printf x >> \"$CALLS_PATH\"\n\
         if test \"$4\" = --session; then\n\
           printf '{\"type\":\"step_start\",\"sessionID\":\"%s\"}\\n' \"$SESSION_ID\"\n\
           printf '{\"type\":\"text\",\"sessionID\":\"%s\",\"part\":{\"text\":\"CONFLICT\"}}\\n' \"$OTHER_ID\"\n\
           printf '{\"type\":\"step_finish\",\"sessionID\":\"%s\"}\\n' \"$OTHER_ID\"\n\
         else\n\
           printf '{\"type\":\"step_start\",\"sessionID\":\"%s\"}\\n' \"$SESSION_ID\"\n\
           printf '{\"type\":\"text\",\"sessionID\":\"%s\",\"part\":{\"text\":\"FRESH_OK\"}}\\n' \"$SESSION_ID\"\n\
           printf '{\"type\":\"step_finish\",\"sessionID\":\"%s\"}\\n' \"$SESSION_ID\"\n\
         fi\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&opencode_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&opencode_path, permissions).unwrap();

    let run = |flag: &str| -> std::process::Output {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .env("CALLS_PATH", &calls_path)
            .env("SESSION_ID", session_id)
            .env("OTHER_ID", other_id)
            .args([
                "--id",
                "big-pickle-conflict-probe",
                "--supervisor",
                "codex:opencode-conflict-contract",
                "--context",
                "pair",
                "--workstream",
                "conflict-lane",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&opencode_path)
            .args(["run", "task", "--model", "opencode/big-pickle"])
            .output()
            .unwrap()
    };

    let fresh: std::process::Output = run("--fresh");
    assert!(fresh.status.success());
    assert_eq!(fresh.stdout, b"FRESH_OK\n");

    let conflicting: std::process::Output = run("--resume");
    assert!(conflicting.status.success());
    assert!(
        String::from_utf8_lossy(&conflicting.stderr).contains("conflicting top-level sessionIDs")
    );

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let status: String = connection
        .query_row(
            "SELECT status FROM child_sessions WHERE native_id = ?1",
            [session_id],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "invalid");

    let rejected: std::process::Output = run("--resume");
    assert_eq!(rejected.status.code(), Some(WRAPPER_ERROR_EXIT));
    assert_eq!(fs::read(&calls_path).unwrap(), b"xx");
}

#[cfg(unix)]
#[test]
fn managed_codex_preserves_caller_json_and_child_exit_on_observation_failure() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let codex_path: PathBuf = workspace.path().join("codex");
    let thread_id: &str = "019d300d-5f1b-7000-8000-000000000043";
    let valid_jsonl: String = format!(
        "{{\"type\":\"thread.started\",\"thread_id\":\"{thread_id}\"}}\n\
         {{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"JSON_OK\"}}}}\n\
         {{\"type\":\"turn.completed\"}}\n"
    );
    fs::write(
        &codex_path,
        "#!/bin/sh\n\
         case \"$2:$3\" in\n\
           json-task:--json) test \"$4\" != --json || exit 71; printf '%s' \"$VALID_JSONL\" ;;\n\
           --json:malformed-task) printf 'not-json\\n'; exit 42 ;;\n\
           --json:malformed-success) printf 'still-not-json\\n' ;;\n\
           *) exit 72 ;;\n\
         esac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&codex_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex_path, permissions).unwrap();

    let base = |workstream: &str, task: &str| -> Command {
        let mut command: Command = subagent_with_clean_supervisor_env(state_dir.path());
        command
            .current_dir(workspace.path())
            .env("VALID_JSONL", &valid_jsonl)
            .args([
                "--id",
                "gpt-luna-reviewer",
                "--supervisor",
                "claude:codex-json-contract",
                "--context",
                "pair",
                "--workstream",
                workstream,
                "--fresh",
                "--quiet",
                "--",
            ])
            .arg(&codex_path)
            .args(["exec", task]);
        command
    };

    base("caller-json", "json-task")
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::eq(valid_jsonl.clone()));

    base("malformed", "malformed-task")
        .assert()
        .code(42)
        .stdout("not-json\n")
        .stderr(predicate::str::contains(
            "could not confirm Codex native continuity",
        ));

    base("malformed-success", "malformed-success")
        .assert()
        .success()
        .stdout("still-not-json\n")
        .stderr(predicate::str::contains(
            "could not confirm Codex native continuity",
        ));

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let live_sessions: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM child_sessions WHERE status IN ('assigned', 'active')",
            [],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(live_sessions, 1, "malformed output created a live session");
}

#[cfg(unix)]
#[test]
fn managed_codex_resume_invalidates_a_mismatched_native_thread() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let codex_path: PathBuf = workspace.path().join("codex");
    let original_thread: &str = "019d300d-5f1b-7000-8000-000000000044";
    let mismatched_thread: &str = "019d300d-5f1b-7000-8000-000000000045";
    fs::write(
        &codex_path,
        "#!/bin/sh\n\
         test \"$1\" = exec || exit 70\n\
         case \"$2\" in\n\
           --json) thread=\"$ORIGINAL_THREAD\"; message=FRESH_OK ;;\n\
           resume) test \"$3\" = --json || exit 71; test \"$4\" = \"$ORIGINAL_THREAD\" || exit 72; thread=\"$MISMATCHED_THREAD\"; message=WRONG_THREAD ;;\n\
           *) exit 73 ;;\n\
         esac\n\
         printf '{\"type\":\"thread.started\",\"thread_id\":\"%s\"}\\n' \"$thread\"\n\
         printf '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"%s\"}}\\n' \"$message\"\n\
         printf '{\"type\":\"turn.completed\"}\\n'\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&codex_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex_path, permissions).unwrap();

    let run = |flag: &str, task: &str| -> std::process::Output {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .env("ORIGINAL_THREAD", original_thread)
            .env("MISMATCHED_THREAD", mismatched_thread)
            .args([
                "--id",
                "gpt-luna-auditor",
                "--supervisor",
                "claude:codex-mismatch-contract",
                "--context",
                "pair",
                "--workstream",
                "mismatch",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&codex_path)
            .args(["exec", task])
            .output()
            .unwrap()
    };

    let fresh: std::process::Output = run("--fresh", "first");
    assert!(fresh.status.success());
    assert_eq!(fresh.stdout, b"FRESH_OK\n");

    let resumed: std::process::Output = run("--resume", "second");
    assert!(resumed.status.success());
    assert!(String::from_utf8_lossy(&resumed.stdout).contains(mismatched_thread));
    assert!(String::from_utf8_lossy(&resumed.stderr).contains("requires"));

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let (status, reason): (String, String) = connection
        .query_row(
            "SELECT status, retired_reason FROM child_sessions WHERE native_id = ?1",
            [original_thread],
            |row: &rusqlite::Row<'_>| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "invalid");
    assert_eq!(reason, "provider_rejected");
}

#[cfg(unix)]
#[test]
fn resume_missing_or_profile_mismatched_session_fails_before_spawn() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    let canary_path: PathBuf = workspace.path().join("resume-canary");
    fs::write(
        &claude_path,
        "#!/bin/sh\ncase \"$4\" in\n  seed) printf 'SEED_OK\\n' ;;\n  *) : > \"$RESUME_CANARY\"; printf 'UNEXPECTED_SPAWN\\n' ;;\nesac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    let base = |workstream: &str, flag: &str, task: &str, model: &str| -> Command {
        let mut command: Command = subagent_with_clean_supervisor_env(state_dir.path());
        command
            .current_dir(workspace.path())
            .env("RESUME_CANARY", &canary_path)
            .args([
                "--id",
                "claude-haiku-reviewer",
                "--supervisor",
                "codex:resume-errors",
                "--context",
                "pair",
                "--workstream",
                workstream,
                flag,
                "--quiet",
                "--",
            ])
            .arg(&claude_path)
            .args(["-p", task, "--model", model]);
        command
    };

    base("missing", "--resume", "must not run", "haiku")
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("no live native session"));
    assert!(!canary_path.exists());

    base("profile-check", "--fresh", "seed", "haiku")
        .assert()
        .success()
        .stdout("SEED_OK\n");
    base("profile-check", "--resume", "must not run", "sonnet")
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("bound to command profile"));
    assert!(!canary_path.exists());
}

#[cfg(unix)]
#[test]
fn nonzero_fresh_session_stays_unconfirmed_and_cannot_be_resumed() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    let canary_path: PathBuf = workspace.path().join("unconfirmed-canary");
    fs::write(
        &claude_path,
        "#!/bin/sh\ncase \"$1\" in\n  --session-id) exit 42 ;;\n  --resume) : > \"$UNCONFIRMED_CANARY\"; exit 0 ;;\nesac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    let command = |flag: &str| -> Command {
        let mut command: Command = subagent_with_clean_supervisor_env(state_dir.path());
        command
            .current_dir(workspace.path())
            .env("UNCONFIRMED_CANARY", &canary_path)
            .args([
                "--id",
                "claude-haiku-worker",
                "--supervisor",
                "codex:unconfirmed",
                "--context",
                "pair",
                "--workstream",
                "failed-first-run",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&claude_path)
            .args(["-p", "task", "--model", "haiku"]);
        command
    };

    command("--fresh").assert().code(42);
    command("--resume")
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("assigned but never confirmed"));
    assert!(!canary_path.exists());

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let status: String = connection
        .query_row(
            "SELECT status FROM child_sessions WHERE workstream_id = 'failed-first-run'",
            [],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "assigned");
}

#[cfg(unix)]
#[test]
fn fresh_spawn_failure_invalidates_the_assigned_session() {
    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let missing_claude: PathBuf = workspace.path().join("claude");
    let run = |flag: &str| -> Command {
        let mut command: Command = subagent_with_clean_supervisor_env(state_dir.path());
        command
            .current_dir(workspace.path())
            .args([
                "--id",
                "claude-haiku-worker",
                "--supervisor",
                "codex:spawn-failure",
                "--context",
                "pair",
                "--workstream",
                "missing-binary",
                flag,
                "--quiet",
                "--",
            ])
            .arg(&missing_claude)
            .args(["-p", "task", "--model", "haiku"]);
        command
    };

    run("--fresh")
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("failed to spawn child"));

    let connection: rusqlite::Connection =
        rusqlite::Connection::open(state_dir.path().join("ledger.sqlite3")).unwrap();
    let (status, reason): (String, String) = connection
        .query_row(
            "SELECT status, retired_reason FROM child_sessions WHERE workstream_id = 'missing-binary'",
            [],
            |row: &rusqlite::Row<'_>| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "invalid");
    assert_eq!(reason, "provider_rejected");
    drop(connection);

    run("--resume")
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("no live native session"));
}

#[cfg(unix)]
#[test]
fn invalid_workstream_flag_combinations_fail_before_state_or_spawn() {
    let temp_dir: tempfile::TempDir = isolated_state_dir();
    let state_root: PathBuf = temp_dir.path().join("state");
    let canary_path: PathBuf = temp_dir.path().join("continuity-canary");
    let claude_path: PathBuf = write_named_canary_script(temp_dir.path(), &canary_path, "claude");
    let codex_path: PathBuf = write_named_canary_script(temp_dir.path(), &canary_path, "codex");

    let cases: Vec<Vec<OsString>> = vec![
        vec![
            "--id".into(),
            "worker".into(),
            "--workstream".into(),
            "lane".into(),
            "--".into(),
            claude_path.as_os_str().to_os_string(),
            "-p".into(),
            "task".into(),
        ],
        vec![
            "--id".into(),
            "worker".into(),
            "--workstream".into(),
            "lane".into(),
            "--fresh".into(),
            "--resume".into(),
            "--".into(),
            claude_path.as_os_str().to_os_string(),
            "-p".into(),
            "task".into(),
        ],
        vec![
            "--id".into(),
            "worker".into(),
            "--workstream".into(),
            "lane".into(),
            "--fresh".into(),
            "--no-record".into(),
            "--".into(),
            claude_path.as_os_str().to_os_string(),
            "-p".into(),
            "task".into(),
        ],
        vec![
            "--id".into(),
            "worker".into(),
            "--workstream".into(),
            "lane".into(),
            "--fresh".into(),
            "--".into(),
            codex_path.as_os_str().to_os_string(),
            "exec".into(),
            "task".into(),
            "--ephemeral".into(),
        ],
        vec![
            "--id".into(),
            "worker".into(),
            "--workstream".into(),
            "lane".into(),
            "--fresh".into(),
            "--".into(),
            codex_path.as_os_str().to_os_string(),
            "exec".into(),
            "--model".into(),
            "gpt-luna".into(),
            "task".into(),
        ],
    ];

    for args in cases {
        subagent_with_clean_supervisor_env(&state_root)
            .env("CODEX_THREAD_ID", "invalid-continuity")
            .args(args)
            .assert()
            .code(WRAPPER_ERROR_EXIT)
            .stdout(predicate::str::is_empty());
    }
    assert!(!state_root.exists());
    assert!(!canary_path.exists());
}

#[cfg(unix)]
#[test]
fn workstream_dry_run_never_replaces_or_invokes_the_live_session() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    fs::write(&claude_path, "#!/bin/sh\nprintf 'ACTIVE_OK\\n'\n").unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    let base_args: [&str; 9] = [
        "--id",
        "claude-haiku-worker",
        "--supervisor",
        "codex:dry-run-continuity",
        "--context",
        "pair",
        "--workstream",
        "dry-run-lane",
        "--quiet",
    ];
    subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace.path())
        .args(base_args)
        .args(["--fresh", "--"])
        .arg(&claude_path)
        .args(["-p", "seed", "--model", "haiku"])
        .assert()
        .success()
        .stdout("ACTIVE_OK\n");

    let ledger_path: PathBuf = state_dir.path().join("ledger.sqlite3");
    let before: (String, i64) = {
        let connection: rusqlite::Connection = rusqlite::Connection::open(&ledger_path).unwrap();
        let native_id: String = connection
            .query_row(
                "SELECT native_id FROM child_sessions WHERE workstream_id = 'dry-run-lane' AND status = 'active'",
                [],
                |row: &rusqlite::Row<'_>| row.get(0),
            )
            .unwrap();
        let invocation_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM invocations",
                [],
                |row: &rusqlite::Row<'_>| row.get(0),
            )
            .unwrap();
        (native_id, invocation_count)
    };

    for flag in ["--fresh", "--resume"] {
        subagent_with_clean_supervisor_env(state_dir.path())
            .current_dir(workspace.path())
            .args(base_args)
            .args([flag, "--dry-run", "--"])
            .arg(&claude_path)
            .args(["-p", "different task", "--model", "haiku"])
            .assert()
            .success();
    }

    let connection: rusqlite::Connection = rusqlite::Connection::open(ledger_path).unwrap();
    let after_native_id: String = connection
        .query_row(
            "SELECT native_id FROM child_sessions WHERE workstream_id = 'dry-run-lane' AND status = 'active'",
            [],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    let after_invocation_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM invocations",
            [],
            |row: &rusqlite::Row<'_>| row.get(0),
        )
        .unwrap();
    assert_eq!(after_native_id, before.0);
    assert_eq!(after_invocation_count, before.1);
}

#[cfg(unix)]
fn write_canary_script(dir: &Path, canary_path: &Path) -> PathBuf {
    write_named_canary_script(dir, canary_path, "fake-child.sh")
}

#[cfg(unix)]
fn write_named_canary_script(dir: &Path, canary_path: &Path, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path: PathBuf = dir.join(name);
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
    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let report_path: PathBuf = temp_dir.path().join("plan.json");

    subagent_with_resolvable_supervisor(state_dir.path())
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

    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["kind"], "run_plan");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["body"]["id"], "reviewer");
    assert_eq!(report["body"]["context_delivery"], "pointer");
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

    let ensured_pair = &report["body"]["ensured_pair"];
    assert!(!ensured_pair.is_null());
    assert_eq!(ensured_pair["subagent_id"], "reviewer");
    assert_eq!(ensured_pair["provider"], "codex");
    assert_eq!(ensured_pair["pair_key"].as_str().unwrap().len(), 64);

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
    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let report_path: PathBuf = temp_dir.path().join("plan.json");

    subagent_with_clean_supervisor_env(state_dir.path())
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

    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let report_path: PathBuf = temp_dir.path().join("plan.json");
    fs::write(&report_path, "old").unwrap();
    fs::set_permissions(&report_path, fs::Permissions::from_mode(0o644)).unwrap();

    subagent_with_resolvable_supervisor(state_dir.path())
        .args(["--id", "reviewer", "--dry-run", "--report"])
        .arg(&report_path)
        .args(["--", "claude", "-p", "hello"])
        .assert()
        .success();

    let mode: u32 = fs::metadata(report_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn ordinary_run_report_reflects_a_resolved_managed_plan() {
    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let report_path: PathBuf = temp_dir.path().join("plan.json");
    let claude_path: PathBuf = write_fake_claude(temp_dir.path(), "managed-ok");

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(temp_dir.path())
        .args(["--id", "reviewer", "--report"])
        .arg(&report_path)
        .arg("--")
        .arg(&claude_path)
        .args(["-p", "hello"])
        .assert()
        .success()
        .stdout("managed-ok\n");

    let report_text = fs::read_to_string(&report_path).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert_eq!(report["kind"], "run_plan");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["body"]["supervisor"]["provider"], "codex");
    assert!(!report["body"]["ensured_pair"].is_null());
}

#[cfg(unix)]
fn write_fake_claude(dir: &Path, response: &str) -> PathBuf {
    let script: String = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response);
    write_fake_claude_script(dir, &script)
}

#[cfg(unix)]
fn write_fake_claude_script(dir: &Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path: PathBuf = dir.join("claude");
    fs::write(&script_path, script).unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).unwrap();
    script_path
}

#[cfg(unix)]
#[test]
fn managed_child_preserves_binary_stdout_stderr_and_exit_42() {
    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = write_fake_claude_script(
        workspace.path(),
        "#!/bin/sh\ncat >/dev/null\nprintf '\\001\\002\\377A'\nprintf 'child-error' >&2\nexit 42\n",
    );

    let output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["--id", "claude-haiku-exit-contract", "--quiet", "--"])
        .arg(&claude_path)
        .args(["-p", "exercise the process contract"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(42));
    assert_eq!(output.stdout, [0x01_u8, 0x02_u8, 0xff_u8, b'A']);
    assert_eq!(output.stderr, b"child-error");
}

#[cfg(unix)]
#[test]
fn managed_child_signal_is_reproduced_by_the_wrapper() {
    use std::os::unix::process::ExitStatusExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = write_fake_claude_script(
        workspace.path(),
        "#!/bin/sh\ncat >/dev/null\nkill -TERM $$\n",
    );

    let output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["--id", "claude-haiku-signal-contract", "--quiet", "--"])
        .arg(&claude_path)
        .args(["-p", "exercise the signal contract"])
        .output()
        .unwrap();

    assert_eq!(output.status.signal(), Some(libc::SIGTERM));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn required_supervisor_history_failure_happens_before_child_spawn() {
    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let canary_path: PathBuf = workspace.path().join("child-ran");
    let child_path: PathBuf = write_canary_script(workspace.path(), &canary_path);
    let claude_path: PathBuf = workspace.path().join("claude");
    fs::rename(child_path, &claude_path).unwrap();

    subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "claude-sonnet-required-test",
            "--supervisor",
            "claude:required-history-test",
            "--context",
            "supervisor",
            "--context-mode",
            "required",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "must not run"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains(
            "required supervisor history is unavailable",
        ));

    assert!(
        !canary_path.exists(),
        "delegated child ran after required context failure"
    );
}

#[cfg(unix)]
#[test]
fn all_context_degrades_best_effort_and_manifests_unavailable_adapter() {
    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = write_fake_claude(workspace.path(), "BEST_EFFORT_OK");

    subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "claude-sonnet-best-effort-test",
            "--supervisor",
            "claude:best-effort-history-test",
            "--context",
            "all",
            "--quiet",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "continue without supervisor transcript"])
        .assert()
        .success()
        .stdout("BEST_EFFORT_OK\n");

    let context_root: PathBuf = state_dir.path().join("context");
    let capsule_dir: PathBuf = fs::read_dir(&context_root)
        .unwrap()
        .next()
        .expect("one capsule directory")
        .unwrap()
        .path();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(capsule_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["supervisor_history"]["status"], "unavailable");
    assert_eq!(
        manifest["supervisor_history"]["reason_kind"],
        "adapter_not_implemented"
    );
    assert!(manifest["files"].get("supervisor_history").is_none());
}

#[cfg(unix)]
#[test]
fn second_managed_run_receives_the_first_runs_response_in_its_bootstrap() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir = isolated_state_dir();
    let workspace = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    fs::write(
        &claude_path,
        "#!/bin/sh\ninput=$(cat)\ncase \"$input\" in\n  *FIRST_RUN_MARKER*) printf 'CONTINUITY_OK\\n' ;;\n  *) printf 'FIRST_RUN_MARKER\\n' ;;\nesac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    for expected in ["FIRST_RUN_MARKER\n", "CONTINUITY_OK\n"] {
        subagent_with_resolvable_supervisor(state_dir.path())
            .current_dir(workspace.path())
            .args([
                "--id",
                "gpt-sol-worker",
                "--context-delivery",
                "inline",
                "--quiet",
                "--",
            ])
            .arg(&claude_path)
            .args(["-p", "perform the current task"])
            .assert()
            .success()
            .stdout(expected);
    }

    let pairs_output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["pairs", "--format", "json"])
        .output()
        .unwrap();
    assert!(pairs_output.status.success());
    let pairs_report: serde_json::Value = serde_json::from_slice(&pairs_output.stdout).unwrap();
    let pair_key: &str = pairs_report["body"]["pairs"][0]["pair_key"]
        .as_str()
        .unwrap();

    let log_output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["log", "--pair", pair_key, "--format", "json"])
        .output()
        .unwrap();
    assert!(log_output.status.success());
    let log_report: serde_json::Value = serde_json::from_slice(&log_output.stdout).unwrap();
    let exchanges: &Vec<serde_json::Value> = log_report["body"]["exchanges"].as_array().unwrap();
    assert_eq!(exchanges.len(), 4);
    assert_eq!(exchanges[0]["body"]["value"], "perform the current task");
    assert!(
        !exchanges[0]["body"]["value"]
            .as_str()
            .unwrap()
            .contains("-p")
    );

    let context_output: std::process::Output =
        subagent_with_resolvable_supervisor(state_dir.path())
            .current_dir(workspace.path())
            .args(["context", "--pair", pair_key, "--format", "json"])
            .output()
            .unwrap();
    assert!(context_output.status.success());
    let context_report: serde_json::Value = serde_json::from_slice(&context_output.stdout).unwrap();
    let invocations: &Vec<serde_json::Value> =
        context_report["body"]["invocations"].as_array().unwrap();
    assert_eq!(invocations.len(), 2);
    for invocation in invocations {
        let manifest_path: &str = invocation["capsule_path"]["value"].as_str().unwrap();
        assert!(Path::new(manifest_path).is_file());
    }

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["forget", "--pair", pair_key])
        .assert()
        .success()
        .stdout(predicate::str::contains("forgot pair"));
    let remaining_context_entries: usize = fs::read_dir(state_dir.path().join("context"))
        .unwrap()
        .count();
    assert_eq!(remaining_context_entries, 0);
}

#[cfg(unix)]
#[test]
fn structured_provider_response_is_valid_in_history_and_outcome_only_in_summary() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    let provider_json: &str = "{\"result\":\"STRUCTURED_OUTCOME_MARKER\",\"usage\":{\"input_tokens\":17,\"cache_read_input_tokens\":9168},\"api_key\":\"sk-provider-secret\"}";
    fs::write(
        &claude_path,
        format!(
            "#!/bin/sh\ninput=$(cat)\ncase \"$*\" in\n  *establish*) printf '%s\\n' '{provider_json}' ;;\n  *) case \"$input\" in\n       *STRUCTURED_OUTCOME_MARKER*)\n         case \"$input\" in\n           *input_tokens*|*sk-provider-secret*) printf 'ENVELOPE_LEAKED\\n' ;;\n           *) printf 'OUTCOME_ONLY_OK\\n' ;;\n         esac ;;\n       *) printf 'OUTCOME_MISSING\\n' ;;\n     esac ;;\nesac\n"
        ),
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "claude-haiku-structured-reviewer",
            "--context-delivery",
            "inline",
            "--quiet",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "establish structured provider response"])
        .assert()
        .success()
        .stdout(format!("{provider_json}\n"));

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "claude-haiku-structured-reviewer",
            "--context-delivery",
            "inline",
            "--quiet",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "recover the prior outcome"])
        .assert()
        .success()
        .stdout("OUTCOME_ONLY_OK\n");

    let pairs_output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["pairs", "--format", "json"])
        .output()
        .unwrap();
    let pairs_report: serde_json::Value = serde_json::from_slice(&pairs_output.stdout).unwrap();
    let pair_key: &str = pairs_report["body"]["pairs"][0]["pair_key"]
        .as_str()
        .unwrap();
    let log_output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["log", "--pair", pair_key, "--format", "json"])
        .output()
        .unwrap();
    let log_report: serde_json::Value = serde_json::from_slice(&log_output.stdout).unwrap();
    let exchanges: &Vec<serde_json::Value> = log_report["body"]["exchanges"].as_array().unwrap();
    let stored_response_text: &str = exchanges[1]["body"]["value"].as_str().unwrap();
    let stored_response: serde_json::Value = serde_json::from_str(stored_response_text).unwrap();
    assert_eq!(stored_response["result"], "STRUCTURED_OUTCOME_MARKER");
    assert_eq!(stored_response["usage"]["input_tokens"], 17);
    assert_eq!(stored_response["usage"]["cache_read_input_tokens"], 9168);
    assert_eq!(stored_response["api_key"], "[REDACTED]");
}

#[cfg(unix)]
#[test]
fn pointer_delivery_is_default_and_keeps_prior_response_out_of_the_bootstrap() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir: tempfile::TempDir = isolated_state_dir();
    let workspace: tempfile::TempDir = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    fs::write(
        &claude_path,
        "#!/bin/sh\ninput=$(cat)\ncase \"$*\" in\n  *establish*) printf 'PRIOR_POINTER_MARKER\\n' ;;\n  *) case \"$input\" in\n       *PRIOR_POINTER_MARKER*) printf 'STALE_BODY_WAS_INLINED\\n' ;;\n       *'Delivery mode: pointer'*) printf 'POINTER_ONLY_OK\\n' ;;\n       *) printf 'POINTER_MISSING\\n' ;;\n     esac ;;\nesac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["--id", "gpt-sol-worker", "--quiet", "--"])
        .arg(&claude_path)
        .args(["-p", "establish prior response"])
        .assert()
        .success()
        .stdout("PRIOR_POINTER_MARKER\n");

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["--id", "gpt-sol-worker", "--quiet", "--"])
        .arg(&claude_path)
        .args(["-p", "perform a separate task"])
        .assert()
        .success()
        .stdout("POINTER_ONLY_OK\n");

    let context_root: PathBuf = state_dir.path().join("context");
    let mut manifests: Vec<PathBuf> = fs::read_dir(context_root)
        .unwrap()
        .map(|entry: std::io::Result<fs::DirEntry>| entry.unwrap().path().join("manifest.json"))
        .collect();
    manifests.sort();
    let pointer_manifest: serde_json::Value = manifests
        .iter()
        .map(|path: &PathBuf| {
            serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()
        })
        .find(|value: &serde_json::Value| value["context_delivery"] == "pointer")
        .expect("one pointer-delivery manifest");
    assert_eq!(pointer_manifest["schema_version"], 5);
}

#[cfg(unix)]
#[test]
fn inherit_from_persists_one_way_context_for_a_renamed_subagent() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir = isolated_state_dir();
    let workspace = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    fs::write(
        &claude_path,
        "#!/bin/sh\ninput=$(cat)\ncase \"$input\" in\n  *SOURCE_MEMORY_MARKER*) printf 'HANDOFF_OK\\n' ;;\n  *) printf 'SOURCE_MEMORY_MARKER\\n' ;;\nesac\n",
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "gpt-luna-architect",
            "--supervisor",
            "codex:inheritance-test",
            "--context-delivery",
            "inline",
            "--quiet",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "establish source context"])
        .assert()
        .success()
        .stdout("SOURCE_MEMORY_MARKER\n");

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "claude-haiku-architect",
            "--inherit-from",
            "gpt-luna-architect",
            "--supervisor",
            "codex:inheritance-test",
            "--context-delivery",
            "inline",
            "--quiet",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "continue after the model switch"])
        .assert()
        .success()
        .stdout("HANDOFF_OK\n");

    // The explicit edge is durable: later calls use the target id alone.
    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args([
            "--id",
            "claude-haiku-architect",
            "--supervisor",
            "codex:inheritance-test",
            "--context-delivery",
            "inline",
            "--quiet",
            "--",
        ])
        .arg(&claude_path)
        .args(["-p", "continue once more"])
        .assert()
        .success()
        .stdout("HANDOFF_OK\n");

    let pairs_output: std::process::Output = subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace.path())
        .args(["pairs", "--format", "json"])
        .output()
        .unwrap();
    assert!(pairs_output.status.success());
    let pairs_report: serde_json::Value = serde_json::from_slice(&pairs_output.stdout).unwrap();
    let pairs: &Vec<serde_json::Value> = pairs_report["body"]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 2);
    let target: &serde_json::Value = pairs
        .iter()
        .find(|pair| pair["subagent_id"] == "claude-haiku-architect")
        .unwrap();
    assert_eq!(target["inherited_from"], "gpt-luna-architect");
}

#[cfg(unix)]
#[test]
fn cheap_model_summarizer_runs_only_after_the_configured_threshold() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir = isolated_state_dir();
    let workspace = isolated_state_dir();
    let claude_path: PathBuf = workspace.path().join("claude");
    let canary_path: PathBuf = workspace.path().join("summarizer-ran");
    let long_source: String = format!("SOURCE_LONG_MARKER_{}", "X".repeat(256));
    let script: String = format!(
        "#!/bin/sh\ninput=$(cat)\ncase \"$*\" in\n  *--strict-mcp-config*) : > \"$SUMMARIZER_CANARY\"; printf 'MODEL_SUMMARY_MARKER\\n' ;;\n  *) case \"$input\" in\n       *MODEL_SUMMARY_MARKER*) printf 'MODEL_SUMMARY_SEEN\\n' ;;\n       *SOURCE_LONG_MARKER*) printf 'DETERMINISTIC_ONLY\\n' ;;\n       *) printf '%s\\n' '{}' ;;\n     esac ;;\nesac\n",
        long_source
    );
    fs::write(&claude_path, script).unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&claude_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude_path, permissions).unwrap();
    let inherited_path: OsString = std::env::var_os("PATH").unwrap_or_default();
    let search_path: OsString = {
        let mut value: OsString = workspace.path().as_os_str().to_os_string();
        value.push(":");
        value.push(inherited_path);
        value
    };

    let run = |threshold: &str| -> std::process::Output {
        subagent_with_resolvable_supervisor(state_dir.path())
            .current_dir(workspace.path())
            .env("PATH", &search_path)
            .env("SUMMARIZER_CANARY", &canary_path)
            .args([
                "--id",
                "claude-haiku-summarizer-test",
                "--supervisor",
                "codex:summarizer-threshold-test",
                "--summarizer",
                "haiku",
                "--context-delivery",
                "inline",
                "--summarize-above-bytes",
                threshold,
                "--quiet",
                "--",
            ])
            .arg(&claude_path)
            .args(["-p", "continue"])
            .output()
            .unwrap()
    };

    let first: std::process::Output = run("1024");
    assert!(first.status.success());
    assert!(
        String::from_utf8(first.stdout)
            .unwrap()
            .contains("SOURCE_LONG_MARKER")
    );

    let below_threshold: std::process::Output = run("1024");
    assert!(below_threshold.status.success());
    assert_eq!(below_threshold.stdout, b"DETERMINISTIC_ONLY\n");
    assert!(!canary_path.exists());

    let above_threshold: std::process::Output = run("1");
    assert!(above_threshold.status.success());
    assert_eq!(above_threshold.stdout, b"MODEL_SUMMARY_SEEN\n");
    assert!(canary_path.is_file());
}

#[cfg(unix)]
#[test]
fn non_utf8_child_arguments_are_preserved_and_reported_without_lossy_replacement() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let state_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let report_path: PathBuf = temp_dir.path().join("plan.json");
    let non_utf8_bytes: [u8; 4] = [0x66, 0x6f, 0xff, 0x6f];
    let non_utf8_arg = OsStr::from_bytes(&non_utf8_bytes);

    subagent_with_resolvable_supervisor(state_dir.path())
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
fn doctor_text_output_reports_child_spawn_as_implemented() {
    subagent()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("child-spawn"))
        .stdout(predicate::str::contains("implemented"));
}

#[test]
fn doctor_text_output_splits_pair_identity_from_the_exchange_ledger() {
    subagent()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("[implemented "))
        .stdout(predicate::str::contains("pair-identity-store"))
        .stdout(predicate::str::contains("pair-exchange-ledger"));
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
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["kind"], "doctor");
    assert_eq!(report["status"], "ok");
    let capabilities = report["body"]["capabilities"].as_array().unwrap();
    assert!(!capabilities.is_empty());

    let find = |name: &str| -> &serde_json::Value {
        capabilities
            .iter()
            .find(|capability| capability["name"] == name)
            .unwrap_or_else(|| panic!("missing capability {name}"))
    };
    assert_eq!(find("pair-identity-store")["state"], "implemented");
    assert_eq!(find("pair-exchange-ledger")["state"], "implemented");
}

#[test]
fn profile_placeholders_and_invalid_state_commands_create_no_files() {
    let temp_dir = isolated_state_dir();

    let cases: Vec<Vec<&str>> = vec![
        vec!["log", "--pair", "p1"],
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
fn context_locates_the_default_context_root_without_creating_it() {
    let temp_dir = isolated_state_dir();
    let state_root: PathBuf = temp_dir.path().join("state");
    subagent_with_clean_supervisor_env(&state_root)
        .arg("context")
        .assert()
        .success()
        .stdout(predicate::str::contains("context root:"));
    assert!(!state_root.exists());
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
    let state_dir = isolated_state_dir();
    subagent_with_resolvable_supervisor(state_dir.path())
        .args(["--id", "reviewer", "--", "echo", "hi"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty());
}

#[test]
fn explicit_supervisor_takes_precedence_over_a_conflicting_native_env() {
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
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
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
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
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
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
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
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
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("no supervisor identity found"))
        .stderr(predicate::str::contains("--supervisor"));
}

#[test]
fn present_but_empty_native_id_is_rejected_not_silently_accepted() {
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
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

    let state_dir = isolated_state_dir();
    let non_utf8_id: &OsStr = OsStr::from_bytes(&[0xff]);
    subagent_with_clean_supervisor_env(state_dir.path())
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
    let state_dir = isolated_state_dir();
    subagent_with_clean_supervisor_env(state_dir.path())
        .env("SUBAGENT_SELF_REF", "/tmp/does-not-matter.json")
        .env("CODEX_THREAD_ID", "thread-123")
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("SUBAGENT_SELF_REF"))
        .stderr(predicate::str::contains("not implemented"));
}

#[test]
fn memory_workspace_fails_closed_as_unimplemented_on_dry_run() {
    let state_dir = isolated_state_dir();
    let state_root = state_dir.path().join("state");
    subagent_with_resolvable_supervisor(&state_root)
        .args([
            "--id",
            "reviewer",
            "--memory",
            "workspace",
            "--dry-run",
            "--",
            "echo",
        ])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("--memory workspace"))
        .stderr(predicate::str::contains("not implemented"));
    assert!(
        !state_root.exists(),
        "--memory workspace must fail before creating any state"
    );
}

#[test]
fn memory_none_performs_no_persistence() {
    let state_dir = isolated_state_dir();
    let state_root = state_dir.path().join("state");
    subagent_with_resolvable_supervisor(&state_root)
        .args([
            "--id",
            "reviewer",
            "--memory",
            "none",
            "--dry-run",
            "--",
            "echo",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("pair-key:").not());
    assert!(
        !state_root.exists(),
        "--memory none must not create the state root"
    );
}

#[test]
fn dry_run_and_ordinary_run_ensure_and_report_the_same_pair() {
    let state_dir = isolated_state_dir();
    let workspace_dir = isolated_state_dir();
    let temp_dir = isolated_state_dir();
    let dry_run_report_path = temp_dir.path().join("dry-run.json");
    let ordinary_report_path = temp_dir.path().join("ordinary.json");

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace_dir.path())
        .args(["--id", "reviewer", "--dry-run", "--report"])
        .arg(&dry_run_report_path)
        .args(["--", "echo", "hi"])
        .assert()
        .success();

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace_dir.path())
        .args(["--id", "reviewer", "--report"])
        .arg(&ordinary_report_path)
        .args(["--", "echo", "hi"])
        .assert()
        .code(WRAPPER_ERROR_EXIT);

    let dry_run_report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&dry_run_report_path).unwrap()).unwrap();
    let ordinary_report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ordinary_report_path).unwrap()).unwrap();

    assert_eq!(
        dry_run_report["body"]["ensured_pair"]["pair_key"],
        ordinary_report["body"]["ensured_pair"]["pair_key"]
    );
    let canonical_workspace = fs::canonicalize(workspace_dir.path()).unwrap();
    assert_eq!(
        dry_run_report["body"]["ensured_pair"]["workspace"]["encoding"],
        "utf8"
    );
    assert_eq!(
        dry_run_report["body"]["ensured_pair"]["workspace"]["value"],
        canonical_workspace.to_str().unwrap()
    );
}

#[test]
fn repeated_runs_are_idempotent_and_pairs_lists_exactly_one_row() {
    let state_dir = isolated_state_dir();
    let workspace_dir = isolated_state_dir();

    for _ in 0..3 {
        subagent_with_resolvable_supervisor(state_dir.path())
            .current_dir(workspace_dir.path())
            .args(["--id", "reviewer", "--dry-run", "--", "echo", "hi"])
            .assert()
            .success();
    }

    let output = subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace_dir.path())
        .args(["pairs", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let pairs = report["body"]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0]["subagent_id"], "reviewer");
}

#[test]
fn pairs_reports_an_empty_list_and_creates_nothing_for_a_missing_state_root() {
    let state_dir = isolated_state_dir();
    let state_root = state_dir.path().join("state");
    let workspace_dir = isolated_state_dir();

    subagent_with_clean_supervisor_env(&state_root)
        .current_dir(workspace_dir.path())
        .arg("pairs")
        .assert()
        .success()
        .stdout(predicate::str::contains("no pairs recorded"));

    assert!(
        !state_root.exists(),
        "`subagent pairs` must not create the state root when it is missing"
    );
}

#[test]
fn pairs_json_reports_an_empty_array_for_a_missing_state_root() {
    let state_dir = isolated_state_dir();
    let state_root = state_dir.path().join("state");
    let workspace_dir = isolated_state_dir();

    let output = subagent_with_clean_supervisor_env(&state_root)
        .current_dir(workspace_dir.path())
        .args(["pairs", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["status"], "ok");
    assert!(report["body"]["pairs"].as_array().unwrap().is_empty());
    assert!(!state_root.exists());
}

#[test]
fn pairs_lists_the_full_pair_key_without_the_raw_supervisor_session_id() {
    let state_dir = isolated_state_dir();
    let workspace_dir = isolated_state_dir();

    subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace_dir.path())
        .env("CODEX_THREAD_ID", "super-secret-thread-id")
        .args(["--id", "reviewer", "--dry-run", "--", "echo", "hi"])
        .assert()
        .success();

    let output = subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace_dir.path())
        .args(["pairs", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        !String::from_utf8_lossy(&output).contains("super-secret-thread-id"),
        "pairs listing must never include the raw supervisor session id"
    );

    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let pairs = report["body"]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1);
    let pair = &pairs[0];
    assert_eq!(pair["pair_key"].as_str().unwrap().len(), 64);
    assert_eq!(pair["subagent_id"], "reviewer");
    assert_eq!(pair["provider"], "codex");
    assert!(pair["created_at_unix"].is_number());
    assert!(pair["last_seen_unix"].is_number());
}

#[test]
fn pairs_are_isolated_between_different_workspaces() {
    let state_dir = isolated_state_dir();
    let workspace_a = isolated_state_dir();
    let workspace_b = isolated_state_dir();

    subagent_with_resolvable_supervisor(state_dir.path())
        .current_dir(workspace_a.path())
        .args(["--id", "reviewer", "--dry-run", "--", "echo", "hi"])
        .assert()
        .success();

    let output = subagent_with_clean_supervisor_env(state_dir.path())
        .current_dir(workspace_b.path())
        .args(["pairs", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(
        report["body"]["pairs"].as_array().unwrap().is_empty(),
        "a pair recorded for one workspace must not appear when listing another"
    );
}

#[cfg(unix)]
#[test]
fn managed_run_rejects_a_symlinked_state_root() {
    let root = isolated_state_dir();
    let real_state_dir = root.path().join("real-state");
    fs::create_dir(&real_state_dir).unwrap();
    let symlinked_state_dir = root.path().join("state-link");
    std::os::unix::fs::symlink(&real_state_dir, &symlinked_state_dir).unwrap();

    subagent_with_resolvable_supervisor(&symlinked_state_dir)
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("symlink"));
}

#[cfg(unix)]
#[test]
fn managed_run_rejects_a_state_root_with_group_readable_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = isolated_state_dir();
    let state_root = root.path().join("state");
    fs::create_dir(&state_root).unwrap();
    fs::set_permissions(&state_root, fs::Permissions::from_mode(0o750)).unwrap();

    subagent_with_resolvable_supervisor(&state_root)
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("group- or other-accessible"));
}

#[cfg(unix)]
#[test]
fn pairs_rejects_a_symlinked_state_root_instead_of_reporting_an_empty_list() {
    let root = isolated_state_dir();
    let real_state_dir = root.path().join("real-state");
    fs::create_dir(&real_state_dir).unwrap();
    let symlinked_state_dir = root.path().join("state-link");
    std::os::unix::fs::symlink(&real_state_dir, &symlinked_state_dir).unwrap();

    subagent_with_clean_supervisor_env(&symlinked_state_dir)
        .arg("pairs")
        .assert()
        .code(WRAPPER_ERROR_EXIT)
        .stderr(predicate::str::contains("symlink"));
}

#[cfg(unix)]
#[test]
fn state_database_file_is_created_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let state_dir = isolated_state_dir();
    let state_root = state_dir.path().join("state");
    subagent_with_resolvable_supervisor(&state_root)
        .args(["--id", "reviewer", "--dry-run", "--", "echo"])
        .assert()
        .success();

    let dir_mode = fs::metadata(&state_root).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    let db_mode = fs::metadata(state_root.join("ledger.sqlite3"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(db_mode, 0o600);
}
