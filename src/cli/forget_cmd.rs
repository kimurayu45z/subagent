//! Delete one pair, its ledger records, and its owned context capsules.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use super::capsule;
use super::pair_key::PairKey;
use super::state_dir;
use super::store::{OpenForRead, Store};
use super::{handle_clap_error, wrapper_error_exit};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-forget", no_binary_name = true)]
struct ForgetArgs {
    #[arg(long)]
    pair: String,
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let forget_args: ForgetArgs = match ForgetArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };
    let pair_key: PairKey = match PairKey::from_hex(&forget_args.pair) {
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
    let records = match Store::open_for_read(&state_root) {
        Ok(OpenForRead::Absent) => {
            let _ = writeln!(err, "subagent: pair was not found");
            return wrapper_error_exit();
        }
        Ok(OpenForRead::Ready(store)) => match store.list_invocations(&pair_key, None) {
            Ok(records) => records,
            Err(error) => {
                let _ = writeln!(
                    err,
                    "subagent: failed to inspect pair before deletion: {error}"
                );
                return wrapper_error_exit();
            }
        },
        Err(error) => {
            let _ = writeln!(err, "subagent: failed to open invocation ledger: {error}");
            return wrapper_error_exit();
        }
    };
    for record in &records {
        if record.capsule_path.is_some() {
            let expected: PathBuf = state_root.join("context").join(&record.invocation_id);
            match std::fs::symlink_metadata(&expected) {
                Ok(_) => {
                    if let Err(error) = capsule::remove_capsule(&state_root, &record.invocation_id)
                    {
                        let _ = writeln!(
                            err,
                            "subagent: capsule cleanup failed before ledger deletion: {error}"
                        );
                        return wrapper_error_exit();
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    let _ = writeln!(
                        err,
                        "subagent: could not inspect capsule before deletion: {error}"
                    );
                    return wrapper_error_exit();
                }
            }
        }
    }
    let mut store: Store = match Store::open_for_write(&state_root) {
        Ok(store) => store,
        Err(error) => {
            let _ = writeln!(
                err,
                "subagent: failed to open invocation ledger for deletion: {error}"
            );
            return wrapper_error_exit();
        }
    };
    match store.delete_pair(&pair_key) {
        Ok(true) => {
            let _ = writeln!(out, "forgot pair {}", pair_key);
            ExitCode::SUCCESS
        }
        Ok(false) => {
            let _ = writeln!(err, "subagent: pair was not found");
            wrapper_error_exit()
        }
        Err(error) => {
            let _ = writeln!(err, "subagent: failed to delete pair: {error}");
            wrapper_error_exit()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_pair_flag_is_a_clap_error() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        assert_eq!(execute(&[], &mut out, &mut err), wrapper_error_exit());
    }

    #[test]
    fn malformed_pair_is_rejected_before_state_access() {
        let args: Vec<OsString> = vec![OsString::from("--pair"), OsString::from("p1")];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        assert_eq!(execute(&args, &mut out, &mut err), wrapper_error_exit());
    }
}
