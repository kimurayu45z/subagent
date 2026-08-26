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

/// Projects the user-authored task text from a recognized provider command.
///
/// Provider flags are deliberately excluded: the exact argv is already
/// committed by [`command_digest`], while pair memory should carry the task
/// forward rather than repeatedly teaching the next child how its predecessor
/// was launched. The provider CLIs both conventionally accept one positional
/// prompt plus optional caller stdin. Unknown option syntax is left alone; if
/// no safe positional prompt can be identified, caller stdin (when present)
/// remains the source of truth.
pub(crate) fn project_task_request(
    kind: ChildKind,
    args: &[OsString],
    caller_stdin: &[u8],
) -> Vec<u8> {
    let positional_prompt: Option<Vec<u8>> = likely_positional_prompt(kind, args).map(os_bytes);
    match (positional_prompt, caller_stdin.is_empty()) {
        (Some(prompt), true) => prompt,
        (None, false) => caller_stdin.to_vec(),
        (Some(prompt), false) => {
            let mut projected: Vec<u8> = Vec::with_capacity(
                prompt
                    .len()
                    .saturating_add(caller_stdin.len())
                    .saturating_add(64),
            );
            projected.extend_from_slice(b"Positional prompt:\n");
            projected.extend_from_slice(&prompt);
            projected.extend_from_slice(b"\n\nCaller stdin:\n");
            projected.extend_from_slice(caller_stdin);
            projected
        }
        (None, true) => {
            b"[request text unavailable; exact child command retained only as a digest]".to_vec()
        }
    }
}

fn likely_positional_prompt(kind: ChildKind, args: &[OsString]) -> Option<&OsStr> {
    let first_index: usize = match kind {
        ChildKind::Claude => 0,
        ChildKind::Codex => 1,
    };
    if args.len() <= first_index {
        return None;
    }

    if let Some(separator_index) = args[first_index..]
        .iter()
        .position(|argument: &OsString| argument == OsStr::new("--"))
    {
        return args
            .get(first_index + separator_index + 1)
            .map(OsString::as_os_str);
    }

    let mut index: usize = args.len();
    while index > first_index {
        index -= 1;
        let argument: &OsStr = args[index].as_os_str();
        if argument == OsStr::new("-") || looks_like_option(argument) {
            continue;
        }
        if index > first_index && option_takes_value(kind, args[index - 1].as_os_str()) {
            index -= 1;
            continue;
        }
        return Some(argument);
    }
    None
}

fn looks_like_option(argument: &OsStr) -> bool {
    os_bytes(argument).first() == Some(&b'-')
}

fn option_takes_value(kind: ChildKind, option: &OsStr) -> bool {
    let raw: &[u8] = &os_bytes(option);
    if raw.contains(&b'=') {
        return false;
    }
    match kind {
        ChildKind::Claude => matches!(
            option.to_str(),
            Some(
                "--model"
                    | "--fallback-model"
                    | "--tools"
                    | "--allowedTools"
                    | "--disallowedTools"
                    | "--permission-mode"
                    | "--permission-prompt-tool"
                    | "--output-format"
                    | "--input-format"
                    | "--json-schema"
                    | "--max-turns"
                    | "--max-budget-usd"
                    | "--system-prompt"
                    | "--append-system-prompt"
                    | "--settings"
                    | "--mcp-config"
                    | "--agents"
                    | "--betas"
                    | "--plugin-dir"
                    | "--add-dir"
                    | "--debug-file"
                    | "--effort"
                    | "--session-id"
                    | "--resume"
                    | "-r"
            )
        ),
        ChildKind::Codex => matches!(
            option.to_str(),
            Some(
                "--config"
                    | "-c"
                    | "--image"
                    | "-i"
                    | "--model"
                    | "-m"
                    | "--local-provider"
                    | "--profile"
                    | "-p"
                    | "--sandbox"
                    | "-s"
                    | "--cd"
                    | "-C"
                    | "--add-dir"
                    | "--thread-source"
                    | "--output-schema"
                    | "--color"
                    | "--output-last-message"
                    | "-o"
            )
        ),
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

    #[test]
    fn projects_only_the_task_from_common_provider_commands() {
        let claude: Vec<u8> = project_task_request(
            ChildKind::Claude,
            &args(&[
                "-p",
                "--model",
                "haiku",
                "--no-session-persistence",
                "review the current diff",
            ]),
            &[],
        );
        assert_eq!(claude, b"review the current diff");

        let codex: Vec<u8> = project_task_request(
            ChildKind::Codex,
            &args(&[
                "exec",
                "--ephemeral",
                "--sandbox",
                "read-only",
                "--model",
                "gpt-5.6-luna",
                "summarize these findings",
            ]),
            &[],
        );
        assert_eq!(codex, b"summarize these findings");
    }

    #[test]
    fn projection_uses_stdin_and_never_records_a_trailing_option_value_as_the_task() {
        let stdin_only: Vec<u8> = project_task_request(
            ChildKind::Claude,
            &args(&["-p", "--model", "haiku"]),
            b"task from stdin",
        );
        assert_eq!(stdin_only, b"task from stdin");

        let combined: Vec<u8> = project_task_request(
            ChildKind::Codex,
            &args(&["exec", "positional task"]),
            b"additional stdin",
        );
        assert_eq!(
            combined,
            b"Positional prompt:\npositional task\n\nCaller stdin:\nadditional stdin"
        );
    }
}
