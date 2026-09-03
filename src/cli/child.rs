//! Recognition of the deliberately small managed-child surface.
//!
//! Ordinary managed calls preserve provider arguments. Explicit workstreams
//! additionally inject wrapper-owned native continuity arguments after task
//! projection, command digesting, and profile hashing have consumed the
//! caller's original `OsString` argv.
//!
//! The command-profile hashing substrate below ([`ProfileHash`],
//! [`CommandProfile`], [`command_profile_hash`]) gates wrapper-managed Claude
//! workstream resume. The wrapper hashes caller argv, then injects its native
//! session arguments only into the final spawn argv.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::store::ChildKind;

const COMMAND_DIGEST_DOMAIN: &[u8] = b"subagent.command.v1\n";

/// Versions the byte layout hashed by [`command_profile_hash`], independent
/// of [`super::store::LEDGER_SCHEMA_VERSION`]: a stored row's
/// `profile_schema_version` pins it to the algorithm that produced it, so a
/// later change here never silently reinterprets an old hash.
pub(crate) const COMMAND_PROFILE_SCHEMA_VERSION: u32 = 2;
const COMMAND_PROFILE_DOMAIN: &[u8] = b"subagent.command-profile.v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeSessionMode {
    Assign,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexSessionMode<'a> {
    Fresh,
    Resume(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenCodeSessionMode<'a> {
    Fresh,
    Resume(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AntigravitySessionMode<'a> {
    Fresh,
    Resume(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildAdapterError {
    UnsupportedProgram(OsString),
    ClaudeRequiresPrintMode,
    ClaudePromptPlacementAmbiguous,
    CodexPromptPlacementAmbiguous,
    CodexWorkstreamOptionUnsupported(&'static str),
    NativeSessionContinuityUnavailable(&'static str),
    CodexRequiresExec,
    CodexExecSubcommandUnsupported(&'static str),
    OpenCodeRequiresRun,
    OpenCodePromptPlacementAmbiguous,
    OpenCodeOptionUnsupported(&'static str),
    AntigravityRequiresPrintMode,
    AntigravityPromptPlacementAmbiguous,
    AntigravityOptionUnsupported(&'static str),
    AntigravityPromptNonUtf8,
}

impl fmt::Display for ChildAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChildAdapterError::UnsupportedProgram(program) => write!(
                f,
                "managed context supports only `claude -p`, `codex exec`, `opencode run`, and `agy -p` \
                 in this build; \
                 got program {:?}. Use --context none --no-record for explicit passthrough",
                program
            ),
            ChildAdapterError::ClaudeRequiresPrintMode => write!(
                f,
                "managed Claude execution requires `claude -p` or `claude --print`; \
                 interactive Claude mode is not supported"
            ),
            ChildAdapterError::ClaudePromptPlacementAmbiguous => write!(
                f,
                "managed Claude execution requires the task immediately after \
                 `-p`/`--print`, after an explicit `--`, or through caller stdin; \
                 a trailing task after provider options is ambiguous"
            ),
            ChildAdapterError::CodexPromptPlacementAmbiguous => write!(
                f,
                "managed Codex workstreams require the task immediately after `exec`, after an \
                 explicit `--`, or through caller stdin"
            ),
            ChildAdapterError::CodexWorkstreamOptionUnsupported(flag) => write!(
                f,
                "Codex option {flag} cannot be combined with a managed workstream"
            ),
            ChildAdapterError::NativeSessionContinuityUnavailable(flag) => write!(
                f,
                "caller-supplied Claude native session option {flag} is not accepted by the \
                 managed adapter; use wrapper --workstream with --fresh/--resume, or use \
                 --context none --no-record passthrough"
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
            ChildAdapterError::OpenCodeRequiresRun => write!(
                f,
                "managed OpenCode execution requires the `opencode run` subcommand"
            ),
            ChildAdapterError::OpenCodePromptPlacementAmbiguous => write!(
                f,
                "managed OpenCode execution requires one quoted task immediately after `run`, \
                 after an explicit `--`, or through caller stdin"
            ),
            ChildAdapterError::OpenCodeOptionUnsupported(flag) => write!(
                f,
                "OpenCode option {flag} is not supported by the managed adapter; use wrapper \
                 --workstream for native continuity, or use --context none --no-record \
                 passthrough"
            ),
            ChildAdapterError::AntigravityRequiresPrintMode => write!(
                f,
                "managed Antigravity execution requires `agy -p TASK`, `agy --print TASK`, or \
                 `agy --prompt TASK`; interactive mode is not supported"
            ),
            ChildAdapterError::AntigravityPromptPlacementAmbiguous => write!(
                f,
                "managed Antigravity execution requires one quoted task immediately after \
                 `-p`/`--print`/`--prompt`"
            ),
            ChildAdapterError::AntigravityOptionUnsupported(flag) => write!(
                f,
                "Antigravity option {flag} is not supported by the managed adapter; use \
                 wrapper --workstream for exact continuity, or use --context none \
                 --no-record passthrough"
            ),
            ChildAdapterError::AntigravityPromptNonUtf8 => write!(
                f,
                "managed Antigravity prompts and caller stdin must be valid UTF-8 for the \
                 stream-json transport"
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
    } else if basename == OsStr::new("opencode") {
        recognize_opencode(args)
    } else if basename == OsStr::new("agy") || basename == OsStr::new("antigravity") {
        recognize_antigravity(args)
    } else {
        Err(ChildAdapterError::UnsupportedProgram(
            program.to_os_string(),
        ))
    }
}

/// Rejects Claude Code and OpenCode argv that require guessing where provider option values end
/// and the task begins. Caller stdin is unambiguous and remains valid.
///
/// This validation is for managed execution only. Explicit no-context,
/// no-record passthrough deliberately preserves provider behavior unchanged.
pub(crate) fn validate_managed_task_input(
    kind: ChildKind,
    args: &[OsString],
    caller_stdin: &[u8],
) -> Result<(), ChildAdapterError> {
    if !caller_stdin.is_empty() && kind != ChildKind::Antigravity {
        return Ok(());
    }
    match kind {
        ChildKind::Claude => {
            if claude_prompt_immediately_after_print(args).is_some()
                || explicit_separator_prompt(args, 0).is_some()
            {
                Ok(())
            } else {
                Err(ChildAdapterError::ClaudePromptPlacementAmbiguous)
            }
        }
        ChildKind::Codex => Ok(()),
        ChildKind::OpenCode => {
            if opencode_prompt_immediately_after_run_index(args).is_some()
                || opencode_explicit_separator_prompt_index(args).is_some()
            {
                Ok(())
            } else {
                Err(ChildAdapterError::OpenCodePromptPlacementAmbiguous)
            }
        }
        ChildKind::Antigravity => {
            let Some(index) = antigravity_prompt_index(args) else {
                return Err(ChildAdapterError::AntigravityPromptPlacementAmbiguous);
            };
            if args[index].to_str().is_none() || std::str::from_utf8(caller_stdin).is_err() {
                Err(ChildAdapterError::AntigravityPromptNonUtf8)
            } else {
                Ok(())
            }
        }
    }
}

/// Applies the stricter input contract required when the wrapper must rewrite
/// provider argv for exact native continuity. Ordinary untracked `codex exec`
/// retains the broader byte-transparent compatibility grammar.
pub(crate) fn validate_managed_continuity_input(
    kind: ChildKind,
    args: &[OsString],
    caller_stdin: &[u8],
) -> Result<(), ChildAdapterError> {
    if kind == ChildKind::OpenCode {
        if !opencode_format_is_compatible(args) {
            return Err(ChildAdapterError::OpenCodeOptionUnsupported(
                "--format other than json",
            ));
        }
        return validate_managed_task_input(kind, args, caller_stdin);
    }
    if kind == ChildKind::Antigravity {
        return validate_managed_task_input(kind, args, caller_stdin);
    }
    if kind == ChildKind::Claude {
        return validate_managed_task_input(kind, args, caller_stdin);
    }
    for argument in codex_provider_arguments(args) {
        let name: Option<&str> = profile_excluded_option_name(argument.as_os_str());
        if name == Some("--ephemeral") {
            return Err(ChildAdapterError::CodexWorkstreamOptionUnsupported(
                "--ephemeral",
            ));
        }
    }
    if !caller_stdin.is_empty() || profile_task_index(kind, args).is_some() {
        Ok(())
    } else {
        Err(ChildAdapterError::CodexPromptPlacementAmbiguous)
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
    let index: usize = likely_positional_prompt_index(kind, args)?;
    Some(args[index].as_os_str())
}

fn likely_positional_prompt_index(kind: ChildKind, args: &[OsString]) -> Option<usize> {
    let first_index: usize = match kind {
        ChildKind::Claude => 0,
        ChildKind::Codex => 1,
        ChildKind::OpenCode => 1,
        ChildKind::Antigravity => 0,
    };
    if args.len() <= first_index {
        return None;
    }

    if kind == ChildKind::Claude
        && let Some(index) = claude_prompt_immediately_after_print_index(args)
    {
        return Some(index);
    }

    if kind == ChildKind::OpenCode
        && let Some(index) = opencode_prompt_immediately_after_run_index(args)
    {
        return Some(index);
    }

    if kind == ChildKind::Antigravity {
        return antigravity_prompt_index(args);
    }

    let separator_prompt_index: Option<usize> = if kind == ChildKind::OpenCode {
        opencode_explicit_separator_prompt_index(args)
    } else {
        explicit_separator_prompt_index(args, first_index)
    };
    if let Some(index) = separator_prompt_index {
        return Some(index);
    }

    if matches!(
        kind,
        ChildKind::Claude | ChildKind::OpenCode | ChildKind::Antigravity
    ) {
        return None;
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
        return Some(index);
    }
    None
}

fn claude_prompt_immediately_after_print(args: &[OsString]) -> Option<&OsStr> {
    let index: usize = claude_prompt_immediately_after_print_index(args)?;
    Some(args[index].as_os_str())
}

fn claude_prompt_immediately_after_print_index(args: &[OsString]) -> Option<usize> {
    let print_index: usize = args.iter().position(|argument: &OsString| {
        argument == OsStr::new("-p") || argument == OsStr::new("--print")
    })?;
    let candidate_index: usize = print_index + 1;
    let candidate: &OsStr = args.get(candidate_index)?.as_os_str();
    if candidate == OsStr::new("-") || looks_like_option(candidate) {
        None
    } else {
        Some(candidate_index)
    }
}

fn opencode_prompt_immediately_after_run_index(args: &[OsString]) -> Option<usize> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("run")) {
        return None;
    }
    let candidate_index: usize = 1;
    let candidate: &OsStr = args.get(candidate_index)?.as_os_str();
    if candidate == OsStr::new("-") || looks_like_option(candidate) {
        return None;
    }
    if args
        .get(candidate_index + 1)
        .is_some_and(|argument: &OsString| !looks_like_option(argument.as_os_str()))
    {
        return None;
    }
    Some(candidate_index)
}

fn opencode_explicit_separator_prompt_index(args: &[OsString]) -> Option<usize> {
    let index: usize = explicit_separator_prompt_index(args, 1)?;
    if index + 1 == args.len() {
        Some(index)
    } else {
        None
    }
}

fn explicit_separator_prompt(args: &[OsString], first_index: usize) -> Option<&OsStr> {
    let index: usize = explicit_separator_prompt_index(args, first_index)?;
    Some(args[index].as_os_str())
}

/// The index of the token immediately following an explicit `--` separator
/// at or after `first_index`, or `None` if there is no such separator or it
/// is the last token. Shared by the prompt-placement heuristic and
/// [`command_profile_hash`]'s task-token exclusion, which locate the same
/// token for different reasons.
fn explicit_separator_prompt_index(args: &[OsString], first_index: usize) -> Option<usize> {
    let separator_index: usize = args[first_index..]
        .iter()
        .position(|argument: &OsString| argument == OsStr::new("--"))?;
    let candidate_index: usize = first_index + separator_index + 1;
    if candidate_index < args.len() {
        Some(candidate_index)
    } else {
        None
    }
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
                    | "--allowed-tools"
                    | "--disallowedTools"
                    | "--disallowed-tools"
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
        ChildKind::OpenCode => matches!(
            option.to_str(),
            Some(
                "--command"
                    | "--session"
                    | "-s"
                    | "--model"
                    | "-m"
                    | "--agent"
                    | "--format"
                    | "--file"
                    | "-f"
                    | "--title"
                    | "--attach"
                    | "--password"
                    | "-p"
                    | "--username"
                    | "-u"
                    | "--dir"
                    | "--port"
                    | "--variant"
                    | "--log-level"
                    | "--replay-limit"
            )
        ),
        ChildKind::Antigravity => matches!(
            option.to_str(),
            Some(
                "--conversation"
                    | "--output-format"
                    | "--input-format"
                    | "--json-schema"
                    | "--log-file"
                    | "--mode"
                    | "--model"
                    | "--agent"
                    | "--project"
                    | "--add-dir"
                    | "--sandbox"
                    | "--print-timeout"
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

fn recognize_opencode(args: &[OsString]) -> Result<ChildKind, ChildAdapterError> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("run")) {
        return Err(ChildAdapterError::OpenCodeRequiresRun);
    }
    for argument in &args[1..] {
        if let Some(text) = argument.to_str()
            && !text.starts_with("--")
            && text.len() > 2
            && (text.starts_with("-s") || text.starts_with("-c") || text.starts_with("-i"))
        {
            return Err(ChildAdapterError::OpenCodeOptionUnsupported(
                "attached native short option",
            ));
        }
        let name: Option<&str> = profile_excluded_option_name(argument.as_os_str());
        let unsupported: Option<&'static str> = match name {
            Some("--session") | Some("-s") => Some("--session/-s"),
            Some("--continue") | Some("-c") => Some("--continue/-c"),
            Some("--fork") => Some("--fork"),
            Some("--interactive") | Some("-i") | Some("--mini") => Some("--interactive/-i"),
            Some("--command") => Some("--command"),
            Some("--attach") => Some("--attach"),
            _ => None,
        };
        if let Some(flag) = unsupported {
            return Err(ChildAdapterError::OpenCodeOptionUnsupported(flag));
        }
    }
    Ok(ChildKind::OpenCode)
}

fn recognize_antigravity(args: &[OsString]) -> Result<ChildKind, ChildAdapterError> {
    let selector_count: usize = args
        .iter()
        .filter(|argument: &&OsString| {
            matches!(
                argument.as_os_str().to_str(),
                Some("-p" | "--print" | "--prompt")
            )
        })
        .count();
    if selector_count == 0 {
        return Err(ChildAdapterError::AntigravityRequiresPrintMode);
    }
    if selector_count != 1 {
        return Err(ChildAdapterError::AntigravityPromptPlacementAmbiguous);
    }
    for argument in args {
        if let Some(text) = argument.to_str()
            && text.len() > 2
            && (text.starts_with("-c") || text.starts_with("-i"))
            && !text.starts_with("--")
        {
            return Err(ChildAdapterError::AntigravityOptionUnsupported(
                "attached native short option",
            ));
        }
        let name: Option<&str> = profile_excluded_option_name(argument.as_os_str());
        let unsupported: Option<&'static str> = match name {
            Some("--conversation") => Some("--conversation"),
            Some("--continue") | Some("-c") => Some("--continue/-c"),
            Some("--prompt-interactive") | Some("-i") => Some("--prompt-interactive/-i"),
            Some("--input-format") => Some("--input-format"),
            Some("--output-format") if !antigravity_output_format_is_compatible(args) => {
                Some("--output-format other than stream-json")
            }
            _ => None,
        };
        if let Some(flag) = unsupported {
            return Err(ChildAdapterError::AntigravityOptionUnsupported(flag));
        }
    }
    Ok(ChildKind::Antigravity)
}

fn antigravity_print_selector_index(args: &[OsString]) -> Option<usize> {
    args.iter().position(|argument: &OsString| {
        matches!(argument.to_str(), Some("-p" | "--print" | "--prompt"))
    })
}

fn antigravity_prompt_index(args: &[OsString]) -> Option<usize> {
    let selector_index: usize = antigravity_print_selector_index(args)?;
    let prompt_index: usize = selector_index.saturating_add(1);
    let prompt: &OsStr = args.get(prompt_index)?.as_os_str();
    if looks_like_option(prompt) {
        return None;
    }
    Some(prompt_index)
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

/// Builds the exact Claude argv used for managed native continuity. Validation,
/// task projection, command digesting, and profile hashing must all happen on
/// the caller argv before this wrapper-owned pair is prepended.
pub(crate) fn inject_claude_session_args(
    caller_args: &[OsString],
    mode: ClaudeSessionMode,
    native_id: &str,
) -> Vec<OsString> {
    let flag: &str = match mode {
        ClaudeSessionMode::Assign => "--session-id",
        ClaudeSessionMode::Resume => "--resume",
    };
    let mut spawn_args: Vec<OsString> = Vec::with_capacity(caller_args.len().saturating_add(2));
    spawn_args.push(OsString::from(flag));
    spawn_args.push(OsString::from(native_id));
    spawn_args.extend_from_slice(caller_args);
    spawn_args
}

/// Builds the Codex argv used for a managed workstream. The wrapper-owned
/// `--json` transport is never part of task projection, command digesting, or
/// profile hashing. Resume inserts the exact stored thread ID as the first
/// positional argument, including after an explicit provider `--` separator.
pub(crate) fn inject_codex_session_args(
    caller_args: &[OsString],
    mode: CodexSessionMode<'_>,
) -> Vec<OsString> {
    debug_assert_eq!(
        caller_args.first().map(OsString::as_os_str),
        Some(OsStr::new("exec"))
    );
    match mode {
        CodexSessionMode::Fresh => {
            let mut spawn_args: Vec<OsString> =
                Vec::with_capacity(caller_args.len().saturating_add(1));
            spawn_args.push(OsString::from("exec"));
            if !codex_json_requested(caller_args) {
                spawn_args.push(OsString::from("--json"));
            }
            spawn_args.extend_from_slice(&caller_args[1..]);
            spawn_args
        }
        CodexSessionMode::Resume(native_id) => {
            let separator_index: Option<usize> = caller_args[1..]
                .iter()
                .position(|argument: &OsString| argument == OsStr::new("--"))
                .map(|index: usize| index + 1);
            let mut spawn_args: Vec<OsString> =
                Vec::with_capacity(caller_args.len().saturating_add(3));
            spawn_args.extend([OsString::from("exec"), OsString::from("resume")]);
            if !codex_json_requested(caller_args) {
                spawn_args.push(OsString::from("--json"));
            }
            match separator_index {
                Some(index) => {
                    append_codex_resume_compatible_args(&mut spawn_args, &caller_args[1..index]);
                    spawn_args.push(OsString::from("--"));
                    spawn_args.push(OsString::from(native_id));
                    spawn_args.extend_from_slice(&caller_args[index + 1..]);
                }
                None if profile_task_index(ChildKind::Codex, caller_args) == Some(1) => {
                    spawn_args.push(OsString::from(native_id));
                    append_codex_resume_compatible_args(&mut spawn_args, &caller_args[1..]);
                }
                None => {
                    append_codex_resume_compatible_args(&mut spawn_args, &caller_args[1..]);
                    spawn_args.push(OsString::from(native_id));
                    spawn_args.push(OsString::from("-"));
                }
            }
            spawn_args
        }
    }
}

/// Builds the OpenCode argv used for managed native continuity. Fresh runs
/// add wrapper-owned JSONL output so the exact provider session can be
/// observed. Resume additionally selects only the stored session ID.
pub(crate) fn inject_opencode_session_args(
    caller_args: &[OsString],
    mode: OpenCodeSessionMode<'_>,
) -> Vec<OsString> {
    debug_assert_eq!(
        caller_args.first().map(OsString::as_os_str),
        Some(OsStr::new("run"))
    );
    let extra_capacity: usize = match mode {
        OpenCodeSessionMode::Fresh => 2,
        OpenCodeSessionMode::Resume(_) => 4,
    };
    let mut spawn_args: Vec<OsString> =
        Vec::with_capacity(caller_args.len().saturating_add(extra_capacity));
    spawn_args.push(OsString::from("run"));
    if !opencode_json_requested(caller_args) {
        spawn_args.extend([OsString::from("--format"), OsString::from("json")]);
    }
    if let OpenCodeSessionMode::Resume(native_id) = mode {
        spawn_args.extend([OsString::from("--session"), OsString::from(native_id)]);
    }
    spawn_args.extend_from_slice(&caller_args[1..]);
    spawn_args
}

/// Builds the Antigravity argv used by every managed invocation. Its normal
/// print mode does not merge piped stdin with the positional task, so the
/// wrapper removes the caller's print selector/task and owns the stream-JSON
/// input/output transport. Resume selects only the exact stored conversation.
pub(crate) fn inject_antigravity_session_args(
    caller_args: &[OsString],
    mode: AntigravitySessionMode<'_>,
) -> Vec<OsString> {
    let selector_index: usize = antigravity_print_selector_index(caller_args)
        .expect("recognized Antigravity argv has a print selector");
    let prompt_index: usize = antigravity_prompt_index(caller_args)
        .expect("validated Antigravity argv has a prompt after its selector");
    let mut spawn_args: Vec<OsString> = Vec::with_capacity(caller_args.len().saturating_add(6));
    let mut index: usize = 0;
    while index < caller_args.len() {
        if index == selector_index || index == prompt_index {
            index = index.saturating_add(1);
            continue;
        }
        let argument: &OsString = &caller_args[index];
        let name: Option<&str> = profile_excluded_option_name(argument.as_os_str());
        if name == Some("--output-format") {
            index = index.saturating_add(1);
            if !option_has_inline_value(argument.as_os_str()) {
                index = index.saturating_add(1);
            }
            continue;
        }
        spawn_args.push(argument.clone());
        index = index.saturating_add(1);
    }
    spawn_args.extend([
        OsString::from("--print="),
        OsString::from("--input-format"),
        OsString::from("stream-json"),
        OsString::from("--output-format"),
        OsString::from("stream-json"),
    ]);
    if let AntigravitySessionMode::Resume(native_id) = mode {
        spawn_args.extend([OsString::from("--conversation"), OsString::from(native_id)]);
    }
    spawn_args
}

pub(crate) fn codex_json_requested(args: &[OsString]) -> bool {
    codex_provider_arguments(args).any(|argument: &OsString| {
        profile_excluded_option_name(argument.as_os_str()) == Some("--json")
    })
}

pub(crate) fn opencode_json_requested(args: &[OsString]) -> bool {
    let end: usize = args
        .iter()
        .position(|argument: &OsString| argument == OsStr::new("--"))
        .unwrap_or(args.len());
    let mut index: usize = 1;
    while index < end {
        let argument: &OsStr = args[index].as_os_str();
        let Some(text) = argument.to_str() else {
            index = index.saturating_add(1);
            continue;
        };
        if text == "--format" {
            return args
                .get(index + 1)
                .and_then(|value: &OsString| value.to_str())
                == Some("json");
        }
        if let Some(value) = text.strip_prefix("--format=") {
            return value == "json";
        }
        index = index.saturating_add(1);
    }
    false
}

pub(crate) fn antigravity_stream_json_requested(args: &[OsString]) -> bool {
    let mut index: usize = 0;
    while index < args.len() {
        let argument: &OsStr = args[index].as_os_str();
        let Some(text) = argument.to_str() else {
            index = index.saturating_add(1);
            continue;
        };
        if text == "--output-format" {
            return args
                .get(index.saturating_add(1))
                .and_then(|value: &OsString| value.to_str())
                == Some("stream-json");
        }
        if let Some(value) = text.strip_prefix("--output-format=") {
            return value == "stream-json";
        }
        index = index.saturating_add(1);
    }
    false
}

fn antigravity_output_format_is_compatible(args: &[OsString]) -> bool {
    let mut index: usize = 0;
    let mut count: usize = 0;
    while index < args.len() {
        let argument: &OsStr = args[index].as_os_str();
        let Some(text) = argument.to_str() else {
            index = index.saturating_add(1);
            continue;
        };
        if text == "--output-format" {
            count = count.saturating_add(1);
            if args
                .get(index.saturating_add(1))
                .and_then(|value: &OsString| value.to_str())
                != Some("stream-json")
            {
                return false;
            }
            index = index.saturating_add(2);
            continue;
        }
        if let Some(value) = text.strip_prefix("--output-format=") {
            count = count.saturating_add(1);
            if value != "stream-json" {
                return false;
            }
        }
        index = index.saturating_add(1);
    }
    count <= 1
}

fn opencode_format_is_compatible(args: &[OsString]) -> bool {
    let end: usize = args
        .iter()
        .position(|argument: &OsString| argument == OsStr::new("--"))
        .unwrap_or(args.len());
    let has_format: bool = args[..end].iter().any(|argument: &OsString| {
        profile_excluded_option_name(argument.as_os_str()) == Some("--format")
    });
    !has_format || opencode_json_requested(args)
}

fn codex_provider_arguments(args: &[OsString]) -> impl Iterator<Item = &OsString> {
    let end: usize = args
        .iter()
        .position(|argument: &OsString| argument == OsStr::new("--"))
        .unwrap_or(args.len());
    args[..end].iter()
}

/// Codex 0.149 persists these launch settings on `exec` but does not accept
/// them in the `exec resume` grammar. Configuration-affecting values remain in
/// the caller-derived command profile so changes fail closed before spawn;
/// output-only color remains excluded by the profile policy.
fn append_codex_resume_compatible_args(target: &mut Vec<OsString>, source: &[OsString]) {
    let mut index: usize = 0;
    while index < source.len() {
        let argument: &OsString = &source[index];
        let name: Option<&str> = profile_excluded_option_name(argument.as_os_str());
        let unsupported_value_arity: Option<usize> = name.and_then(|value: &str| match value {
            "--oss" | "--approve-for-me" => Some(0),
            "--local-provider" | "--profile" | "-p" | "--sandbox" | "-s" | "--cd" | "-C"
            | "--add-dir" | "--color" => Some(1),
            _ => None,
        });
        if let Some(value_arity) = unsupported_value_arity {
            let uses_equals_form: bool = argument
                .to_str()
                .is_some_and(|text: &str| text.contains('='));
            index = index.saturating_add(1);
            if value_arity == 1 && !uses_equals_form && index < source.len() {
                index = index.saturating_add(1);
            }
            continue;
        }
        target.push(argument.clone());
        index = index.saturating_add(1);
    }
}

fn write_framed(hasher: &mut Sha256, bytes: &[u8]) {
    let length: u64 = bytes.len() as u64;
    hasher.update(length.to_le_bytes());
    hasher.update(bytes);
}

/// A 32-byte SHA-256 digest identifying a child's non-task launch
/// configuration, as produced by [`command_profile_hash`]. Mirrors
/// [`super::pair_key::PairKey`]'s byte/hex accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProfileHash([u8; 32]);

impl ProfileHash {
    #[allow(dead_code)]
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[allow(dead_code)]
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> ProfileHash {
        ProfileHash(bytes)
    }

    #[allow(dead_code)]
    pub(crate) fn to_hex(self) -> String {
        let mut hex: String = String::with_capacity(self.0.len() * 2);
        for byte in &self.0 {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

/// The exact launch configuration hashed by [`command_profile_hash`]:
/// everything about a child invocation except its task text and caller
/// stdin, both of which vary every run and are already recorded as exchange
/// history.
#[allow(dead_code)]
pub(crate) struct CommandProfile<'a> {
    pub child_kind: ChildKind,
    pub program: &'a OsStr,
    pub working_directory: &'a Path,
    pub args: &'a [OsString],
}

/// Stable SHA-256 hash of a child's launch configuration, excluding the task
/// text and a short list of session-continuity, output-shaping, and
/// per-run-budget options that the managed wrapper injects or that vary
/// every run without changing what the provider does. Every other argv
/// token — including any option this build does not recognize — is
/// included by default, so an unrecognized flag is treated as a
/// configuration change rather than silently ignored: see
/// [`ChildAdapterError::ClaudePromptPlacementAmbiguous`]'s "unknown implies
/// incompatible" posture.
///
/// The `profile_schema_version` accompanying a stored hash pins it to
/// [`COMMAND_PROFILE_SCHEMA_VERSION`], so a later change to this function's
/// byte layout never gets reused across algorithm versions.
#[allow(dead_code)]
pub(crate) fn command_profile_hash(profile: &CommandProfile<'_>) -> ProfileHash {
    let excluded: Vec<bool> = excluded_profile_token_mask(profile.child_kind, profile.args);
    let residual_token_count: u64 =
        excluded.iter().filter(|is_excluded| !**is_excluded).count() as u64;

    let mut hasher: Sha256 = Sha256::new();
    hasher.update(COMMAND_PROFILE_DOMAIN);
    write_framed(&mut hasher, &COMMAND_PROFILE_SCHEMA_VERSION.to_le_bytes());
    write_framed(&mut hasher, profile.child_kind.to_string().as_bytes());
    write_framed(&mut hasher, &os_bytes(profile.program));
    write_framed(
        &mut hasher,
        &os_bytes(profile.working_directory.as_os_str()),
    );
    write_framed(&mut hasher, &residual_token_count.to_le_bytes());
    for (index, argument) in profile.args.iter().enumerate() {
        if excluded[index] {
            continue;
        }
        write_framed(&mut hasher, &os_bytes(argument));
    }
    ProfileHash(hasher.finalize().into())
}

/// Marks, by index, every argv token [`command_profile_hash`] must exclude:
/// the task token, the mode-selector token(s), and any token on the short
/// exclusion list together with its value (if the option takes one, per
/// [`option_takes_value`]). A token whose text cannot be decoded as UTF-8
/// never matches an exclusion rule and is therefore always retained,
/// matching the default-include posture.
#[allow(dead_code)]
fn excluded_profile_token_mask(kind: ChildKind, args: &[OsString]) -> Vec<bool> {
    let mut excluded: Vec<bool> = vec![false; args.len()];

    match kind {
        ChildKind::Claude => {
            for (index, argument) in args.iter().enumerate() {
                if argument == OsStr::new("-p") || argument == OsStr::new("--print") {
                    excluded[index] = true;
                }
            }
        }
        ChildKind::Codex => {
            if args.first().map(OsString::as_os_str) == Some(OsStr::new("exec")) {
                excluded[0] = true;
            }
        }
        ChildKind::OpenCode => {
            if args.first().map(OsString::as_os_str) == Some(OsStr::new("run")) {
                excluded[0] = true;
            }
        }
        ChildKind::Antigravity => {
            if let Some(index) = antigravity_print_selector_index(args) {
                excluded[index] = true;
            }
        }
    }

    let task_index: Option<usize> = profile_task_index(kind, args);
    if let Some(index) = task_index {
        excluded[index] = true;
        if index > 0 && args[index - 1] == OsStr::new("--") {
            excluded[index - 1] = true;
        }
    }

    let mut index: usize = 0;
    while index < args.len() {
        if excluded[index] {
            index += 1;
            continue;
        }
        let is_excluded_option: bool = profile_excluded_option_name(args[index].as_os_str())
            .map(|name: &str| is_excluded_profile_option_name(kind, name))
            .unwrap_or(false);
        if is_excluded_option {
            excluded[index] = true;
            if option_takes_value(kind, args[index].as_os_str())
                && !option_has_inline_value(args[index].as_os_str())
                && index + 1 < args.len()
            {
                excluded[index + 1] = true;
                index += 2;
                continue;
            }
        }
        index += 1;
    }

    excluded
}

/// Locates only a task position whose syntax is unambiguous enough to omit
/// from a resume-compatibility hash. Request projection may heuristically use
/// a trailing Codex positional, but doing that here could mistake a future
/// variadic option value for the task and create a false-compatible profile.
#[allow(dead_code)]
fn profile_task_index(kind: ChildKind, args: &[OsString]) -> Option<usize> {
    match kind {
        ChildKind::Claude => claude_prompt_immediately_after_print_index(args)
            .or_else(|| explicit_separator_prompt_index(args, 0)),
        ChildKind::Codex => {
            if let Some(index) = explicit_separator_prompt_index(args, 1) {
                return Some(index);
            }
            let candidate_index: usize = 1;
            let candidate: &OsStr = args.get(candidate_index)?.as_os_str();
            if candidate == OsStr::new("-") || looks_like_option(candidate) {
                None
            } else {
                Some(candidate_index)
            }
        }
        ChildKind::OpenCode => opencode_prompt_immediately_after_run_index(args)
            .or_else(|| opencode_explicit_separator_prompt_index(args)),
        ChildKind::Antigravity => antigravity_prompt_index(args),
    }
}

/// The option name portion of an argv token, splitting off a `--opt=value`
/// suffix so `--opt=value` and `--opt value` are recognized as the same
/// option name for exclusion purposes. `None` for non-UTF-8 tokens.
#[allow(dead_code)]
fn profile_excluded_option_name(token: &OsStr) -> Option<&str> {
    let text: &str = token.to_str()?;
    Some(text.split_once('=').map(|(name, _)| name).unwrap_or(text))
}

fn option_has_inline_value(token: &OsStr) -> bool {
    token.to_str().is_some_and(|text: &str| text.contains('='))
}

/// The exclusion list from the command-profile hash design: session-
/// continuity options (injected by the wrapper after hashing),
/// output-shaping/telemetry options, and per-run budget knobs. Each entry is
/// a known single-token or `--opt value` pair; arity is resolved separately
/// via [`option_takes_value`].
#[allow(dead_code)]
fn is_excluded_profile_option_name(kind: ChildKind, name: &str) -> bool {
    match kind {
        ChildKind::Claude => matches!(
            name,
            "--session-id"
                | "--resume"
                | "-r"
                | "--continue"
                | "-c"
                | "--fork-session"
                | "--output-format"
                | "--input-format"
                | "--json-schema"
                | "--debug-file"
                | "--max-turns"
                | "--max-budget-usd"
        ),
        ChildKind::Codex => matches!(
            name,
            "--color" | "-o" | "--output-last-message" | "--output-schema" | "--json"
        ),
        ChildKind::OpenCode => matches!(
            name,
            "--session"
                | "-s"
                | "--continue"
                | "-c"
                | "--fork"
                | "--format"
                | "--print-logs"
                | "--log-level"
                | "--thinking"
        ),
        ChildKind::Antigravity => matches!(
            name,
            "--conversation"
                | "--continue"
                | "-c"
                | "--output-format"
                | "--input-format"
                | "--json-schema"
                | "--log-file"
                | "--print-timeout"
        ),
    }
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
    fn recognizes_opencode_run_and_rejects_caller_native_continuity() {
        assert_eq!(
            recognize_managed_child(
                OsStr::new("/usr/local/bin/opencode"),
                &args(&["run", "review this", "--model", "opencode/big-pickle"]),
            )
            .unwrap(),
            ChildKind::OpenCode
        );
        for arguments in [
            args(&["run", "task", "--session", "ses_owned"]),
            args(&["run", "task", "-s=ses_owned"]),
            args(&["run", "task", "--continue"]),
            args(&["run", "task", "--fork"]),
        ] {
            assert!(matches!(
                recognize_managed_child(OsStr::new("opencode"), &arguments),
                Err(ChildAdapterError::OpenCodeOptionUnsupported(_))
            ));
        }
        assert!(matches!(
            recognize_managed_child(OsStr::new("opencode"), &args(&["session", "list"])),
            Err(ChildAdapterError::OpenCodeRequiresRun)
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
    fn command_profile_hash_is_framed_and_stable() {
        let cwd = Path::new("/workspace");
        let first: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program: OsStr::new("ab"),
            working_directory: cwd,
            args: &args(&["exec", "c"]),
        });
        let second: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program: OsStr::new("a"),
            working_directory: cwd,
            args: &args(&["exec", "bc"]),
        });
        assert_ne!(first, second);

        let repeat: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program: OsStr::new("ab"),
            working_directory: cwd,
            args: &args(&["exec", "c"]),
        });
        assert_eq!(first, repeat);
    }

    #[test]
    fn command_profile_hash_ignores_claude_task_text() {
        let cwd = Path::new("/workspace");
        let program = OsStr::new("claude");
        let first: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program,
            working_directory: cwd,
            args: &args(&["-p", "review the diff", "--model", "haiku"]),
        });
        let second: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program,
            working_directory: cwd,
            args: &args(&["-p", "an entirely different task", "--model", "haiku"]),
        });
        assert_eq!(first, second);
    }

    #[test]
    fn command_profile_hash_ignores_only_unambiguous_codex_task_text() {
        let cwd = Path::new("/workspace");
        let program = OsStr::new("codex");
        let first: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program,
            working_directory: cwd,
            args: &args(&["exec", "--sandbox", "read-only", "--", "review this"]),
        });
        let second: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program,
            working_directory: cwd,
            args: &args(&["exec", "--sandbox", "read-only", "--", "summarize that"]),
        });
        assert_eq!(first, second);

        let ordinary_first: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program,
            working_directory: cwd,
            args: &args(&["exec", "ordinary task", "--sandbox", "read-only"]),
        });
        let ordinary_second: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program,
            working_directory: cwd,
            args: &args(&["exec", "different task", "--sandbox", "read-only"]),
        });
        assert_eq!(ordinary_first, ordinary_second);

        let ambiguous_a: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program,
            working_directory: cwd,
            args: &args(&["exec", "--future-variadic", "value-a"]),
        });
        let ambiguous_b: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program,
            working_directory: cwd,
            args: &args(&["exec", "--future-variadic", "value-b"]),
        });
        assert_ne!(ambiguous_a, ambiguous_b);
    }

    #[test]
    fn opencode_profile_ignores_task_and_wrapper_transport_but_keeps_model() {
        let cwd: &Path = Path::new("/workspace");
        let first: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::OpenCode,
            program: OsStr::new("opencode"),
            working_directory: cwd,
            args: &args(&["run", "review the diff", "--model", "opencode/big-pickle"]),
        });
        let second: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::OpenCode,
            program: OsStr::new("opencode"),
            working_directory: cwd,
            args: &args(&[
                "run",
                "different task",
                "--format=json",
                "--model",
                "opencode/big-pickle",
            ]),
        });
        let different_model: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::OpenCode,
            program: OsStr::new("opencode"),
            working_directory: cwd,
            args: &args(&["run", "different task", "--model", "anthropic/haiku"]),
        });
        assert_eq!(first, second);
        assert_ne!(first, different_model);
    }

    #[test]
    fn provider_specific_short_flags_do_not_cross_contaminate_profiles() {
        let cwd: &Path = Path::new("/workspace");
        let codex_without_config: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program: OsStr::new("codex"),
            working_directory: cwd,
            args: &args(&["exec", "task"]),
        });
        let codex_with_config: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Codex,
            program: OsStr::new("codex"),
            working_directory: cwd,
            args: &args(&["exec", "task", "-c", "model_reasoning_effort=low"]),
        });
        assert_ne!(codex_without_config, codex_with_config);
    }

    #[test]
    fn command_profile_hash_changes_with_model_cwd_program_or_unknown_option() {
        let base_args: Vec<OsString> = args(&["-p", "task", "--model", "haiku"]);
        let base: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &base_args,
        });

        let different_model: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&["-p", "task", "--model", "opus"]),
        });
        assert_ne!(base, different_model);

        let different_cwd: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/other-workspace"),
            args: &base_args,
        });
        assert_ne!(base, different_cwd);

        let different_program: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("/usr/local/bin/claude"),
            working_directory: Path::new("/workspace"),
            args: &base_args,
        });
        assert_ne!(base, different_program);

        let unknown_option: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&["-p", "task", "--model", "haiku", "--future-flag", "value"]),
        });
        assert_ne!(base, unknown_option);
    }

    #[test]
    fn command_profile_hash_ignores_session_continuity_and_output_shaping_flags() {
        let base: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&["-p", "task", "--model", "haiku"]),
        });
        let with_continuity_and_output: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&[
                "-p",
                "task",
                "--model",
                "haiku",
                "--session-id",
                "abc-123",
                "--resume",
                "def-456",
                "--continue",
                "--fork-session",
                "--output-format",
                "json",
            ]),
        });
        assert_eq!(base, with_continuity_and_output);
    }

    #[test]
    fn wrapper_injected_claude_session_args_preserve_profile_and_prompt_adjacency() {
        let caller: Vec<OsString> = args(&["-p", "review the current diff", "--model", "haiku"]);
        let injected: Vec<OsString> = inject_claude_session_args(
            &caller,
            ClaudeSessionMode::Assign,
            "018f4e5c-5d6a-7b8c-9d0e-123456789abc",
        );
        assert_eq!(injected[0], "--session-id");
        assert_eq!(injected[2], "-p");
        assert_eq!(injected[3], "review the current diff");

        let caller_hash: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &caller,
        });
        let injected_hash: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &injected,
        });
        assert_eq!(caller_hash, injected_hash);
    }

    #[test]
    fn wrapper_injected_codex_args_preserve_canonical_task_shapes() {
        let immediate: Vec<OsString> = args(&["exec", "task", "--model", "gpt-luna"]);
        assert_eq!(
            inject_codex_session_args(&immediate, CodexSessionMode::Fresh),
            args(&["exec", "--json", "task", "--model", "gpt-luna"])
        );
        assert_eq!(
            inject_codex_session_args(&immediate, CodexSessionMode::Resume("thread-id")),
            args(&[
                "exec",
                "resume",
                "--json",
                "thread-id",
                "task",
                "--model",
                "gpt-luna"
            ])
        );

        let separated: Vec<OsString> = args(&["exec", "--model", "gpt-luna", "--", "task"]);
        assert_eq!(
            inject_codex_session_args(&separated, CodexSessionMode::Resume("thread-id")),
            args(&[
                "exec",
                "resume",
                "--json",
                "--model",
                "gpt-luna",
                "--",
                "thread-id",
                "task"
            ])
        );

        let caller_json: Vec<OsString> = args(&["exec", "task", "--json"]);
        assert_eq!(
            inject_codex_session_args(&caller_json, CodexSessionMode::Fresh),
            caller_json
        );
        assert_eq!(
            inject_codex_session_args(&caller_json, CodexSessionMode::Resume("thread-id")),
            args(&["exec", "resume", "thread-id", "task", "--json"])
        );

        let fresh_only_options: Vec<OsString> = args(&[
            "exec",
            "task",
            "--sandbox",
            "read-only",
            "--profile=isolated",
            "--cd",
            "/workspace",
            "--model",
            "gpt-luna",
        ]);
        assert_eq!(
            inject_codex_session_args(&fresh_only_options, CodexSessionMode::Resume("thread-id")),
            args(&[
                "exec",
                "resume",
                "--json",
                "thread-id",
                "task",
                "--model",
                "gpt-luna"
            ])
        );

        let json_as_task: Vec<OsString> = args(&["exec", "--", "--json"]);
        assert_eq!(
            inject_codex_session_args(&json_as_task, CodexSessionMode::Fresh),
            args(&["exec", "--json", "--", "--json"])
        );
    }

    #[test]
    fn codex_workstream_validation_rejects_ambiguous_or_unresumable_forms() {
        assert!(matches!(
            validate_managed_continuity_input(
                ChildKind::Codex,
                &args(&["exec", "--model", "gpt-luna", "task"]),
                &[]
            ),
            Err(ChildAdapterError::CodexPromptPlacementAmbiguous)
        ));
        assert!(matches!(
            validate_managed_continuity_input(
                ChildKind::Codex,
                &args(&["exec", "task", "--ephemeral"]),
                &[]
            ),
            Err(ChildAdapterError::CodexWorkstreamOptionUnsupported(
                "--ephemeral"
            ))
        ));
        assert!(
            validate_managed_continuity_input(
                ChildKind::Codex,
                &args(&["exec", "--model", "gpt-luna"]),
                b"task on stdin"
            )
            .is_ok()
        );
    }

    #[test]
    fn command_profile_hash_excludes_opt_value_and_opt_equals_value_forms_alike() {
        let no_flag: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&["-p", "task", "--model", "haiku"]),
        });
        let space_form: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&["-p", "task", "--model", "haiku", "--output-format", "json"]),
        });
        let equals_form: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Claude,
            program: OsStr::new("claude"),
            working_directory: Path::new("/workspace"),
            args: &args(&["-p", "task", "--model", "haiku", "--output-format=json"]),
        });
        assert_eq!(no_flag, space_form);
        assert_eq!(no_flag, equals_form);
    }

    #[test]
    fn projects_only_the_task_from_common_provider_commands() {
        let claude: Vec<u8> = project_task_request(
            ChildKind::Claude,
            &args(&[
                "-p",
                "review the current diff",
                "--model",
                "haiku",
                "--no-session-persistence",
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

        let opencode: Vec<u8> = project_task_request(
            ChildKind::OpenCode,
            &args(&[
                "run",
                "inspect the current patch",
                "--model",
                "opencode/big-pickle",
            ]),
            &[],
        );
        assert_eq!(opencode, b"inspect the current patch");
    }

    #[test]
    fn opencode_task_placement_is_deliberately_unambiguous() {
        let immediate: Vec<OsString> = args(&["run", "quoted task", "--model", "model/name"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::OpenCode, &immediate, &[]),
            Ok(())
        );

        let after_options: Vec<OsString> = args(&["run", "--model", "model/name", "task"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::OpenCode, &after_options, &[]),
            Err(ChildAdapterError::OpenCodePromptPlacementAmbiguous)
        );

        let multiple_tokens: Vec<OsString> = args(&["run", "task", "split", "into", "tokens"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::OpenCode, &multiple_tokens, &[]),
            Err(ChildAdapterError::OpenCodePromptPlacementAmbiguous)
        );

        let separated: Vec<OsString> =
            args(&["run", "--model", "model/name", "--", "one quoted task"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::OpenCode, &separated, &[]),
            Ok(())
        );
        assert_eq!(
            project_task_request(ChildKind::OpenCode, &separated, &[]),
            b"one quoted task"
        );
    }

    #[test]
    fn wrapper_injected_opencode_args_preserve_task_and_exact_resume() {
        let caller: Vec<OsString> = args(&["run", "review this", "--model", "opencode/big-pickle"]);
        assert_eq!(
            inject_opencode_session_args(&caller, OpenCodeSessionMode::Fresh),
            args(&[
                "run",
                "--format",
                "json",
                "review this",
                "--model",
                "opencode/big-pickle",
            ])
        );
        assert_eq!(
            inject_opencode_session_args(&caller, OpenCodeSessionMode::Resume("ses_exact-child")),
            args(&[
                "run",
                "--format",
                "json",
                "--session",
                "ses_exact-child",
                "review this",
                "--model",
                "opencode/big-pickle",
            ])
        );

        let caller_json: Vec<OsString> = args(&["run", "task", "--format=json"]);
        assert_eq!(
            inject_opencode_session_args(&caller_json, OpenCodeSessionMode::Fresh),
            caller_json
        );
        assert_eq!(
            validate_managed_continuity_input(
                ChildKind::OpenCode,
                &args(&["run", "task", "--format", "default"]),
                &[],
            ),
            Err(ChildAdapterError::OpenCodeOptionUnsupported(
                "--format other than json"
            ))
        );
    }

    #[test]
    fn projects_claude_prompt_before_variadic_tool_options() {
        let claude: Vec<u8> = project_task_request(
            ChildKind::Claude,
            &args(&[
                "-p",
                "review the current diff",
                "--model",
                "haiku",
                "--tools",
                "Read,Bash",
                "--allowedTools",
                "Read",
                "Bash(rg *)",
                "--disallowedTools",
                "Edit",
                "Write",
            ]),
            &[],
        );
        assert_eq!(claude, b"review the current diff");
    }

    #[test]
    fn rejects_ambiguous_claude_prompt_after_provider_options() {
        let arguments: Vec<OsString> = args(&[
            "-p",
            "--model",
            "haiku",
            "--allowedTools",
            "Read",
            "review the current diff",
        ]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Claude, &arguments, &[]),
            Err(ChildAdapterError::ClaudePromptPlacementAmbiguous)
        );
        assert_eq!(
            project_task_request(ChildKind::Claude, &arguments, &[]),
            b"[request text unavailable; exact child command retained only as a digest]"
        );

        let alias_arguments: Vec<OsString> =
            args(&["-p", "--allowed-tools", "Read", "review the current diff"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Claude, &alias_arguments, &[]),
            Err(ChildAdapterError::ClaudePromptPlacementAmbiguous)
        );

        let future_option_arguments: Vec<OsString> = args(&[
            "-p",
            "--future-variadic-option",
            "value",
            "review the current diff",
        ]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Claude, &future_option_arguments, &[]),
            Err(ChildAdapterError::ClaudePromptPlacementAmbiguous)
        );
    }

    #[test]
    fn caller_stdin_is_unambiguous_with_claude_provider_options() {
        let arguments: Vec<OsString> = args(&["-p", "--model", "haiku", "--allowedTools", "Read"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Claude, &arguments, b"review the current diff"),
            Ok(())
        );
        assert_eq!(
            project_task_request(ChildKind::Claude, &arguments, b"review the current diff"),
            b"review the current diff"
        );
    }

    #[test]
    fn explicit_separator_makes_trailing_claude_prompt_unambiguous() {
        let arguments: Vec<OsString> = args(&[
            "-p",
            "--allowedTools",
            "Read",
            "--",
            "review the current diff",
        ]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Claude, &arguments, &[]),
            Ok(())
        );
        assert_eq!(
            project_task_request(ChildKind::Claude, &arguments, &[]),
            b"review the current diff"
        );
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

    #[test]
    fn recognizes_antigravity_aliases_and_rejects_unsafe_native_modes() {
        for program in ["agy", "/usr/local/bin/antigravity"] {
            let kind: ChildKind = recognize_managed_child(
                OsStr::new(program),
                &args(&["-p", "review this", "--model", "gemini-3.8-flash-high"]),
            )
            .unwrap();
            assert_eq!(kind, ChildKind::Antigravity);
        }
        for unsupported in ["--conversation", "--continue", "-c", "-i"] {
            let arguments: Vec<OsString> = args(&["-p", "review this", unsupported, "native-id"]);
            assert!(matches!(
                recognize_managed_child(OsStr::new("agy"), &arguments),
                Err(ChildAdapterError::AntigravityOptionUnsupported(_))
            ));
        }
        assert!(matches!(
            recognize_managed_child(
                OsStr::new("agy"),
                &args(&["-p", "review", "--output-format", "json"])
            ),
            Err(ChildAdapterError::AntigravityOptionUnsupported(_))
        ));
    }

    #[test]
    fn antigravity_requires_prompt_adjacency_and_utf8() {
        let misplaced: Vec<OsString> = args(&["-p", "--model", "gemini-3.8-flash-high", "review"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Antigravity, &misplaced, &[]),
            Err(ChildAdapterError::AntigravityPromptPlacementAmbiguous)
        );
        assert_eq!(
            validate_managed_task_input(ChildKind::Antigravity, &misplaced, b"piped task"),
            Err(ChildAdapterError::AntigravityPromptPlacementAmbiguous)
        );
        let valid: Vec<OsString> =
            args(&["--prompt", "review", "--model", "gemini-3.8-flash-high"]);
        assert_eq!(
            validate_managed_task_input(ChildKind::Antigravity, &valid, b"extra"),
            Ok(())
        );
        assert_eq!(
            project_task_request(ChildKind::Antigravity, &valid, &[]),
            b"review"
        );
    }

    #[test]
    fn antigravity_transport_rewrites_task_and_injects_exact_resume() {
        let caller: Vec<OsString> = args(&[
            "-p",
            "review this",
            "--model",
            "gemini-3.8-flash-high",
            "--output-format",
            "stream-json",
        ]);
        assert_eq!(
            inject_antigravity_session_args(&caller, AntigravitySessionMode::Fresh),
            args(&[
                "--model",
                "gemini-3.8-flash-high",
                "--print=",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
            ])
        );
        assert_eq!(
            inject_antigravity_session_args(
                &caller,
                AntigravitySessionMode::Resume("0222067a-9e42-4b76-9649-66b84fd6bb26")
            ),
            args(&[
                "--model",
                "gemini-3.8-flash-high",
                "--print=",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--conversation",
                "0222067a-9e42-4b76-9649-66b84fd6bb26",
            ])
        );
    }

    #[test]
    fn antigravity_profile_ignores_task_and_transport_but_keeps_model() {
        let workspace: &Path = Path::new("/workspace");
        let first: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Antigravity,
            program: OsStr::new("agy"),
            working_directory: workspace,
            args: &args(&["-p", "first", "--model", "gemini-3.8-flash-high"]),
        });
        let second: ProfileHash = command_profile_hash(&CommandProfile {
            child_kind: ChildKind::Antigravity,
            program: OsStr::new("agy"),
            working_directory: workspace,
            args: &args(&[
                "--print",
                "second",
                "--model",
                "gemini-3.8-flash-high",
                "--output-format",
                "stream-json",
            ]),
        });
        assert_eq!(first, second);
    }
}
