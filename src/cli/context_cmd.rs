//! Locate persisted context capsules, optionally scoped to one pair.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::pair_key::PairKey;
use super::report::{OsStringJson, Report, ReportStatus};
use super::state_dir;
use super::store::{InvocationRecord, OpenForRead, Store};
use super::{OutputFormat, handle_clap_error, wrapper_error_exit};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-context", no_binary_name = true)]
struct ContextArgs {
    #[arg(long)]
    pair: Option<String>,
    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct ContextInvocation {
    invocation_id: String,
    sequence: i64,
    status: String,
    capsule_path: Option<OsStringJson>,
    started_at_unix: i64,
    completed_at_unix: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ContextBody {
    state_root: OsStringJson,
    context_root: OsStringJson,
    pair: Option<String>,
    invocations: Vec<ContextInvocation>,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let context_args: ContextArgs = match ContextArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };
    let pair_key: Option<PairKey> = match context_args.pair.as_deref() {
        Some(raw) => match PairKey::from_hex(raw) {
            Ok(key) => Some(key),
            Err(message) => {
                let _ = writeln!(err, "subagent: invalid --pair: {message}");
                return wrapper_error_exit();
            }
        },
        None => None,
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
    let records: Vec<InvocationRecord> = match (pair_key, Store::open_for_read(&state_root)) {
        (_, Ok(OpenForRead::Absent)) | (None, Ok(OpenForRead::Ready(_))) => Vec::new(),
        (Some(key), Ok(OpenForRead::Ready(store))) => match store.list_invocations(&key, None) {
            Ok(records) => records,
            Err(error) => {
                let _ = writeln!(err, "subagent: failed to read context metadata: {error}");
                return wrapper_error_exit();
            }
        },
        (_, Err(error)) => {
            let _ = writeln!(err, "subagent: failed to open invocation ledger: {error}");
            return wrapper_error_exit();
        }
    };
    let body = ContextBody {
        state_root: OsStringJson::from_os_str(state_root.as_os_str()),
        context_root: OsStringJson::from_os_str(state_root.join("context").as_os_str()),
        pair: pair_key.map(PairKey::to_hex),
        invocations: records
            .into_iter()
            .map(|record| ContextInvocation {
                invocation_id: record.invocation_id,
                sequence: record.sequence,
                status: record.status.to_string(),
                capsule_path: record
                    .capsule_path
                    .as_deref()
                    .map(|path| OsStringJson::from_os_str(path.as_os_str())),
                started_at_unix: record.started_at_unix,
                completed_at_unix: record.completed_at_unix,
            })
            .collect(),
    };
    match context_args.format {
        OutputFormat::Json => {
            let report = Report::new("context", ReportStatus::Ok, &body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            let _ = writeln!(
                out,
                "context root: {}",
                state_root.join("context").display()
            );
            if pair_key.is_none() {
                let _ = writeln!(
                    out,
                    "use `subagent pairs` and then `subagent context --pair PAIR` to list capsules"
                );
            }
            for invocation in &body.invocations {
                let capsule: String = match &invocation.capsule_path {
                    Some(OsStringJson::Utf8(path)) => path.clone(),
                    Some(OsStringJson::Bytes(bytes)) => {
                        format!("<{} non-UTF-8 path bytes>", bytes.len())
                    }
                    None => "<none>".to_string(),
                };
                let _ = writeln!(
                    out,
                    "#{} {} {} {}",
                    invocation.sequence, invocation.status, invocation.invocation_id, capsule
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
    fn malformed_pair_is_rejected() {
        let args: Vec<OsString> = vec![OsString::from("--pair"), OsString::from("p1")];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        assert_eq!(execute(&args, &mut out, &mut err), wrapper_error_exit());
    }
}
