//! The canonical `subagent --id ID [RUN-OPTIONS] -- COMMAND [ARG...]` form.
//!
//! This module resolves and validates the run plan described in
//! `docs/design.md` sections 6.2 and 7. Conversation-scoped pair identity
//! (`docs/design.md` section 3.4) is ensured and reported for both
//! `--dry-run` and an ordinary run, since recording that a pair exists is
//! preparation, not an exchange record. `--dry-run` prints the resolved
//! plan and exits successfully without spawning anything. An ordinary
//! (non-dry-run) managed invocation never spawns the child either; it exits
//! `125` with an explicit "backend not implemented" diagnostic, per this
//! milestone's requirement to be honest about what is and is not built.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::id::SubagentId;
use super::report::{OsStringJson, Report, ReportStatus, write_json_atomic};
use super::state_dir;
use super::store;
use super::supervisor::{self, DetectionEnv, Provider, SupervisorRef};
use super::workspace::WorkspaceRef;
use super::{handle_clap_error, split_on_double_dash, wrapper_error_exit};

const SUBAGENT_ID_ENV: &str = "SUBAGENT_ID";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MemoryMode {
    #[default]
    Conversation,
    Workspace,
    None,
}

impl fmt::Display for MemoryMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            MemoryMode::Conversation => "conversation",
            MemoryMode::Workspace => "workspace",
            MemoryMode::None => "none",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContextScope {
    Pair,
    Supervisor,
    All,
    None,
}

impl fmt::Display for ContextScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            ContextScope::Pair => "pair",
            ContextScope::Supervisor => "supervisor",
            ContextScope::All => "all",
            ContextScope::None => "none",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContextMode {
    Required,
    BestEffort,
}

impl fmt::Display for ContextMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            ContextMode::Required => "required",
            ContextMode::BestEffort => "best-effort",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum SummarizerChoice {
    Deterministic,
    None,
    Alias(String),
}

impl SummarizerChoice {
    fn resolve(raw: Option<&str>) -> Self {
        match raw {
            None | Some("deterministic") => SummarizerChoice::Deterministic,
            Some("none") => SummarizerChoice::None,
            Some(alias) => SummarizerChoice::Alias(alias.to_string()),
        }
    }
}

impl fmt::Display for SummarizerChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SummarizerChoice::Deterministic => f.write_str("deterministic"),
            SummarizerChoice::None => f.write_str("none"),
            SummarizerChoice::Alias(alias) => write!(f, "alias:{alias}"),
        }
    }
}

/// Wrapper flags accepted before the explicit `--` boundary. Parsed with
/// `clap` so malformed flags produce clap's standard diagnostics; the `--`
/// boundary itself, and everything after it, is handled outside `clap` (see
/// [`super::split_on_double_dash`]) so a child argument can never be
/// misparsed as a wrapper option.
#[derive(Debug, Clone, Parser)]
#[command(name = "subagent", version = env!("CARGO_PKG_VERSION"), no_binary_name = true)]
struct RunArgs {
    #[arg(long)]
    id: Option<String>,

    #[arg(long)]
    supervisor: Option<String>,

    #[arg(long, value_enum)]
    memory: Option<MemoryMode>,

    #[arg(long, value_enum)]
    context: Option<ContextScope>,

    #[arg(long = "context-mode", value_enum)]
    context_mode: Option<ContextMode>,

    #[arg(long)]
    summarizer: Option<String>,

    #[arg(long = "max-context-bytes")]
    max_context_bytes: Option<u64>,

    #[arg(long)]
    fresh: bool,

    #[arg(long = "no-record")]
    no_record: bool,

    #[arg(long = "dry-run")]
    dry_run: bool,

    #[arg(long)]
    quiet: bool,

    /// Not part of `docs/design.md` section 6.2's "principal run options"
    /// list; see the README-equivalent discussion in the implementation
    /// notes for why an explicit machine-report destination was added for
    /// this milestone.
    #[arg(long)]
    report: Option<PathBuf>,
}

/// The subset of an ensured pair (`docs/design.md` section 3.4) reported to
/// the caller. Never includes the raw supervisor session id string, even
/// though the process resolving it obviously has it; the pair key already
/// binds that session id irreversibly, and the plan/report surfaces do not
/// need to restate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EnsuredPairReport {
    pair_key: String,
    workspace: OsStringJson,
    subagent_id: String,
    provider: Provider,
    created_at_unix: i64,
    last_seen_unix: i64,
}

impl From<&store::EnsuredPair> for EnsuredPairReport {
    fn from(pair: &store::EnsuredPair) -> Self {
        EnsuredPairReport {
            pair_key: pair.pair_key.to_hex(),
            workspace: OsStringJson::from_os_str(pair.workspace.as_os_str()),
            subagent_id: pair.subagent_id.clone(),
            provider: pair.provider,
            created_at_unix: pair.created_at_unix,
            last_seen_unix: pair.last_seen_unix,
        }
    }
}

/// The fully resolved plan for a managed run: wrapper defaults applied,
/// `--id` validated, and the child command preserved as `OsString`.
#[derive(Debug, Clone)]
struct RunPlan {
    id: Option<SubagentId>,
    supervisor: SupervisorRef,
    supervisor_override: Option<String>,
    memory: MemoryMode,
    context: ContextScope,
    context_mode: ContextMode,
    summarizer: SummarizerChoice,
    max_context_bytes: Option<u64>,
    fresh: bool,
    no_record: bool,
    quiet: bool,
    program: OsString,
    args: Vec<OsString>,
    ensured_pair: Option<store::EnsuredPair>,
}

#[derive(Debug, Clone, Serialize)]
struct RunPlanReport {
    id: Option<String>,
    supervisor: SupervisorRef,
    supervisor_override: Option<String>,
    memory: MemoryMode,
    context: ContextScope,
    context_mode: ContextMode,
    summarizer: SummarizerChoice,
    max_context_bytes: Option<u64>,
    fresh: bool,
    no_record: bool,
    quiet: bool,
    program: OsStringJson,
    args: Vec<OsStringJson>,
    ensured_pair: Option<EnsuredPairReport>,
}

impl From<&RunPlan> for RunPlanReport {
    fn from(plan: &RunPlan) -> Self {
        RunPlanReport {
            id: plan.id.as_ref().map(|id| id.as_str().to_string()),
            supervisor: plan.supervisor.clone(),
            supervisor_override: plan.supervisor_override.clone(),
            memory: plan.memory,
            context: plan.context,
            context_mode: plan.context_mode,
            summarizer: plan.summarizer.clone(),
            max_context_bytes: plan.max_context_bytes,
            fresh: plan.fresh,
            no_record: plan.no_record,
            quiet: plan.quiet,
            program: OsStringJson::from_os_str(&plan.program),
            args: plan
                .args
                .iter()
                .map(|arg| OsStringJson::from_os_str(arg))
                .collect(),
            ensured_pair: plan.ensured_pair.as_ref().map(EnsuredPairReport::from),
        }
    }
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let subagent_id_env: Option<String> = std::env::var(SUBAGENT_ID_ENV).ok();
    let detection_env: DetectionEnv = DetectionEnv::from_process_env();
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
    execute_with_env(
        args,
        out,
        err,
        subagent_id_env.as_deref(),
        &detection_env,
        &cwd,
        state_dir_override.as_deref(),
    )
}

/// Same as [`execute`], but with the `SUBAGENT_ID` environment lookup, the
/// supervisor-detection environment, the working directory, and the
/// `SUBAGENT_STATE_DIR` override all injected explicitly -- each resolved
/// exactly once, at this process edge -- so tests never need to mutate real
/// process environment state or touch the real user state root.
fn execute_with_env(
    args: &[OsString],
    out: &mut dyn Write,
    err: &mut dyn Write,
    subagent_id_env: Option<&str>,
    detection_env: &DetectionEnv,
    cwd: &Path,
    state_dir_override: Option<&OsStr>,
) -> ExitCode {
    let split = split_on_double_dash(args);

    let run_args: RunArgs = match RunArgs::try_parse_from(split.before.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };

    let child_tokens: &[OsString] = match split.after {
        Some(tokens) => tokens,
        None => {
            let _ = writeln!(
                err,
                "subagent: missing required `--` boundary before the child command"
            );
            let _ = writeln!(err, "usage: subagent --id ID [OPTIONS] -- COMMAND [ARG...]");
            return wrapper_error_exit();
        }
    };
    if child_tokens.is_empty() {
        let _ = writeln!(err, "subagent: no child command given after `--`");
        return wrapper_error_exit();
    }

    let id: Option<SubagentId> = match resolve_id(run_args.id.as_deref(), subagent_id_env, err) {
        Ok(id) => id,
        Err(code) => return code,
    };

    let memory: MemoryMode = run_args.memory.unwrap_or_default();
    if id.is_none() && (memory != MemoryMode::None || !run_args.no_record) {
        let _ = writeln!(
            err,
            "subagent: --id (or SUBAGENT_ID) is required unless both --memory none and --no-record are set; this build can inspect that unidentified plan only with --dry-run"
        );
        return wrapper_error_exit();
    }
    if memory == MemoryMode::Workspace {
        let _ = writeln!(
            err,
            "subagent: --memory workspace is not implemented in this build (docs/design.md \
             section 3.4's WorkspaceMemoryKey); re-run with --memory conversation (the default) \
             or --memory none"
        );
        return wrapper_error_exit();
    }

    let supervisor: SupervisorRef =
        match supervisor::resolve(run_args.supervisor.as_deref(), detection_env) {
            Ok(supervisor) => supervisor,
            Err(resolution_error) => {
                let _ = writeln!(err, "subagent: {resolution_error}");
                return wrapper_error_exit();
            }
        };

    let ensured_pair: Option<store::EnsuredPair> = match memory {
        MemoryMode::None => None,
        MemoryMode::Workspace => unreachable!("--memory workspace already handled above"),
        MemoryMode::Conversation => {
            // `id` is guaranteed `Some` here: `memory != MemoryMode::None` and
            // the check above already rejected a missing `--id` in that case.
            let subagent_id: &SubagentId = id
                .as_ref()
                .expect("--memory conversation requires --id, which is enforced before this point");
            match ensure_conversation_pair(cwd, state_dir_override, &supervisor, subagent_id) {
                Ok(pair) => Some(pair),
                Err(message) => {
                    let _ = writeln!(err, "subagent: {message}");
                    return wrapper_error_exit();
                }
            }
        }
    };

    let plan = RunPlan {
        id,
        supervisor,
        supervisor_override: run_args.supervisor.clone(),
        memory,
        context: run_args.context.unwrap_or(ContextScope::All),
        context_mode: run_args.context_mode.unwrap_or(ContextMode::Required),
        summarizer: SummarizerChoice::resolve(run_args.summarizer.as_deref()),
        max_context_bytes: run_args.max_context_bytes,
        fresh: run_args.fresh,
        no_record: run_args.no_record,
        quiet: run_args.quiet,
        program: child_tokens[0].clone(),
        args: child_tokens[1..].to_vec(),
        ensured_pair,
    };

    if let Some(report_path) = &run_args.report {
        let status: ReportStatus = if run_args.dry_run {
            ReportStatus::Ok
        } else {
            ReportStatus::Error
        };
        let kind: &str = if run_args.dry_run {
            "run_plan"
        } else {
            "run_backend_unavailable"
        };
        let report = Report::new(kind, status, RunPlanReport::from(&plan));
        if let Err(io_error) = write_json_atomic(report_path, &report) {
            let _ = writeln!(
                err,
                "subagent: failed to write report to {}: {io_error}",
                report_path.display()
            );
            return wrapper_error_exit();
        }
    }

    if !plan.quiet {
        print_human_plan(&plan, run_args.dry_run, err);
    }

    if run_args.dry_run {
        ExitCode::SUCCESS
    } else {
        let _ = writeln!(
            err,
            "subagent: backend not implemented: the pair exchange ledger, the context capsule, and child process spawning are not implemented in this build"
        );
        let _ = writeln!(
            err,
            "subagent: no child process was started; re-run with --dry-run to inspect the resolved plan without this error"
        );
        wrapper_error_exit()
    }
}

/// Resolves the canonical workspace identity and the on-disk pair-identity
/// store, then idempotently ensures the conversation-scoped pair
/// (`docs/design.md` section 3.4) for `supervisor`/`subagent_id` exists.
/// Returns a plain diagnostic message (never a raw supervisor session id;
/// callers only need to know the operation failed) so it can be printed
/// with a uniform `subagent: {message}` prefix at the call site.
fn ensure_conversation_pair(
    cwd: &Path,
    state_dir_override: Option<&OsStr>,
    supervisor: &SupervisorRef,
    subagent_id: &SubagentId,
) -> Result<store::EnsuredPair, String> {
    let workspace_ref: WorkspaceRef = WorkspaceRef::from_dir(cwd).map_err(|io_error| {
        format!(
            "failed to resolve the workspace identity for {}: {io_error}",
            cwd.display()
        )
    })?;
    let state_root: PathBuf = state_dir::resolve_state_root(state_dir_override)
        .map_err(|resolution_error| resolution_error.to_string())?;
    let mut pair_store: store::Store =
        store::Store::open_for_write(&state_root).map_err(|store_error: store::StoreError| {
            format!(
                "failed to open the pair-identity state store at {}: {store_error}",
                state_root.display()
            )
        })?;
    pair_store
        .ensure_pair(
            &workspace_ref,
            supervisor.provider,
            &supervisor.session_id,
            subagent_id,
        )
        .map_err(|store_error: store::StoreError| {
            format!("failed to record the pair identity: {store_error}")
        })
}

fn resolve_id(
    explicit: Option<&str>,
    env: Option<&str>,
    err: &mut dyn Write,
) -> Result<Option<SubagentId>, ExitCode> {
    match explicit.or(env) {
        None => Ok(None),
        Some(raw_id) => match SubagentId::parse(raw_id) {
            Ok(id) => Ok(Some(id)),
            Err(invalid) => {
                let _ = writeln!(err, "subagent: {invalid}");
                Err(wrapper_error_exit())
            }
        },
    }
}

fn print_human_plan(plan: &RunPlan, dry_run: bool, err: &mut dyn Write) {
    let heading: &str = if dry_run {
        "dry-run plan (no child was started)"
    } else {
        "run plan"
    };
    let _ = writeln!(err, "subagent: {heading}");
    let id_display: String = plan
        .id
        .as_ref()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "<none>".to_string());
    let _ = writeln!(err, "  id:                {id_display}");
    let _ = writeln!(err, "  supervisor:        {}", plan.supervisor);
    let _ = writeln!(err, "  memory:            {}", plan.memory);
    let _ = writeln!(err, "  context:           {}", plan.context);
    let _ = writeln!(err, "  context-mode:      {}", plan.context_mode);
    let _ = writeln!(err, "  summarizer:        {}", plan.summarizer);
    let _ = writeln!(
        err,
        "  max-context-bytes: {}",
        plan.max_context_bytes
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "<unset>".to_string())
    );
    let _ = writeln!(err, "  fresh:             {}", plan.fresh);
    let _ = writeln!(err, "  no-record:         {}", plan.no_record);
    if let Some(pair) = &plan.ensured_pair {
        let _ = writeln!(err, "  pair-key:          {}", pair.pair_key);
        let _ = writeln!(err, "  pair-created-at:   {}", pair.created_at_unix);
        let _ = writeln!(err, "  pair-last-seen:    {}", pair.last_seen_unix);
    }
    let _ = writeln!(
        err,
        "  child program:     {}",
        plan.program.to_string_lossy()
    );
    for (index, arg) in plan.args.iter().enumerate() {
        let _ = writeln!(err, "  child arg[{index}]:      {}", arg.to_string_lossy());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    /// Most tests in this module are not about supervisor detection, so
    /// they run with one unambiguous native id already resolvable.
    /// Supervisor-detection precedence, ambiguity, and failure behavior are
    /// covered by the dedicated `supervisor_resolution` tests below and by
    /// `src/cli/supervisor.rs`'s own unit tests.
    fn default_detection_env() -> DetectionEnv {
        DetectionEnv {
            self_ref: None,
            codex_thread_id: Some(OsString::from("test-thread")),
            claude_session_id: None,
        }
    }

    fn run(args: &[OsString], env_id: Option<&str>) -> (ExitCode, String, String) {
        run_with_detection(args, env_id, &default_detection_env())
    }

    /// Every test in this module gets its own scratch working directory and
    /// its own scratch state root (both freshly created temporary
    /// directories, dropped -- and thus deleted -- at the end of the call),
    /// so a managed run's default conversation-memory pair-ensure step never
    /// touches the real user state root or interferes with another test.
    fn run_with_detection(
        args: &[OsString],
        env_id: Option<&str>,
        detection_env: &DetectionEnv,
    ) -> (ExitCode, String, String) {
        let cwd_dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let state_dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let state_root: PathBuf = state_dir.path().join("state");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(
            args,
            &mut out,
            &mut err,
            env_id,
            detection_env,
            cwd_dir.path(),
            Some(state_root.as_os_str()),
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn missing_double_dash_is_a_wrapper_error() {
        let args = os(&["--id", "reviewer"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("--"));
    }

    #[test]
    fn empty_child_after_double_dash_is_a_wrapper_error() {
        let args = os(&["--id", "reviewer", "--"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("no child command"));
    }

    #[test]
    fn invalid_id_is_rejected_before_any_other_error() {
        let args = os(&["--id", "not valid", "--", "echo", "hi"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("invalid subagent id"));
    }

    #[test]
    fn missing_id_without_memory_none_is_rejected() {
        let args = os(&["--", "echo", "hi"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("--id"));
    }

    #[test]
    fn missing_id_is_accepted_only_for_an_unrecorded_unidentified_plan() {
        let args = os(&[
            "--memory",
            "none",
            "--no-record",
            "--dry-run",
            "--",
            "echo",
            "hi",
        ]);
        let (code, _out, _err) = run(&args, None);
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn missing_id_with_recording_enabled_is_rejected() {
        let args = os(&["--memory", "none", "--dry-run", "--", "echo", "hi"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("--no-record"));
    }

    #[test]
    fn memory_workspace_fails_closed_as_unimplemented() {
        let args = os(&[
            "--id",
            "reviewer",
            "--memory",
            "workspace",
            "--dry-run",
            "--",
            "echo",
            "hi",
        ]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("--memory workspace"));
        assert!(err.contains("not implemented"));
    }

    #[test]
    fn memory_workspace_fails_closed_even_for_an_ordinary_run() {
        let args = os(&[
            "--id",
            "reviewer",
            "--memory",
            "workspace",
            "--",
            "echo",
            "hi",
        ]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("--memory workspace"));
    }

    #[test]
    fn conversation_memory_ensures_and_reports_a_pair_key_on_dry_run() {
        let args = os(&["--id", "reviewer", "--dry-run", "--", "echo", "hi"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.contains("pair-key:"));
    }

    #[test]
    fn memory_none_does_not_report_a_pair_key() {
        let args = os(&[
            "--id",
            "reviewer",
            "--memory",
            "none",
            "--dry-run",
            "--",
            "echo",
            "hi",
        ]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!err.contains("pair-key:"));
    }

    #[test]
    fn dry_run_and_ordinary_run_ensure_the_same_pair_key() {
        let cwd_dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let state_dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let state_root: PathBuf = state_dir.path().join("state");
        let detection_env = default_detection_env();

        let run_once = |args: &[OsString]| -> String {
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            execute_with_env(
                args,
                &mut out,
                &mut err,
                None,
                &detection_env,
                cwd_dir.path(),
                Some(state_root.as_os_str()),
            );
            String::from_utf8(err).unwrap()
        };

        let dry_run_err = run_once(&os(&["--id", "reviewer", "--dry-run", "--", "echo", "hi"]));
        let ordinary_err = run_once(&os(&["--id", "reviewer", "--", "echo", "hi"]));

        let extract_pair_key = |text: &str| -> String {
            text.lines()
                .find(|line| line.contains("pair-key:"))
                .expect("plan output should include a pair-key line")
                .to_string()
        };
        assert_eq!(
            extract_pair_key(&dry_run_err),
            extract_pair_key(&ordinary_err)
        );
    }

    #[test]
    fn invalid_supervisor_override_is_rejected() {
        let args = os(&[
            "--id",
            "reviewer",
            "--supervisor",
            "unknown:session",
            "--dry-run",
            "--",
            "echo",
        ]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("invalid --supervisor"));
        assert!(!err.contains("unknown:session"));
    }

    #[test]
    fn subagent_id_env_var_is_used_when_no_explicit_id_given() {
        let args = os(&["--dry-run", "--", "echo", "hi"]);
        let (code, _out, err) = run(&args, Some("from-env"));
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.contains("from-env"));
    }

    #[test]
    fn explicit_id_takes_precedence_over_env_var() {
        let args = os(&["--id", "explicit", "--dry-run", "--", "echo", "hi"]);
        let (code, _out, err) = run(&args, Some("from-env"));
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.contains("explicit"));
        assert!(!err.contains("from-env"));
    }

    #[test]
    fn dry_run_succeeds_without_backend_diagnostic() {
        let args = os(&[
            "--id",
            "reviewer",
            "--dry-run",
            "--",
            "claude",
            "-p",
            "hello",
        ]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!err.contains("backend not implemented"));
        assert!(err.contains("claude"));
    }

    #[test]
    fn ordinary_run_reports_backend_not_implemented_and_exits_125() {
        let args = os(&["--id", "reviewer", "--", "claude", "-p", "hello"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(err.contains("backend not implemented"));
    }

    #[test]
    fn quiet_suppresses_human_plan_output() {
        let args = os(&["--id", "reviewer", "--dry-run", "--quiet", "--", "claude"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.is_empty());
    }

    #[test]
    fn wrapper_flags_after_double_dash_are_preserved_as_child_arguments() {
        let args = os(&[
            "--id",
            "reviewer",
            "--dry-run",
            "--",
            "echo",
            "--id",
            "sneaky",
            "--dry-run",
            "--",
        ]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.contains("child program:     echo"));
        assert!(err.contains("child arg[0]:      --id"));
        assert!(err.contains("child arg[1]:      sneaky"));
        assert!(err.contains("child arg[2]:      --dry-run"));
        assert!(err.contains("child arg[3]:      --"));
    }

    #[test]
    fn unknown_wrapper_flag_is_a_clap_error_not_a_panic() {
        let args = os(&["--id", "reviewer", "--not-a-real-flag", "--", "echo"]);
        let (code, _out, err) = run(&args, None);
        assert_eq!(code, wrapper_error_exit());
        assert!(!err.is_empty());
    }

    mod supervisor_resolution {
        use super::*;

        #[test]
        fn explicit_supervisor_wins_over_native_env() {
            let detection_env = DetectionEnv {
                self_ref: None,
                codex_thread_id: Some(OsString::from("thread-1")),
                claude_session_id: None,
            };
            let args = os(&[
                "--id",
                "reviewer",
                "--supervisor",
                "claude:override-session",
                "--dry-run",
                "--",
                "echo",
            ]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, ExitCode::SUCCESS);
            assert!(err.contains("claude:override-session (via explicit)"));
        }

        #[test]
        fn native_codex_thread_id_is_detected_when_unambiguous() {
            let detection_env = DetectionEnv {
                self_ref: None,
                codex_thread_id: Some(OsString::from("thread-42")),
                claude_session_id: None,
            };
            let args = os(&["--id", "reviewer", "--dry-run", "--", "echo"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, ExitCode::SUCCESS);
            assert!(err.contains("codex:thread-42 (via native-env)"));
        }

        #[test]
        fn native_claude_session_id_is_detected_when_unambiguous() {
            let detection_env = DetectionEnv {
                self_ref: None,
                codex_thread_id: None,
                claude_session_id: Some(OsString::from("session-42")),
            };
            let args = os(&["--id", "reviewer", "--dry-run", "--", "echo"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, ExitCode::SUCCESS);
            assert!(err.contains("claude:session-42 (via native-env)"));
        }

        #[test]
        fn both_native_ids_present_is_rejected_as_ambiguous() {
            let detection_env = DetectionEnv {
                self_ref: None,
                codex_thread_id: Some(OsString::from("thread-1")),
                claude_session_id: Some(OsString::from("session-1")),
            };
            let args = os(&["--id", "reviewer", "--dry-run", "--", "echo"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, wrapper_error_exit());
            assert!(err.contains("ambiguous") || err.contains("cannot be inferred safely"));
        }

        #[test]
        fn no_identity_present_is_rejected_with_actionable_diagnostic() {
            let detection_env = DetectionEnv::default();
            let args = os(&["--id", "reviewer", "--dry-run", "--", "echo"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, wrapper_error_exit());
            assert!(err.contains("no supervisor identity found"));
            assert!(err.contains("--supervisor"));
        }

        #[test]
        fn present_but_empty_native_id_is_rejected_not_silently_accepted() {
            let detection_env = DetectionEnv {
                self_ref: None,
                codex_thread_id: Some(OsString::new()),
                claude_session_id: None,
            };
            let args = os(&["--id", "reviewer", "--dry-run", "--", "echo"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, wrapper_error_exit());
            assert!(err.contains("CODEX_THREAD_ID"));
            assert!(err.contains("set but empty"));
        }

        #[test]
        fn managed_ref_fails_closed_instead_of_falling_through_to_native_env() {
            let detection_env = DetectionEnv {
                self_ref: Some(OsString::from("/tmp/manifest.json")),
                codex_thread_id: Some(OsString::from("thread-1")),
                claude_session_id: None,
            };
            let args = os(&["--id", "reviewer", "--dry-run", "--", "echo"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, wrapper_error_exit());
            assert!(err.contains("SUBAGENT_SELF_REF"));
            assert!(err.contains("not implemented"));
        }

        #[test]
        fn ordinary_run_still_exits_125_without_spawning_even_with_a_resolved_supervisor() {
            let detection_env = DetectionEnv {
                self_ref: None,
                codex_thread_id: Some(OsString::from("thread-1")),
                claude_session_id: None,
            };
            let args = os(&["--id", "reviewer", "--", "echo", "hi"]);
            let (code, _out, err) = run_with_detection(&args, None, &detection_env);
            assert_eq!(code, wrapper_error_exit());
            assert!(err.contains("backend not implemented"));
            assert!(!err.contains("supervisor detection"));
        }
    }
}
