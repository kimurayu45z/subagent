//! `subagent context`: reports pair exchange context as unavailable.
//!
//! This is a stateful placeholder. It never touches the filesystem or any
//! other persistent state; it only reports why it cannot fulfil the
//! request in this build.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::report::{Report, ReportStatus};
use super::{OutputFormat, handle_clap_error, wrapper_error_exit};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-context", no_binary_name = true)]
struct ContextArgs {
    #[arg(long)]
    pair: Option<String>,

    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize)]
struct UnavailableBody {
    component: String,
    reason: String,
    requested_pair: Option<String>,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let context_args: ContextArgs = match ContextArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };

    let body = UnavailableBody {
        component: "pair-ledger".to_string(),
        reason: "the SQLite pair exchange ledger and context capsule (design.md sections 10-11) are not implemented in this build".to_string(),
        requested_pair: context_args.pair,
    };

    match context_args.format {
        OutputFormat::Json => {
            let report = Report::new("context", ReportStatus::Unavailable, body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            let _ = writeln!(err, "subagent: context is unavailable: {}", body.reason);
        }
    }
    wrapper_error_exit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_format_reports_unavailable_on_stderr_and_exits_nonzero() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&[], &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(out.is_empty());
        assert!(String::from_utf8(err).unwrap().contains("unavailable"));
    }

    #[test]
    fn json_format_reports_unavailable_status_on_stdout() {
        let args: Vec<OsString> = vec![
            OsString::from("--format"),
            OsString::from("json"),
            OsString::from("--pair"),
            OsString::from("p1"),
        ];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["status"], "unavailable");
        assert_eq!(value["body"]["requested_pair"], "p1");
    }
}
