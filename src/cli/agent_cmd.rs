//! `subagent agent add|remove|list`: reports agent-profile storage as
//! unavailable. A stateful placeholder; it never writes any configuration.
//!
//! These sub-subcommands are simple enough (one optional ID, plus for
//! `add` an explicit `--`-bounded child command) that they are parsed by
//! hand rather than through `clap`, which keeps the mandatory `--` boundary
//! handling for `agent add` identical to the top-level run form.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use super::id::SubagentId;
use super::{is_help_flag, split_on_double_dash, wrapper_error_exit};

const ADD_USAGE: &str = "usage: subagent agent add ID -- COMMAND [ARG...]";
const REMOVE_USAGE: &str = "usage: subagent agent remove ID";
const LIST_USAGE: &str = "usage: subagent agent list";

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let Some(first) = args.first() else {
        let _ = writeln!(
            err,
            "subagent: agent requires a subcommand: add, remove, or list"
        );
        return wrapper_error_exit();
    };

    if is_help_flag(first.as_os_str()) {
        let _ = writeln!(out, "{ADD_USAGE}");
        let _ = writeln!(out, "{REMOVE_USAGE}");
        let _ = writeln!(out, "{LIST_USAGE}");
        return ExitCode::SUCCESS;
    }

    match first.to_str() {
        Some("add") => add(&args[1..], out, err),
        Some("remove") => remove(&args[1..], out, err),
        Some("list") => list(&args[1..], out, err),
        _ => {
            let _ = writeln!(
                err,
                "subagent: unknown agent subcommand {first:?}; expected add, remove, or list"
            );
            wrapper_error_exit()
        }
    }
}

fn add(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    if args
        .first()
        .is_some_and(|token| is_help_flag(token.as_os_str()))
    {
        let _ = writeln!(out, "{ADD_USAGE}");
        return ExitCode::SUCCESS;
    }

    let split = split_on_double_dash(args);
    if split.before.is_empty() {
        let _ = writeln!(err, "subagent: agent add requires an ID");
        let _ = writeln!(err, "{ADD_USAGE}");
        return wrapper_error_exit();
    }
    if split.before.len() > 1 {
        let _ = writeln!(
            err,
            "subagent: agent add takes exactly one ID before `--`, got {} extra token(s)",
            split.before.len() - 1
        );
        let _ = writeln!(err, "{ADD_USAGE}");
        return wrapper_error_exit();
    }

    let raw_id = match split.before[0].to_str() {
        Some(text) => text,
        None => {
            let _ = writeln!(err, "subagent: agent id must be valid UTF-8");
            return wrapper_error_exit();
        }
    };
    let id: SubagentId = match SubagentId::parse(raw_id) {
        Ok(id) => id,
        Err(invalid) => {
            let _ = writeln!(err, "subagent: {invalid}");
            return wrapper_error_exit();
        }
    };

    let command_tokens: &[OsString] = match split.after {
        Some(tokens) if !tokens.is_empty() => tokens,
        Some(_) => {
            let _ = writeln!(err, "subagent: agent add requires a command after `--`");
            let _ = writeln!(err, "{ADD_USAGE}");
            return wrapper_error_exit();
        }
        None => {
            let _ = writeln!(
                err,
                "subagent: agent add requires an explicit `--` boundary before the command"
            );
            let _ = writeln!(err, "{ADD_USAGE}");
            return wrapper_error_exit();
        }
    };

    let formatted_command: String = command_tokens
        .iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect::<Vec<String>>()
        .join(" ");
    let _ = writeln!(
        err,
        "subagent: agent add is unavailable: agent profile storage (design.md section 16) is not implemented in this build (id={id}, command=`{formatted_command}`)"
    );
    wrapper_error_exit()
}

fn remove(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    if args
        .first()
        .is_some_and(|token| is_help_flag(token.as_os_str()))
    {
        let _ = writeln!(out, "{REMOVE_USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.len() != 1 {
        let _ = writeln!(err, "subagent: agent remove requires exactly one ID");
        let _ = writeln!(err, "{REMOVE_USAGE}");
        return wrapper_error_exit();
    }
    let raw_id = match args[0].to_str() {
        Some(text) => text,
        None => {
            let _ = writeln!(err, "subagent: agent id must be valid UTF-8");
            return wrapper_error_exit();
        }
    };
    let id: SubagentId = match SubagentId::parse(raw_id) {
        Ok(id) => id,
        Err(invalid) => {
            let _ = writeln!(err, "subagent: {invalid}");
            return wrapper_error_exit();
        }
    };
    let _ = writeln!(
        err,
        "subagent: agent remove is unavailable: agent profile storage (design.md section 16) is not implemented in this build (id={id})"
    );
    wrapper_error_exit()
}

fn list(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    if args
        .first()
        .is_some_and(|token| is_help_flag(token.as_os_str()))
    {
        let _ = writeln!(out, "{LIST_USAGE}");
        return ExitCode::SUCCESS;
    }
    if !args.is_empty() {
        let _ = writeln!(err, "subagent: agent list takes no arguments");
        let _ = writeln!(err, "{LIST_USAGE}");
        return wrapper_error_exit();
    }
    let _ = writeln!(
        err,
        "subagent: agent list is unavailable: agent profile storage (design.md section 16) is not implemented in this build"
    );
    wrapper_error_exit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn unknown_agent_subcommand_is_a_wrapper_error() {
        let args = os(&["bogus"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
    }

    #[test]
    fn agent_help_lists_subcommands_and_succeeds() {
        let args = os(&["--help"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(err.is_empty());
        let text: String = String::from_utf8(out).unwrap();
        assert!(text.contains("agent add"));
        assert!(text.contains("agent remove"));
        assert!(text.contains("agent list"));
    }

    #[test]
    fn missing_agent_subcommand_is_a_wrapper_error() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&[], &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
    }

    #[test]
    fn add_requires_double_dash_boundary() {
        let args = os(&["add", "reviewer"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(String::from_utf8(err).unwrap().contains("--"));
    }

    #[test]
    fn add_rejects_invalid_id() {
        let args = os(&["add", "bad id", "--", "claude", "-p"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("invalid subagent id")
        );
    }

    #[test]
    fn add_preserves_command_and_reports_unavailable() {
        let args = os(&["add", "reviewer", "--", "claude", "-p", "--model", "opus"]);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("id=reviewer"));
        assert!(text.contains("claude -p --model opus"));
        assert!(text.contains("unavailable"));
    }

    #[test]
    fn remove_requires_exactly_one_id() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&os(&["remove"]), &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
    }

    #[test]
    fn remove_reports_unavailable_for_valid_id() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&os(&["remove", "reviewer"]), &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(String::from_utf8(err).unwrap().contains("unavailable"));
    }

    #[test]
    fn list_reports_unavailable_and_takes_no_arguments() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&os(&["list"]), &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(String::from_utf8(err).unwrap().contains("unavailable"));
    }

    #[test]
    fn list_rejects_unexpected_arguments() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&os(&["list", "extra"]), &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
    }
}
