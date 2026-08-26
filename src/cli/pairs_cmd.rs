//! `subagent pairs [--format text|json]`: reports the pair ledger as
//! unavailable. A stateful placeholder; it never touches the filesystem or
//! any other persistent state.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::report::{Report, ReportStatus};
use super::{OutputFormat, handle_clap_error, wrapper_error_exit};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-pairs", no_binary_name = true)]
struct PairsArgs {
    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize)]
struct UnavailableBody {
    component: String,
    reason: String,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let pairs_args: PairsArgs = match PairsArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };

    let body = UnavailableBody {
        component: "pair-ledger".to_string(),
        reason: "the SQLite pair ledger (design.md section 10) is not implemented in this build"
            .to_string(),
    };

    match pairs_args.format {
        OutputFormat::Json => {
            let report = Report::new("pairs", ReportStatus::Unavailable, body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            let _ = writeln!(err, "subagent: pairs is unavailable: {}", body.reason);
        }
    }
    wrapper_error_exit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_text_format_reports_unavailable() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&[], &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(out.is_empty());
        assert!(String::from_utf8(err).unwrap().contains("unavailable"));
    }

    #[test]
    fn json_format_reports_unavailable_status() {
        let args: Vec<OsString> = vec![OsString::from("--format"), OsString::from("json")];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["status"], "unavailable");
        assert_eq!(value["kind"], "pairs");
    }
}
