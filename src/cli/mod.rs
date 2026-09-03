//! Top-level CLI dispatch for `subagent`.
//!
//! This module implements the CLI shell described in `docs/design.md`
//! section 6 ("CLI contract"), supervisor identity resolution, and the
//! workspace-scoped pair store, durable exchange ledger, context capsule,
//! and managed child-process backend for Claude print mode and Codex exec.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::process::ExitCode;

mod agent_cmd;
mod antigravity_json;
mod capsule;
mod child;
mod codex_json;
mod context_cmd;
mod doctor_cmd;
mod forget_cmd;
mod history;
mod id;
mod log_cmd;
mod managed_run;
mod opencode_json;
mod pair_key;
mod pairs_cmd;
mod process;
mod redaction;
mod report;
mod run_cmd;
mod state_dir;
mod store;
mod summarizer;
mod supervisor;
mod workspace;
mod workstream;

/// Wrapper-level exit code used for every failure that happens before, or in
/// place of, spawning a managed child process. See `docs/design.md` section
/// 14 ("Before spawn, wrapper errors use exit status `125`").
pub(crate) const WRAPPER_ERROR_EXIT: u8 = 125;

pub(crate) fn wrapper_error_exit() -> ExitCode {
    ExitCode::from(WRAPPER_ERROR_EXIT)
}

/// Output format shared by the commands that the design explicitly calls
/// out as offering `--format text|json` (`context`, `log`, `pairs`,
/// `doctor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}

const HELP_TEXT: &str = "\
subagent - preserve delegation context across Codex, Claude Code, OpenCode, and Antigravity sub-agent invocations

USAGE:
    subagent --id ID [RUN-OPTIONS] -- COMMAND [ARG...]
    subagent context [--pair PAIR] [--format text|json]
    subagent log --pair PAIR [-n COUNT] [--format text|json]
    subagent pairs [--format text|json]
    subagent doctor [--format text|json]
    subagent forget --pair PAIR
    subagent agent add ID -- COMMAND [ARG...]
    subagent agent remove ID
    subagent agent list

RUN OPTIONS:
    --id ID                        Logical subordinate id
    --supervisor PROVIDER:SESSION  Explicit supervisor override
    --inherit-from ID              Inherit older history from another id in
                                    this supervisor conversation
    --memory conversation|workspace|none
    --context pair|supervisor|all|none
    --context-mode required|best-effort
    --context-delivery pointer|inline
    --summarizer deterministic|haiku|luna|none
    --summarize-above-bytes BYTES  Model summary threshold (default 16384)
    --max-context-bytes BYTES
    --workstream ID                 Explicit native-session task chain
    --fresh                         Start/restart the named workstream
    --resume                        Resume the named workstream exactly
    --no-record
    --dry-run
    --quiet
    --report PATH                  Write a JSON report describing the plan
                                    (or the reason a real run did not start)
                                    to PATH.

GLOBAL OPTIONS:
    -h, --help                     Print help
    -V, --version                  Print version

Everything after the first literal `--` belongs to the caller's child command
and is never interpreted as a wrapper option. Managed workstreams may adapt
provider-native continuity argv after recording the caller command.
";

/// Parses and dispatches the full argument vector (excluding argv[0]).
///
/// `out` and `err` stand in for the process stdout/stderr streams so that
/// dispatch logic can be exercised in unit tests without spawning the real
/// binary.
pub fn dispatch(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let Some(first) = args.first() else {
        let _ = writeln!(err, "subagent: no command given");
        let _ = write!(err, "{HELP_TEXT}");
        return wrapper_error_exit();
    };
    let first: &OsStr = first.as_os_str();

    if first == "--help" || first == "-h" {
        let _ = write!(out, "{HELP_TEXT}");
        return ExitCode::SUCCESS;
    }
    if first == "--version" || first == "-V" {
        let _ = writeln!(out, "subagent {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    match first.to_str() {
        Some("context") => context_cmd::execute(&args[1..], out, err),
        Some("log") => log_cmd::execute(&args[1..], out, err),
        Some("pairs") => pairs_cmd::execute(&args[1..], out, err),
        Some("doctor") => doctor_cmd::execute(&args[1..], out, err),
        Some("forget") => forget_cmd::execute(&args[1..], out, err),
        Some("agent") => agent_cmd::execute(&args[1..], out, err),
        _ => run_cmd::execute(args, out, err),
    }
}

/// Splits `tokens` on the first literal `--` argument.
///
/// `before` holds everything preceding that token; `after` holds everything
/// following it, or `None` if no literal `--` token is present. Only the
/// first `--` is treated as the boundary, so a second `--` inside the child
/// command is preserved verbatim as a child argument.
pub(crate) struct ArgSplit<'a> {
    pub before: &'a [OsString],
    pub after: Option<&'a [OsString]>,
}

pub(crate) fn split_on_double_dash(tokens: &[OsString]) -> ArgSplit<'_> {
    let boundary = OsStr::new("--");
    match tokens.iter().position(|t| t.as_os_str() == boundary) {
        Some(idx) => ArgSplit {
            before: &tokens[..idx],
            after: Some(&tokens[idx + 1..]),
        },
        None => ArgSplit {
            before: tokens,
            after: None,
        },
    }
}

/// Renders a `clap` parse error using the injected output streams and maps
/// it to the process exit code it should produce.
///
/// `clap`'s own `Error::exit` writes directly to the real process stdio,
/// which would bypass the `out`/`err` indirection used throughout this
/// crate for testability, so the rendering and stream selection is done
/// here instead.
pub(crate) fn handle_clap_error(
    error: clap::Error,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    use clap::error::ErrorKind;
    let message: String = error.render().to_string();
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let _ = write!(out, "{message}");
            ExitCode::SUCCESS
        }
        ErrorKind::DisplayVersion => {
            let _ = write!(out, "{message}");
            ExitCode::SUCCESS
        }
        _ => {
            let _ = write!(err, "{message}");
            wrapper_error_exit()
        }
    }
}

/// Returns `true` when `token` is a bare help flag, used by the hand-rolled
/// (non-`clap`) parsers for the smaller `agent` subcommands.
pub(crate) fn is_help_flag(token: &OsStr) -> bool {
    token == "-h" || token == "--help"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn split_on_double_dash_finds_first_boundary_only() {
        let tokens = os(&["--id", "foo", "--", "echo", "--", "bar"]);
        let split = split_on_double_dash(&tokens);
        assert_eq!(split.before, os(&["--id", "foo"]).as_slice());
        assert_eq!(split.after, Some(os(&["echo", "--", "bar"]).as_slice()));
    }

    #[test]
    fn split_on_double_dash_reports_missing_boundary() {
        let tokens = os(&["--id", "foo"]);
        let split = split_on_double_dash(&tokens);
        assert_eq!(split.before, tokens.as_slice());
        assert!(split.after.is_none());
    }

    #[test]
    fn split_on_double_dash_handles_empty_input() {
        let tokens: Vec<OsString> = Vec::new();
        let split = split_on_double_dash(&tokens);
        assert!(split.before.is_empty());
        assert!(split.after.is_none());
    }

    #[test]
    fn dispatch_with_no_arguments_is_a_wrapper_error() {
        let args: Vec<OsString> = Vec::new();
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = dispatch(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(!err.is_empty());
    }

    #[test]
    fn dispatch_help_goes_to_stdout_and_succeeds() {
        let args = os(&["--help"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = dispatch(&args, &mut out, &mut err);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.is_empty());
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("subagent --id ID"));
    }

    #[test]
    fn dispatch_version_goes_to_stdout_and_succeeds() {
        let args = os(&["--version"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = dispatch(&args, &mut out, &mut err);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.is_empty());
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("subagent "));
    }

    #[test]
    fn dispatch_routes_known_subcommands_by_leading_token() {
        for name in ["context", "log", "pairs", "doctor", "forget", "agent"] {
            let args = os(&[name, "--help"]);
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let code = dispatch(&args, &mut out, &mut err);
            // Help proves routing without consulting the user's real state
            // directory or depending on its current schema version.
            assert_eq!(code, ExitCode::SUCCESS, "subcommand {name}");
            assert!(!out.is_empty(), "subcommand {name}");
            assert!(err.is_empty(), "subcommand {name}");
        }
    }
}
