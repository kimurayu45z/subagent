//! `subagent log --pair PAIR [-n COUNT] [--format text|json]`: reports the
//! pair exchange ledger as unavailable. A stateful placeholder; it never
//! touches the filesystem or any other persistent state.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::report::{Report, ReportStatus};
use super::{OutputFormat, handle_clap_error, wrapper_error_exit};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-log", no_binary_name = true)]
struct LogArgs {
    #[arg(long)]
    pair: String,

    #[arg(short = 'n', long = "count")]
    count: Option<u32>,

    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize)]
struct UnavailableBody {
    component: String,
    reason: String,
    requested_pair: String,
    requested_count: Option<u32>,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let log_args: LogArgs = match LogArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };

    let body = UnavailableBody {
        component: "pair-ledger".to_string(),
        reason: "the SQLite pair exchange ledger (design.md section 10) is not implemented in this build"
            .to_string(),
        requested_pair: log_args.pair,
        requested_count: log_args.count,
    };

    match log_args.format {
        OutputFormat::Json => {
            let report = Report::new("log", ReportStatus::Unavailable, body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            let _ = writeln!(err, "subagent: log is unavailable: {}", body.reason);
        }
    }
    wrapper_error_exit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_pair_flag_is_a_clap_error() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&[], &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(!err.is_empty());
    }

    #[test]
    fn json_format_reports_unavailable_status() {
        let args: Vec<OsString> = vec![
            OsString::from("--pair"),
            OsString::from("p1"),
            OsString::from("-n"),
            OsString::from("5"),
            OsString::from("--format"),
            OsString::from("json"),
        ];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["status"], "unavailable");
        assert_eq!(value["body"]["requested_pair"], "p1");
        assert_eq!(value["body"]["requested_count"], 5);
    }
}
