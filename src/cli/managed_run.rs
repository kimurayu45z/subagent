//! End-to-end managed invocation: ledger, context capsule, child process,
//! redaction, and durable response completion.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use super::capsule::{self, Capsule, CapsuleRequest, InheritedHistory};
use super::child::{self, ClaudeSessionMode, CommandProfile, ProfileHash};
use super::history::{self, SupervisorHistory};
use super::id::SubagentId;
use super::process::{self, ChildExit, ChildOutcome, ChildRunRequest};
use super::redaction::{self, RedactionResult};
use super::run_cmd::{
    ContextDelivery, ContextMode, ContextScope, NativeContinuity, SummarizerChoice,
};
use super::state_dir;
use super::store::{
    self, BegunInvocation, ChildKind, ChildSessionRecord, ChildSessionRetirement,
    ChildSessionStatus, ExchangeBody, ExitOutcome,
};
use super::summarizer;
use super::supervisor::SupervisorRef;
use super::workstream::WorkstreamId;
use super::wrapper_error_exit;

const DEFAULT_MAX_CONTEXT_BYTES: u64 = 256 * 1024;
const MIN_MAX_CONTEXT_BYTES: u64 = 4 * 1024;
const MAX_MAX_CONTEXT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORDED_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORDED_RESPONSE_BYTES: usize = 1024 * 1024;
const STALE_PENDING_AFTER_SECONDS: i64 = 24 * 60 * 60;

pub(crate) struct ManagedRunRequest<'a> {
    pub program: &'a OsStr,
    pub args: &'a [OsString],
    pub cwd: &'a Path,
    pub caller_stdin: &'a [u8],
    pub state_dir_override: Option<&'a OsStr>,
    pub pair: Option<&'a store::EnsuredPair>,
    pub subagent_id: Option<&'a SubagentId>,
    pub supervisor: Option<&'a SupervisorRef>,
    pub context_scope: ContextScope,
    pub context_mode: ContextMode,
    pub context_delivery: ContextDelivery,
    pub summarizer: &'a SummarizerChoice,
    pub summarize_above_bytes: u64,
    pub max_context_bytes: Option<u64>,
    pub workstream: Option<&'a WorkstreamId>,
    pub native_continuity: NativeContinuity,
    pub no_record: bool,
    pub quiet: bool,
    pub forward_signals: bool,
}

pub(crate) fn execute(
    request: ManagedRunRequest<'_>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    if request.caller_stdin.len() > MAX_STDIN_BYTES {
        let _ = writeln!(
            err,
            "subagent: caller stdin is too large ({} bytes; maximum {})",
            request.caller_stdin.len(),
            MAX_STDIN_BYTES
        );
        return wrapper_error_exit();
    }

    let passthrough: bool = request.no_record && request.context_scope == ContextScope::None;
    let child_kind: Option<ChildKind> =
        match child::recognize_managed_child(request.program, request.args) {
            Ok(kind) => Some(kind),
            Err(_adapter_error) if passthrough => None,
            Err(adapter_error) => {
                let _ = writeln!(err, "subagent: {adapter_error}");
                return wrapper_error_exit();
            }
        };
    if !passthrough
        && let Some(kind) = child_kind
        && let Err(adapter_error) =
            child::validate_managed_task_input(kind, request.args, request.caller_stdin)
    {
        let _ = writeln!(err, "subagent: {adapter_error}");
        return wrapper_error_exit();
    }

    let max_context_bytes: u64 = request
        .max_context_bytes
        .unwrap_or(DEFAULT_MAX_CONTEXT_BYTES);
    if !(MIN_MAX_CONTEXT_BYTES..=MAX_MAX_CONTEXT_BYTES).contains(&max_context_bytes) {
        let _ = writeln!(
            err,
            "subagent: --max-context-bytes must be between {MIN_MAX_CONTEXT_BYTES} and {MAX_MAX_CONTEXT_BYTES}"
        );
        return wrapper_error_exit();
    }

    let supervisor_history: SupervisorHistory = if matches!(
        request.context_scope,
        ContextScope::Supervisor | ContextScope::All
    ) {
        match request.supervisor {
            Some(supervisor) => history::read_supervisor_history(supervisor, request.cwd),
            None => SupervisorHistory::Unavailable {
                adapter: "none",
                reason_kind: "missing_supervisor",
                reason: "supervisor history was requested without a resolved supervisor"
                    .to_string(),
            },
        }
    } else {
        SupervisorHistory::NotRequested
    };
    if request.context_scope == ContextScope::Supervisor
        && request.context_mode == ContextMode::Required
        && !supervisor_history.is_available()
    {
        let reason: &str = supervisor_history
            .reason()
            .unwrap_or("supervisor history is unavailable");
        let _ = writeln!(
            err,
            "subagent: required supervisor history is unavailable: {reason}"
        );
        return wrapper_error_exit();
    }
    if matches!(
        request.context_scope,
        ContextScope::Supervisor | ContextScope::All
    ) && !supervisor_history.is_available()
        && !request.quiet
        && let Some(reason) = supervisor_history.reason()
    {
        let _ = writeln!(
            err,
            "subagent: warning: supervisor history unavailable: {reason}"
        );
    }

    if request.no_record {
        return execute_unrecorded(
            request,
            child_kind,
            max_context_bytes,
            &supervisor_history,
            out,
            err,
        );
    }

    let Some(pair) = request.pair else {
        let _ = writeln!(
            err,
            "subagent: recording requires conversation memory and a pair identity"
        );
        return wrapper_error_exit();
    };
    if request.subagent_id.is_none() {
        let _ = writeln!(err, "subagent: recording requires a subagent id");
        return wrapper_error_exit();
    }
    let Some(kind) = child_kind else {
        let _ = writeln!(
            err,
            "subagent: internal error: a recorded child has no managed kind"
        );
        return wrapper_error_exit();
    };
    if request.workstream.is_some() && kind != ChildKind::Claude {
        let _ = writeln!(
            err,
            "subagent: managed native continuity currently supports only Claude Code"
        );
        return wrapper_error_exit();
    }

    let state_root: PathBuf = match state_dir::resolve_state_root(request.state_dir_override) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(err, "subagent: {error}");
            return wrapper_error_exit();
        }
    };
    let mut ledger: store::Store = match store::Store::open_for_write(&state_root) {
        Ok(store) => store,
        Err(error) => {
            let _ = writeln!(err, "subagent: failed to open invocation ledger: {error}");
            return wrapper_error_exit();
        }
    };

    let profile_hash: Option<ProfileHash> = request.workstream.map(|_workstream: &WorkstreamId| {
        child::command_profile_hash(&CommandProfile {
            child_kind: kind,
            program: request.program,
            working_directory: &pair.workspace,
            args: request.args,
        })
    });
    let resolved_resume: Option<ChildSessionRecord> =
        if request.native_continuity == NativeContinuity::Resume {
            let workstream: &WorkstreamId = request
                .workstream
                .expect("resume continuity is validated with a workstream");
            let profile_hash: ProfileHash =
                profile_hash.expect("tracked continuity always computes a profile hash");
            match resolve_resume_session(&ledger, &pair.pair_key, kind, workstream, profile_hash) {
                Ok(session) => Some(session),
                Err(message) => {
                    let _ = writeln!(err, "subagent: {message}");
                    return wrapper_error_exit();
                }
            }
        } else {
            None
        };

    let cutoff: i64 = unix_now().saturating_sub(STALE_PENDING_AFTER_SECONDS);
    if let Err(error) = ledger.abandon_stale_pending_invocations(&pair.pair_key, cutoff) {
        let _ = writeln!(
            err,
            "subagent: failed to recover stale invocations: {error}"
        );
        return wrapper_error_exit();
    }

    let recorded_request: ExchangeBody = make_recorded_request(kind, &request);
    let provenance: String = context_provenance(
        request.context_scope,
        request.context_delivery,
        request.native_continuity,
        request.workstream,
        &supervisor_history,
    );
    let program_name: String = std::path::Path::new(request.program)
        .file_name()
        .unwrap_or(request.program)
        .to_string_lossy()
        .into_owned();
    let begun: BegunInvocation = match ledger.begin_invocation(
        &pair.pair_key,
        std::process::id(),
        child::command_digest(request.program, request.args),
        &program_name,
        kind,
        &provenance,
        recorded_request,
    ) {
        Ok(begun) => begun,
        Err(error) => {
            let _ = writeln!(err, "subagent: failed to begin invocation: {error}");
            return wrapper_error_exit();
        }
    };

    let native_session: Option<ChildSessionRecord> = match request.native_continuity {
        NativeContinuity::Untracked => None,
        NativeContinuity::Fresh => {
            let workstream: &WorkstreamId = request
                .workstream
                .expect("fresh continuity is validated with a workstream");
            let profile_hash: ProfileHash =
                profile_hash.expect("tracked continuity always computes a profile hash");
            let native_id: String = Uuid::now_v7().to_string();
            match ledger.assign_fresh_child_session(
                &pair.pair_key,
                kind,
                workstream.as_str(),
                profile_hash.as_bytes(),
                &native_id,
            ) {
                Ok(session) => Some(session),
                Err(error) => {
                    let _ = ledger.mark_spawn_failed(&begun.invocation_id);
                    let _ = writeln!(
                        err,
                        "subagent: failed to assign fresh child session: {error}"
                    );
                    return wrapper_error_exit();
                }
            }
        }
        NativeContinuity::Resume => {
            Some(resolved_resume.expect("resume resolution succeeds before invocation allocation"))
        }
    };
    if let Some(session) = &native_session
        && let Err(error) =
            ledger.link_invocation_child_session(&begun.invocation_id, &session.native_id)
    {
        let _ = ledger.mark_spawn_failed(&begun.invocation_id);
        let _ = writeln!(err, "subagent: failed to link child session: {error}");
        return wrapper_error_exit();
    }

    let capsule: Option<Capsule> = match prepare_capsule(
        &request,
        &state_root,
        &begun.invocation_id,
        begun.sequence,
        &ledger,
        max_context_bytes,
        &supervisor_history,
        err,
    ) {
        Ok(capsule) => capsule,
        Err(message) => {
            let _ = ledger.mark_spawn_failed(&begun.invocation_id);
            let _ = writeln!(err, "subagent: {message}");
            return wrapper_error_exit();
        }
    };
    if let Some(capsule) = &capsule
        && let Err(error) = ledger.attach_capsule(
            &begun.invocation_id,
            &capsule.manifest_path,
            capsule.capsule_digest,
        )
    {
        let _ = capsule::remove_capsule(&state_root, &begun.invocation_id);
        let _ = ledger.mark_spawn_failed(&begun.invocation_id);
        let _ = writeln!(err, "subagent: failed to attach context capsule: {error}");
        return wrapper_error_exit();
    }

    let stdin_bytes: Vec<u8> = prepare_child_stdin(capsule.as_ref(), request.caller_stdin);
    let spawn_args_storage: Option<Vec<OsString>> =
        native_session.as_ref().map(|session: &ChildSessionRecord| {
            let mode: ClaudeSessionMode = match request.native_continuity {
                NativeContinuity::Fresh => ClaudeSessionMode::Assign,
                NativeContinuity::Resume => ClaudeSessionMode::Resume,
                NativeContinuity::Untracked => {
                    unreachable!("an untracked invocation has no native session")
                }
            };
            child::inject_claude_session_args(request.args, mode, &session.native_id)
        });
    let spawn_args: &[OsString] = spawn_args_storage.as_deref().unwrap_or(request.args);
    let outcome: ChildOutcome = match process::run_child(
        ChildRunRequest {
            program: request.program,
            args: spawn_args,
            cwd: request.cwd,
            stdin_bytes,
            env_overrides: Vec::new(),
            max_capture_bytes: MAX_RECORDED_RESPONSE_BYTES,
            forward_signals: request.forward_signals,
            timeout: None,
        },
        out,
        err,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            if request.native_continuity == NativeContinuity::Fresh
                && let Some(session) = &native_session
            {
                let _ = ledger.retire_child_session(
                    &pair.pair_key,
                    &session.native_id,
                    ChildSessionRetirement::ProviderRejected,
                );
            }
            let _ = ledger.mark_spawn_failed(&begun.invocation_id);
            let _ = writeln!(err, "subagent: {error}");
            return wrapper_error_exit();
        }
    };

    report_forwarding_errors(&outcome, err);
    let response_redaction: RedactionResult =
        redaction::redact(&outcome.stdout_capture, MAX_RECORDED_RESPONSE_BYTES);
    let response: ExchangeBody = ExchangeBody {
        body: response_redaction.redacted_bytes,
        truncated: outcome.stdout_truncated || response_redaction.truncated,
        redaction_count: response_redaction.redaction_count,
        redaction_classes: response_redaction.redaction_classes,
    };
    let exit_outcome: ExitOutcome = store_exit(outcome.exit);
    if child_exit_succeeded(outcome.exit)
        && let Some(session) = &native_session
        && let Err(error) = ledger.mark_child_session_active(&pair.pair_key, &session.native_id)
    {
        let _ = writeln!(
            err,
            "subagent: warning: child succeeded but its native session could not be activated: {error}"
        );
    }
    if let Err(error) = ledger.complete_invocation(&begun.invocation_id, exit_outcome, response) {
        let _ = writeln!(
            err,
            "subagent: warning: child finished but its response could not be recorded: {error}"
        );
    }
    reproduce_signal_if_needed(outcome.exit, request.forward_signals);
    child_exit_code(outcome.exit)
}

fn execute_unrecorded(
    request: ManagedRunRequest<'_>,
    _child_kind: Option<ChildKind>,
    max_context_bytes: u64,
    supervisor_history: &SupervisorHistory,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    let mut capsule: Option<Capsule> = None;
    let mut capsule_identity: Option<(PathBuf, String)> = None;
    if request.context_scope != ContextScope::None {
        let Some(pair) = request.pair else {
            let _ = writeln!(
                err,
                "subagent: context requires conversation memory and a pair identity"
            );
            return wrapper_error_exit();
        };
        if request.subagent_id.is_none() {
            let _ = writeln!(err, "subagent: context requires a subagent id");
            return wrapper_error_exit();
        }
        let state_root: PathBuf = match state_dir::resolve_state_root(request.state_dir_override) {
            Ok(path) => path,
            Err(error) => {
                let _ = writeln!(err, "subagent: {error}");
                return wrapper_error_exit();
            }
        };
        let ledger: store::Store = match store::Store::open_for_write(&state_root) {
            Ok(store) => store,
            Err(error) => {
                let _ = writeln!(err, "subagent: failed to open invocation ledger: {error}");
                return wrapper_error_exit();
            }
        };
        let invocation_id: String = Uuid::now_v7().to_string();
        capsule = match prepare_capsule(
            &request,
            &state_root,
            &invocation_id,
            i64::MAX,
            &ledger,
            max_context_bytes,
            supervisor_history,
            err,
        ) {
            Ok(value) => value,
            Err(message) => {
                let _ = writeln!(err, "subagent: {message}");
                return wrapper_error_exit();
            }
        };
        capsule_identity = Some((state_root, invocation_id));
        let _ = pair;
    }

    let stdin_bytes: Vec<u8> = prepare_child_stdin(capsule.as_ref(), request.caller_stdin);
    let result: Result<ChildOutcome, process::ChildProcessError> = process::run_child(
        ChildRunRequest {
            program: request.program,
            args: request.args,
            cwd: request.cwd,
            stdin_bytes,
            env_overrides: Vec::new(),
            max_capture_bytes: 0,
            forward_signals: request.forward_signals,
            timeout: None,
        },
        out,
        err,
    );
    if let Some((state_root, invocation_id)) = capsule_identity
        && let Err(error) = capsule::remove_capsule(&state_root, &invocation_id)
    {
        let _ = writeln!(
            err,
            "subagent: warning: failed to remove temporary context capsule: {error}"
        );
    }
    match result {
        Ok(outcome) => {
            report_forwarding_errors(&outcome, err);
            child_exit_code(outcome.exit)
        }
        Err(error) => {
            let _ = writeln!(err, "subagent: {error}");
            wrapper_error_exit()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_capsule(
    request: &ManagedRunRequest<'_>,
    state_root: &Path,
    invocation_id: &str,
    sequence: i64,
    ledger: &store::Store,
    max_context_bytes: u64,
    supervisor_history: &SupervisorHistory,
    err: &mut dyn Write,
) -> Result<Option<Capsule>, String> {
    if request.context_scope == ContextScope::None {
        return Ok(None);
    }
    let pair: &store::EnsuredPair = request
        .pair
        .ok_or_else(|| "context requires a pair identity".to_string())?;
    let subagent_id: &SubagentId = request
        .subagent_id
        .ok_or_else(|| "context requires a subagent id".to_string())?;
    let history: Vec<store::CompletedExchange> = ledger
        .list_completed_exchanges(&pair.pair_key, Some(sequence))
        .map_err(|error| format!("failed to read pair history: {error}"))?;
    let inherited_history: Option<InheritedHistory> = ledger
        .inheritance_for(&pair.pair_key)
        .map_err(|error| format!("failed to resolve inherited pair history: {error}"))?
        .map(|edge: store::InheritanceEdge| {
            let completed_exchanges: Vec<store::CompletedExchange> = ledger
                .list_completed_exchanges(&edge.source_pair_key, None)
                .map_err(|error| format!("failed to read inherited pair history: {error}"))?;
            Ok::<InheritedHistory, String>(InheritedHistory {
                source_pair_key: edge.source_pair_key,
                source_subagent_id: edge.source_subagent_id,
                completed_exchanges,
            })
        })
        .transpose()?;
    let inherited_for_model: Option<(&str, &[store::CompletedExchange])> = inherited_history
        .as_ref()
        .map(|inherited: &InheritedHistory| {
            (
                inherited.source_subagent_id.as_str(),
                inherited.completed_exchanges.as_slice(),
            )
        });
    let inherited_records: Option<&[store::CompletedExchange]> =
        inherited_for_model.map(|(_source_id, records)| records);
    let source_bytes: u64 = summarizer::history_source_bytes(&history, inherited_records);
    let model_summary: Option<summarizer::ModelSummary> = if let SummarizerChoice::Alias(alias) =
        request.summarizer
        && !request.no_record
        && source_bytes >= request.summarize_above_bytes
        && source_bytes > 0
    {
        match summarizer::summarize(alias, &history, inherited_for_model, state_root) {
            Ok(summary) => Some(summary),
            Err(error) => {
                if !request.quiet {
                    let _ = writeln!(
                        err,
                        "subagent: warning: model summarizer failed; using deterministic summary: {error}"
                    );
                }
                None
            }
        }
    } else {
        None
    };
    let include_summary_snippets: bool =
        !matches!(request.summarizer, SummarizerChoice::None) && model_summary.is_none();
    let capsule: Capsule = capsule::create_capsule(
        state_root,
        CapsuleRequest {
            invocation_id,
            pair_key: pair.pair_key,
            sequence,
            workspace: &pair.workspace,
            subagent_id,
            supervisor_provider: request
                .supervisor
                .map(|supervisor: &SupervisorRef| supervisor.provider)
                .ok_or_else(|| "context requires a resolved supervisor".to_string())?,
            context_scope: request.context_scope,
            context_delivery: request.context_delivery,
            include_summary_snippets,
            max_context_bytes,
            completed_exchanges: history,
            supervisor_history: supervisor_history.clone(),
            inherited_history,
            model_summary,
        },
    )
    .map_err(|error| format!("failed to create context capsule: {error}"))?;
    Ok(Some(capsule))
}

fn make_recorded_request(kind: ChildKind, request: &ManagedRunRequest<'_>) -> ExchangeBody {
    let projected: Vec<u8> = child::project_task_request(kind, request.args, request.caller_stdin);
    redaction_to_exchange(redaction::redact(&projected, MAX_RECORDED_REQUEST_BYTES))
}

fn redaction_to_exchange(result: RedactionResult) -> ExchangeBody {
    ExchangeBody {
        body: result.redacted_bytes,
        truncated: result.truncated,
        redaction_count: result.redaction_count,
        redaction_classes: result.redaction_classes,
    }
}

fn prepare_child_stdin(capsule: Option<&Capsule>, caller_stdin: &[u8]) -> Vec<u8> {
    let Some(capsule) = capsule else {
        return caller_stdin.to_vec();
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(
        capsule
            .bootstrap_text
            .len()
            .saturating_add(caller_stdin.len())
            .saturating_add(128),
    );
    bytes.extend_from_slice(capsule.bootstrap_text.as_bytes());
    bytes.extend_from_slice(
        b"\n\n--- END SUBAGENT CONTEXT; CURRENT REQUEST CONTINUES IN THE COMMAND PROMPT ---\n",
    );
    if !caller_stdin.is_empty() {
        bytes.extend_from_slice(b"\n--- BEGIN CALLER STDIN ---\n");
        bytes.extend_from_slice(caller_stdin);
        bytes.extend_from_slice(b"\n--- END CALLER STDIN ---\n");
    }
    bytes.extend_from_slice(
        b"\nThe positional command prompt and any CALLER STDIN above are the current authoritative request. Execute that request now. Do not answer this context bootstrap as a separate request.\n",
    );
    bytes
}

fn context_provenance(
    scope: ContextScope,
    delivery: ContextDelivery,
    native_continuity: NativeContinuity,
    workstream: Option<&WorkstreamId>,
    supervisor_history: &SupervisorHistory,
) -> String {
    let pair: &str = if matches!(scope, ContextScope::Pair | ContextScope::All) {
        "included"
    } else {
        "not_requested"
    };
    let supervisor: &str = match supervisor_history {
        SupervisorHistory::Available { .. } => "included",
        SupervisorHistory::Unavailable { .. } => "unavailable",
        SupervisorHistory::NotRequested => "not_requested",
    };
    let workstream_json: String = workstream
        .map(|id: &WorkstreamId| format!("\"{}\"", id.as_str()))
        .unwrap_or_else(|| "null".to_string());
    format!(
        "{{\"pair\":\"{pair}\",\"supervisor\":\"{supervisor}\",\"delivery\":\"{delivery}\",\"native_continuity\":\"{native_continuity}\",\"workstream\":{workstream_json}}}"
    )
}

pub(crate) fn resolve_resume_session(
    ledger: &store::Store,
    pair_key: &super::pair_key::PairKey,
    child_kind: ChildKind,
    workstream: &WorkstreamId,
    profile_hash: ProfileHash,
) -> Result<ChildSessionRecord, String> {
    let session: ChildSessionRecord = ledger
        .live_child_session_for_workstream(pair_key, child_kind, workstream.as_str())
        .map_err(|error| format!("failed to resolve workstream session: {error}"))?
        .ok_or_else(|| {
            format!(
                "no live native session exists for workstream {:?}; start it with --fresh",
                workstream.as_str()
            )
        })?;
    if session.status != ChildSessionStatus::Active {
        return Err(format!(
            "workstream {:?} was assigned but never confirmed by a successful child exit; restart it with --fresh",
            workstream.as_str()
        ));
    }
    if session.profile_schema_version != i64::from(child::COMMAND_PROFILE_SCHEMA_VERSION) {
        return Err(format!(
            "workstream {:?} uses command profile schema {}, but this build requires {}; restart it with --fresh",
            workstream.as_str(),
            session.profile_schema_version,
            child::COMMAND_PROFILE_SCHEMA_VERSION
        ));
    }
    if session.profile_hash != *profile_hash.as_bytes() {
        return Err(format!(
            "workstream {:?} is bound to command profile {}, but this invocation uses {}; restart it with --fresh",
            workstream.as_str(),
            ProfileHash::from_bytes(session.profile_hash).to_hex(),
            profile_hash.to_hex()
        ));
    }
    Ok(session)
}

fn child_exit_succeeded(exit: ChildExit) -> bool {
    matches!(exit, ChildExit::Exited(0))
}

fn report_forwarding_errors(outcome: &ChildOutcome, err: &mut dyn Write) {
    let unique: BTreeSet<&str> = outcome
        .forwarding_errors
        .iter()
        .map(String::as_str)
        .collect();
    for message in unique.into_iter().take(8) {
        let _ = writeln!(err, "subagent: warning: {message}");
    }
}

fn store_exit(exit: ChildExit) -> ExitOutcome {
    match exit {
        ChildExit::Exited(code) => ExitOutcome::Exited { code },
        #[cfg(unix)]
        ChildExit::Signaled(signal) => ExitOutcome::Signaled { signal },
    }
}

fn child_exit_code(exit: ChildExit) -> ExitCode {
    match exit {
        ChildExit::Exited(code) => ExitCode::from(u8::try_from(code).unwrap_or(125)),
        #[cfg(unix)]
        ChildExit::Signaled(signal) => {
            let conventional: i32 = 128_i32.saturating_add(signal);
            ExitCode::from(u8::try_from(conventional).unwrap_or(125))
        }
    }
}

#[cfg(unix)]
fn reproduce_signal_if_needed(exit: ChildExit, enabled: bool) {
    if !enabled {
        return;
    }
    if let ChildExit::Signaled(signal) = exit {
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }
}

#[cfg(not(unix))]
fn reproduce_signal_if_needed(_exit: ChildExit, _enabled: bool) {}

fn unix_now() -> i64 {
    let seconds: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(seconds).unwrap_or(i64::MAX)
}
