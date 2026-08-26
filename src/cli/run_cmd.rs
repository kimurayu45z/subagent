//! The canonical `subagent --id ID [RUN-OPTIONS] -- COMMAND [ARG...]` form.
//!
//! This module resolves and validates the run plan described in
//! `docs/design.md` sections 6.2 and 7, but does not implement the
//! supervisor/pair/context/child backend. `--dry-run` prints the resolved
//! plan and exits successfully without spawning anything. An ordinary
//! (non-dry-run) managed invocation never spawns the child either; it exits
//! `125` with an explicit "backend not implemented" diagnostic, per this
//! milestone's requirement to be honest about what is and is not built.

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::id::SubagentId;
use super::report::{OsStringJson, Report, ReportStatus, write_json_atomic};
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

/// The fully resolved plan for a managed run: wrapper defaults applied,
/// `--id` validated, and the child command preserved as `OsString`.
#[derive(Debug, Clone)]
struct RunPlan {
    id: Option<SubagentId>,
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
}

#[derive(Debug, Clone, Serialize)]
struct RunPlanReport {
    id: Option<String>,
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
}

impl From<&RunPlan> for RunPlanReport {
    fn from(plan: &RunPlan) -> Self {
        RunPlanReport {
            id: plan.id.as_ref().map(|id| id.as_str().to_string()),
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
        }
    }
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let subagent_id_env: Option<String> = std::env::var(SUBAGENT_ID_ENV).ok();
    execute_with_env(args, out, err, subagent_id_env.as_deref())
}

/// Same as [`execute`], but with the `SUBAGENT_ID` environment lookup
/// injected explicitly so tests never need to mutate real process
/// environment state (which is unsafe to do from parallel test threads).
fn execute_with_env(
    args: &[OsString],
    out: &mut dyn Write,
    err: &mut dyn Write,
    subagent_id_env: Option<&str>,
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

    let supervisor_override: Option<String> =
        match validate_supervisor_override(run_args.supervisor.as_deref(), err) {
            Ok(supervisor) => supervisor,
            Err(code) => return code,
        };

    let plan = RunPlan {
        id,
        supervisor_override,
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
            "subagent: backend not implemented: supervisor detection, the pair ledger, the context capsule, and child process spawning are not implemented in this build"
        );
        let _ = writeln!(
            err,
            "subagent: no child process was started; re-run with --dry-run to inspect the resolved plan without this error"
        );
        wrapper_error_exit()
    }
}

fn validate_supervisor_override(
    raw: Option<&str>,
    err: &mut dyn Write,
) -> Result<Option<String>, ExitCode> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let Some((provider, session_id)) = value.split_once(':') else {
        let _ = writeln!(
            err,
            "subagent: invalid --supervisor {value:?}: expected codex:SESSION_ID or claude:SESSION_ID"
        );
        return Err(wrapper_error_exit());
    };
    if !matches!(provider, "codex" | "claude") || session_id.is_empty() {
        let _ = writeln!(
            err,
            "subagent: invalid --supervisor {value:?}: expected codex:SESSION_ID or claude:SESSION_ID"
        );
        return Err(wrapper_error_exit());
    }
    Ok(Some(value.to_string()))
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
    let _ = writeln!(
        err,
        "  supervisor:        {}",
        plan.supervisor_override
            .as_deref()
            .unwrap_or("<auto-detect: not implemented>")
    );
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

    fn run(args: &[OsString], env_id: Option<&str>) -> (ExitCode, String, String) {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(args, &mut out, &mut err, env_id);
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
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_with_env(&args, &mut out, &mut err, None);
        assert_eq!(code, ExitCode::SUCCESS);
        let err = String::from_utf8(err).unwrap();
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
}
