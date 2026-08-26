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
    use std::os::unix::fs::PermissionsExt;

    let script_path: PathBuf = dir.join("claude");
    fs::write(
        &script_path,
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response),
    )
    .unwrap();
    let mut permissions: fs::Permissions = fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).unwrap();
    script_path
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
            .args(["--id", "gpt-sol-worker", "--quiet", "--"])
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
    assert_eq!(report["schema_version"], 1);
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
