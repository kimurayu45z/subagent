//! Recognition of the deliberately small managed-child surface.
//!
//! The adapter never rewrites provider arguments. Both supported installed
//! CLIs accept additional stdin alongside a positional prompt, so the runner
//! can inject a context bootstrap through stdin while preserving the caller's
//! original `OsString` argv byte-for-byte.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::store::ChildKind;

const COMMAND_DIGEST_DOMAIN: &[u8] = b"subagent.command.v1\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildAdapterError {
    UnsupportedProgram(OsString),
    ClaudeRequiresPrintMode,
    NativeSessionContinuityUnavailable(&'static str),
    CodexRequiresExec,
    CodexExecSubcommandUnsupported(&'static str),
}

impl fmt::Display for ChildAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChildAdapterError::UnsupportedProgram(program) => write!(
                f,
                "managed context supports only `claude -p` and `codex exec` in this build; \
                 got program {:?}. Use --context none --no-record for explicit passthrough",
                program
            ),
            ChildAdapterError::ClaudeRequiresPrintMode => write!(
                f,
                "managed Claude execution requires `claude -p` or `claude --print`; \
                 interactive Claude mode is not supported"
            ),
            ChildAdapterError::NativeSessionContinuityUnavailable(flag) => write!(
                f,
                "Claude native session option {flag} is not supported by the managed adapter \
                 yet; remove it or use --context none --no-record passthrough"
            ),
            ChildAdapterError::CodexRequiresExec => write!(
                f,
                "managed Codex execution requires the `codex exec` subcommand"
            ),
            ChildAdapterError::CodexExecSubcommandUnsupported(subcommand) => write!(
                f,
                "`codex exec {subcommand}` native session/subcommand behavior is not supported \
                 by the managed adapter yet"
            ),
        }
    }
}

impl std::error::Error for ChildAdapterError {}

/// Recognizes a child without modifying any argument. Unknown provider flags
/// are intentionally preserved because bootstrap injection does not need to
/// identify the positional prompt or guess option arity.
pub(crate) fn recognize_managed_child(
    program: &OsStr,
    args: &[OsString],
) -> Result<ChildKind, ChildAdapterError> {
    let basename: &OsStr = Path::new(program).file_name().unwrap_or(program);
    if basename == OsStr::new("claude") {
        recognize_claude(args)
    } else if basename == OsStr::new("codex") {
        recognize_codex(args)
    } else {
        Err(ChildAdapterError::UnsupportedProgram(
            program.to_os_string(),
        ))
    }
}

fn recognize_claude(args: &[OsString]) -> Result<ChildKind, ChildAdapterError> {
    let has_print: bool = args.iter().any(|argument: &OsString| {
        argument == OsStr::new("-p") || argument == OsStr::new("--print")
    });
    if !has_print {
        return Err(ChildAdapterError::ClaudeRequiresPrintMode);
    }

    for argument in args {
        let unsupported: Option<&'static str> = if argument == OsStr::new("--resume") {
            Some("--resume")
        } else if argument == OsStr::new("-r") {
            Some("-r")
        } else if argument == OsStr::new("--continue") {
            Some("--continue")
        } else if argument == OsStr::new("-c") {
            Some("-c")
        } else if argument == OsStr::new("--session-id") {
            Some("--session-id")
        } else if argument == OsStr::new("--fork-session") {
            Some("--fork-session")
        } else {
            None
        };
        if let Some(flag) = unsupported {
            return Err(ChildAdapterError::NativeSessionContinuityUnavailable(flag));
        }
    }

    Ok(ChildKind::Claude)
}

fn recognize_codex(args: &[OsString]) -> Result<ChildKind, ChildAdapterError> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("exec")) {
        return Err(ChildAdapterError::CodexRequiresExec);
    }
    if let Some(subcommand) = args
        .get(1)
        .and_then(|argument: &OsString| argument.to_str())
        && matches!(subcommand, "resume" | "fork" | "review")
    {
        return Err(ChildAdapterError::CodexExecSubcommandUnsupported(
            match subcommand {
                "resume" => "resume",
                "fork" => "fork",
                "review" => "review",
                _ => unreachable!("match guard restricts the subcommand"),
            },
        ));
    }
    Ok(ChildKind::Codex)
}

/// Stable SHA-256 digest of the exact child argv, including the program, with
/// raw OS bytes and explicit length framing.
pub(crate) fn command_digest(program: &OsStr, args: &[OsString]) -> [u8; 32] {
    let mut hasher: Sha256 = Sha256::new();
    hasher.update(COMMAND_DIGEST_DOMAIN);
    write_framed(&mut hasher, &os_bytes(program));
    for argument in args {
        write_framed(&mut hasher, &os_bytes(argument));
    }
    hasher.finalize().into()
}

fn write_framed(hasher: &mut Sha256, bytes: &[u8]) {
    let length: u64 = bytes.len() as u64;
    hasher.update(length.to_le_bytes());
    hasher.update(bytes);
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.as_encoded_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn recognizes_claude_print_without_parsing_unknown_provider_options() {
        let child: ChildKind = recognize_managed_child(
            OsStr::new("/usr/local/bin/claude"),
            &args(&["-p", "--future-option", "value", "review this"]),
        )
        .unwrap();
        assert_eq!(child, ChildKind::Claude);
    }

    #[test]
    fn rejects_interactive_and_native_resume_claude_forms() {
        assert!(matches!(
            recognize_managed_child(OsStr::new("claude"), &args(&["review"])),
            Err(ChildAdapterError::ClaudeRequiresPrintMode)
        ));
        assert!(matches!(
            recognize_managed_child(
                OsStr::new("claude"),
                &args(&["-p", "--resume", "session", "review"]),
            ),
            Err(ChildAdapterError::NativeSessionContinuityUnavailable(
                "--resume"
            ))
        ));
    }

    #[test]
    fn recognizes_codex_exec_and_rejects_native_session_subcommands() {
        assert_eq!(
            recognize_managed_child(
                OsStr::new("codex"),
                &args(&["exec", "--future-option", "value", "review"]),
            )
            .unwrap(),
            ChildKind::Codex
        );
        assert!(matches!(
            recognize_managed_child(OsStr::new("codex"), &args(&["exec", "resume", "id"])),
            Err(ChildAdapterError::CodexExecSubcommandUnsupported("resume"))
        ));
    }

    #[test]
    fn rejects_unknown_programs_for_managed_context() {
        assert!(matches!(
            recognize_managed_child(OsStr::new("bash"), &args(&["-c", "true"])),
            Err(ChildAdapterError::UnsupportedProgram(_))
        ));
    }

    #[test]
    fn command_digest_is_framed_and_stable() {
        let first: [u8; 32] = command_digest(OsStr::new("ab"), &args(&["c"]));
        let second: [u8; 32] = command_digest(OsStr::new("a"), &args(&["bc"]));
        assert_ne!(first, second);
        assert_eq!(first, command_digest(OsStr::new("ab"), &args(&["c"])));
    }
}
