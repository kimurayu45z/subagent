//! `subagent forget --pair PAIR`: reports pair deletion as unavailable.
//!
//! A stateful placeholder; it never deletes or otherwise mutates any state.
//! Unlike `context`/`log`/`pairs`/`doctor`, the design grammar (section
//! 6.1) does not give `forget` a `--format` option, so this command only
//! ever produces a human-readable diagnostic.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

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

    let _ = writeln!(
        err,
        "subagent: forget is unavailable: pair deletion across the SQLite identity and exchange ledger (design.md section 10) is not implemented in this build (requested pair={})",
        forget_args.pair
    );
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
    fn valid_invocation_reports_unavailable_and_does_not_mutate_state() {
        let args: Vec<OsString> = vec![OsString::from("--pair"), OsString::from("p1")];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, wrapper_error_exit());
        assert!(out.is_empty());
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("unavailable"));
        assert!(text.contains("p1"));
    }
}
