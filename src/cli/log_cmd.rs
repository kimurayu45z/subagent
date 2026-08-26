//! Inspect completed request/response messages for one pair.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::pair_key::PairKey;
use super::report::{Report, ReportStatus};
use super::state_dir;
use super::store::{CompletedExchange, OpenForRead, Store};
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

#[derive(Debug, Serialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
enum BodyValue {
    Utf8(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Serialize)]
struct ExchangeReport {
    invocation_id: String,
    sequence: i64,
    direction: String,
    body: BodyValue,
    truncated: bool,
    redaction_count: u32,
    redaction_classes: Vec<String>,
    created_at_unix: i64,
}

impl From<CompletedExchange> for ExchangeReport {
    fn from(exchange: CompletedExchange) -> ExchangeReport {
        let body: BodyValue = match String::from_utf8(exchange.body) {
            Ok(text) => BodyValue::Utf8(text),
            Err(error) => BodyValue::Bytes(error.into_bytes()),
        };
        ExchangeReport {
            invocation_id: exchange.invocation_id,
            sequence: exchange.sequence,
            direction: exchange.direction.to_string(),
            body,
            truncated: exchange.truncated,
            redaction_count: exchange.redaction_count,
            redaction_classes: exchange.redaction_classes,
            created_at_unix: exchange.created_at_unix,
        }
    }
}

#[derive(Debug, Serialize)]
struct LogBody {
    pair: String,
    exchanges: Vec<ExchangeReport>,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let log_args: LogArgs = match LogArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };
    let pair_key: PairKey = match PairKey::from_hex(&log_args.pair) {
        Ok(key) => key,
        Err(message) => {
            let _ = writeln!(err, "subagent: invalid --pair: {message}");
            return wrapper_error_exit();
        }
    };
    let state_root: PathBuf = match state_dir::resolve_state_root(
        std::env::var_os(state_dir::SUBAGENT_STATE_DIR_ENV).as_deref(),
    ) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(err, "subagent: {error}");
            return wrapper_error_exit();
        }
    };
    let mut exchanges: Vec<CompletedExchange> = match Store::open_for_read(&state_root) {
        Ok(OpenForRead::Absent) => Vec::new(),
        Ok(OpenForRead::Ready(store)) => match store.list_completed_exchanges(&pair_key, None) {
            Ok(values) => values,
            Err(error) => {
                let _ = writeln!(err, "subagent: failed to read pair log: {error}");
                return wrapper_error_exit();
            }
        },
        Err(error) => {
            let _ = writeln!(err, "subagent: failed to open invocation ledger: {error}");
            return wrapper_error_exit();
        }
    };
    if let Some(count) = log_args.count {
        let keep: usize = usize::try_from(count).unwrap_or(usize::MAX);
        if exchanges.len() > keep {
            exchanges.drain(..exchanges.len() - keep);
        }
    }
    let body = LogBody {
        pair: pair_key.to_hex(),
        exchanges: exchanges.into_iter().map(ExchangeReport::from).collect(),
    };
    match log_args.format {
        OutputFormat::Json => {
            let report = Report::new("log", ReportStatus::Ok, &body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            for exchange in &body.exchanges {
                let text: String = match &exchange.body {
                    BodyValue::Utf8(text) => text.clone(),
                    BodyValue::Bytes(bytes) => format!("<{} non-UTF-8 bytes>", bytes.len()),
                };
                let _ = writeln!(
                    out,
                    "#{} {} {}{}\n{}",
                    exchange.sequence,
                    exchange.direction,
                    exchange.invocation_id,
                    if exchange.truncated {
                        " [truncated]"
                    } else {
                        ""
                    },
                    text
                );
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_pair_flag_is_a_clap_error() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        assert_eq!(execute(&[], &mut out, &mut err), wrapper_error_exit());
        assert!(!err.is_empty());
    }

    #[test]
    fn malformed_pair_is_rejected_before_state_access() {
        let args: Vec<OsString> = vec![OsString::from("--pair"), OsString::from("p1")];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        assert_eq!(execute(&args, &mut out, &mut err), wrapper_error_exit());
        assert!(String::from_utf8(err).unwrap().contains("invalid --pair"));
    }
}
