//! Read-only supervisor conversation adapters.
//!
//! The Codex adapter uses only app-server's `initialize` and `thread/read`
//! methods. The Antigravity adapter reads one explicit, workspace-validated
//! CLI transcript. Both project an allowlist of visible message items and
//! treat all other item kinds as untrusted provider internals that must not
//! cross the supervisor/subordinate boundary.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use super::supervisor::{Provider, SupervisorRef};

const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STDOUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_ANTIGRAVITY_CACHE_BYTES: usize = 1024 * 1024;
const MAX_ANTIGRAVITY_TRANSCRIPT_BYTES: usize = 32 * 1024 * 1024;
const MAX_ANTIGRAVITY_VISIBLE_RECORDS: usize = 4096;
const INITIALIZE_REQUEST_ID: u64 = 1;
const THREAD_READ_REQUEST_ID: u64 = 2;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HistoryRecord {
    pub source_provider: Provider,
    pub source_session_id: String,
    pub source_record_id: String,
    pub sequence: u64,
    pub timestamp: Option<i64>,
    pub role: HistoryRole,
    pub kind: HistoryKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistoryRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistoryKind {
    UserMessage,
    AgentMessage,
}

#[derive(Debug, Clone)]
pub(crate) enum SupervisorHistory {
    Available {
        adapter: &'static str,
        adapter_version: u32,
        records: Vec<HistoryRecord>,
        skipped_item_count: usize,
    },
    Unavailable {
        adapter: &'static str,
        reason_kind: &'static str,
        reason: String,
    },
    NotRequested,
}

impl SupervisorHistory {
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, SupervisorHistory::Available { .. })
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            SupervisorHistory::Unavailable { reason, .. } => Some(reason),
            SupervisorHistory::Available { .. } | SupervisorHistory::NotRequested => None,
        }
    }
}

enum ReaderEvent {
    Message(Value),
    TooLarge,
    ReadFailed(String),
    Closed,
}

pub(crate) fn read_supervisor_history(
    supervisor: &SupervisorRef,
    workspace: &Path,
) -> SupervisorHistory {
    match supervisor.provider {
        Provider::Codex => read_codex_with_program(
            OsStr::new("codex"),
            &supervisor.session_id,
            workspace,
            APP_SERVER_TIMEOUT,
        ),
        Provider::Claude => unavailable(
            "claude_transcript",
            "adapter_not_implemented",
            "the Claude Code supervisor-history adapter is not implemented yet",
        ),
        Provider::OpenCode => unavailable(
            "opencode_transcript",
            "adapter_not_implemented",
            "the OpenCode supervisor-history adapter is not implemented yet",
        ),
        Provider::Antigravity => {
            read_antigravity_from_default_home(&supervisor.session_id, workspace)
        }
    }
}

fn read_antigravity_from_default_home(
    conversation_id: &str,
    workspace: &Path,
) -> SupervisorHistory {
    let Some(base_dirs) = directories::BaseDirs::new() else {
        return unavailable(
            "antigravity_transcript",
            "home_unavailable",
            "could not resolve the user home directory for Antigravity CLI state",
        );
    };
    let cli_state_root: PathBuf = base_dirs.home_dir().join(".gemini").join("antigravity-cli");
    read_antigravity_from_state_root(&cli_state_root, conversation_id, workspace)
}

fn read_antigravity_from_state_root(
    cli_state_root: &Path,
    conversation_id: &str,
    workspace: &Path,
) -> SupervisorHistory {
    if !super::antigravity_json::is_valid_conversation_id(conversation_id) {
        return unavailable(
            "antigravity_transcript",
            "invalid_conversation_id",
            "the requested Antigravity conversation ID is not a canonical UUID",
        );
    }

    let canonical_workspace: PathBuf = match std::fs::canonicalize(workspace) {
        Ok(path) => path,
        Err(error) => {
            return unavailable(
                "antigravity_transcript",
                "workspace_unavailable",
                format!("could not canonicalize the current workspace: {error}"),
            );
        }
    };
    let cache_path: PathBuf = cli_state_root.join("cache").join("last_conversations.json");
    let cache_bytes: Vec<u8> =
        match read_bounded_regular_file(&cache_path, MAX_ANTIGRAVITY_CACHE_BYTES) {
            Ok(bytes) => bytes,
            Err(reason) => {
                return unavailable(
                    "antigravity_transcript",
                    "workspace_evidence_unavailable",
                    format!(
                        "could not read Antigravity's workspace-to-conversation cache: {reason}"
                    ),
                );
            }
        };
    let cache: serde_json::Map<String, Value> = match serde_json::from_slice::<Value>(&cache_bytes)
        .ok()
        .and_then(|value: Value| value.as_object().cloned())
    {
        Some(cache) => cache,
        None => {
            return unavailable(
                "antigravity_transcript",
                "workspace_evidence_malformed",
                "Antigravity's workspace-to-conversation cache is not a JSON object",
            );
        }
    };
    let mut workspace_conversations: BTreeSet<String> = BTreeSet::new();
    for (raw_workspace, raw_conversation) in cache {
        let Some(cached_conversation) = raw_conversation.as_str() else {
            return unavailable(
                "antigravity_transcript",
                "workspace_evidence_malformed",
                "Antigravity's workspace-to-conversation cache contains a non-string ID",
            );
        };
        let cached_workspace: PathBuf = match std::fs::canonicalize(&raw_workspace) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if cached_workspace == canonical_workspace {
            workspace_conversations.insert(cached_conversation.to_string());
        }
    }
    if workspace_conversations.len() != 1 || !workspace_conversations.contains(conversation_id) {
        return unavailable(
            "antigravity_transcript",
            "workspace_unverified",
            "the exact requested Antigravity conversation is not the cache entry for the current canonical workspace; the cache is used only to validate an explicit ID, never to select one",
        );
    }

    let canonical_root: PathBuf = match std::fs::canonicalize(cli_state_root) {
        Ok(path) => path,
        Err(error) => {
            return unavailable(
                "antigravity_transcript",
                "state_root_unavailable",
                format!("could not canonicalize Antigravity CLI state: {error}"),
            );
        }
    };
    let canonical_brain_root: PathBuf = match std::fs::canonicalize(canonical_root.join("brain")) {
        Ok(path) => path,
        Err(error) => {
            return unavailable(
                "antigravity_transcript",
                "transcript_unavailable",
                format!("could not canonicalize Antigravity's brain directory: {error}"),
            );
        }
    };
    if !canonical_brain_root.starts_with(&canonical_root) {
        return unavailable(
            "antigravity_transcript",
            "unsafe_transcript_path",
            "Antigravity's brain directory resolves outside its CLI state root",
        );
    }
    let expected_path: PathBuf = canonical_brain_root
        .join(conversation_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl");
    let canonical_transcript: PathBuf = match std::fs::canonicalize(&expected_path) {
        Ok(path) => path,
        Err(error) => {
            return unavailable(
                "antigravity_transcript",
                "transcript_unavailable",
                format!("could not locate the exact Antigravity transcript: {error}"),
            );
        }
    };
    let canonical_conversation_root: PathBuf = canonical_brain_root.join(conversation_id);
    if !canonical_transcript.starts_with(&canonical_conversation_root)
        || canonical_transcript.file_name() != Some(OsStr::new("transcript.jsonl"))
    {
        return unavailable(
            "antigravity_transcript",
            "unsafe_transcript_path",
            "the Antigravity transcript resolves outside the exact conversation directory",
        );
    }

    let transcript_bytes: Vec<u8> =
        match read_bounded_regular_file(&canonical_transcript, MAX_ANTIGRAVITY_TRANSCRIPT_BYTES) {
            Ok(bytes) => bytes,
            Err(reason) => {
                return unavailable("antigravity_transcript", "transcript_unavailable", reason);
            }
        };
    project_antigravity_transcript(&transcript_bytes, conversation_id)
}

fn read_bounded_regular_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut options: OpenOptions = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file: File = options
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let metadata: std::fs::Metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(format!(
            "{} exceeds the {max_bytes}-byte read limit",
            path.display()
        ));
    }
    let mut bytes: Vec<u8> = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(max_bytes)
            .min(max_bytes),
    );
    file.take(
        u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut bytes)
    .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "{} grew beyond the {max_bytes}-byte read limit while being read",
            path.display()
        ));
    }
    Ok(bytes)
}

fn project_antigravity_transcript(bytes: &[u8], conversation_id: &str) -> SupervisorHistory {
    let complete_bytes: &[u8] = match bytes.iter().rposition(|byte: &u8| *byte == b'\n') {
        Some(index) => &bytes[..=index],
        None if bytes.is_empty() => bytes,
        None => &[],
    };
    let mut records: VecDeque<HistoryRecord> = VecDeque::new();
    let mut skipped_item_count: usize = usize::from(complete_bytes.len() != bytes.len());
    let mut sequence: u64 = 0;
    for raw_line in complete_bytes.split(|byte: &u8| *byte == b'\n') {
        if raw_line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value: Value = match serde_json::from_slice(raw_line) {
            Ok(value) => value,
            Err(error) => {
                return unavailable(
                    "antigravity_transcript",
                    "malformed_transcript",
                    format!("Antigravity transcript contains malformed JSONL: {error}"),
                );
            }
        };
        let Some(item_type) = value.get("type").and_then(Value::as_str) else {
            skipped_item_count = skipped_item_count.saturating_add(1);
            continue;
        };
        let (expected_source, role, kind): (&str, HistoryRole, HistoryKind) = match item_type {
            "USER_INPUT" => ("USER_EXPLICIT", HistoryRole::User, HistoryKind::UserMessage),
            "PLANNER_RESPONSE" => ("MODEL", HistoryRole::Assistant, HistoryKind::AgentMessage),
            _ => {
                skipped_item_count = skipped_item_count.saturating_add(1);
                continue;
            }
        };
        let source: Option<&str> = value.get("source").and_then(Value::as_str);
        let status: Option<&str> = value.get("status").and_then(Value::as_str);
        let step_index: Option<u64> = value.get("step_index").and_then(Value::as_u64);
        let content: Option<&str> = value.get("content").and_then(Value::as_str);
        if source != Some(expected_source) || step_index.is_none() || content.is_none() {
            return unavailable(
                "antigravity_transcript",
                "malformed_transcript",
                format!("Antigravity {item_type} record is missing required typed fields"),
            );
        }
        let Some(status) = status else {
            return unavailable(
                "antigravity_transcript",
                "malformed_transcript",
                format!("Antigravity {item_type} record has no string status"),
            );
        };
        if status != "DONE" {
            skipped_item_count = skipped_item_count.saturating_add(1);
            continue;
        }
        let text: &str = content.expect("content was checked above");
        if text.is_empty() {
            skipped_item_count = skipped_item_count.saturating_add(1);
            continue;
        }
        sequence = sequence.saturating_add(1);
        records.push_back(HistoryRecord {
            source_provider: Provider::Antigravity,
            source_session_id: conversation_id.to_string(),
            source_record_id: format!("step-{}", step_index.expect("step index was checked above")),
            sequence,
            timestamp: None,
            role,
            kind,
            text: text.to_string(),
        });
        if records.len() > MAX_ANTIGRAVITY_VISIBLE_RECORDS {
            records.pop_front();
            skipped_item_count = skipped_item_count.saturating_add(1);
        }
    }

    SupervisorHistory::Available {
        adapter: "antigravity_transcript",
        adapter_version: 1,
        records: records.into_iter().collect(),
        skipped_item_count,
    }
}

fn read_codex_with_program(
    program: &OsStr,
    thread_id: &str,
    workspace: &Path,
    timeout: Duration,
) -> SupervisorHistory {
    let mut command: Command = Command::new(program);
    command
        .arg("app-server")
        .arg("--stdio")
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("SUBAGENT_SELF_REF");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child: Child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return unavailable(
                "codex_app_server",
                "spawn_failed",
                format!("failed to spawn codex app-server: {error}"),
            );
        }
    };
    let child_pid: u32 = child.id();
    let Some(mut child_stdin) = child.stdin.take() else {
        terminate(&mut child, child_pid);
        return unavailable(
            "codex_app_server",
            "protocol_error",
            "codex app-server stdin was unavailable",
        );
    };
    let Some(child_stdout) = child.stdout.take() else {
        terminate(&mut child, child_pid);
        return unavailable(
            "codex_app_server",
            "protocol_error",
            "codex app-server stdout was unavailable",
        );
    };
    let Some(child_stderr) = child.stderr.take() else {
        terminate(&mut child, child_pid);
        return unavailable(
            "codex_app_server",
            "protocol_error",
            "codex app-server stderr was unavailable",
        );
    };

    let (sender, receiver): (Sender<ReaderEvent>, Receiver<ReaderEvent>) = mpsc::channel();
    let stdout_thread: thread::JoinHandle<()> =
        spawn_protocol_reader(child_stdout, sender, MAX_STDOUT_BYTES);
    let stderr_thread: thread::JoinHandle<Vec<u8>> =
        spawn_bounded_reader(child_stderr, MAX_STDERR_BYTES);

    let requests: [Value; 3] = [
        json!({
            "id": INITIALIZE_REQUEST_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {"name": "subagent", "version": env!("CARGO_PKG_VERSION")}
            }
        }),
        json!({"method": "initialized", "params": {}}),
        json!({
            "id": THREAD_READ_REQUEST_ID,
            "method": "thread/read",
            "params": {"threadId": thread_id, "includeTurns": true}
        }),
    ];
    for request in &requests {
        if let Err(error) = serde_json::to_writer(&mut child_stdin, request)
            .and_then(|()| child_stdin.write_all(b"\n").map_err(serde_json::Error::io))
        {
            terminate(&mut child, child_pid);
            drop(child_stdin);
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return unavailable(
                "codex_app_server",
                "protocol_error",
                format!("failed to write an app-server request: {error}"),
            );
        }
    }
    if let Err(error) = child_stdin.flush() {
        terminate(&mut child, child_pid);
        drop(child_stdin);
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        return unavailable(
            "codex_app_server",
            "protocol_error",
            format!("failed to flush app-server requests: {error}"),
        );
    }

    let started: Instant = Instant::now();
    let mut result: Option<SupervisorHistory> = None;
    while result.is_none() {
        let elapsed: Duration = started.elapsed();
        let Some(remaining) = timeout.checked_sub(elapsed) else {
            result = Some(unavailable(
                "codex_app_server",
                "timeout",
                "codex app-server thread/read timed out",
            ));
            break;
        };
        match receiver.recv_timeout(remaining) {
            Ok(ReaderEvent::Message(message)) => {
                let response_id: Option<u64> = message.get("id").and_then(Value::as_u64);
                if response_id == Some(INITIALIZE_REQUEST_ID) && message.get("error").is_some() {
                    result = Some(unavailable(
                        "codex_app_server",
                        "initialize_failed",
                        rpc_error_message(&message),
                    ));
                } else if response_id == Some(THREAD_READ_REQUEST_ID) {
                    result = Some(if message.get("error").is_some() {
                        unavailable(
                            "codex_app_server",
                            "thread_read_failed",
                            rpc_error_message(&message),
                        )
                    } else {
                        project_thread_read(&message, thread_id, workspace)
                    });
                }
            }
            Ok(ReaderEvent::TooLarge) => {
                result = Some(unavailable(
                    "codex_app_server",
                    "response_too_large",
                    format!("codex app-server output exceeded {MAX_STDOUT_BYTES} bytes"),
                ));
            }
            Ok(ReaderEvent::ReadFailed(error)) => {
                result = Some(unavailable(
                    "codex_app_server",
                    "protocol_error",
                    format!("failed to read codex app-server output: {error}"),
                ));
            }
            Ok(ReaderEvent::Closed) => {
                result = Some(unavailable(
                    "codex_app_server",
                    "early_exit",
                    "codex app-server closed stdout before replying to thread/read",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
                result = Some(unavailable(
                    "codex_app_server",
                    "timeout",
                    "codex app-server thread/read timed out",
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                result = Some(unavailable(
                    "codex_app_server",
                    "early_exit",
                    "codex app-server response channel closed unexpectedly",
                ));
            }
        }
    }

    terminate(&mut child, child_pid);
    drop(child_stdin);
    let _ = child.wait();
    let _ = stdout_thread.join();
    let _stderr: Vec<u8> = stderr_thread.join().unwrap_or_default();
    result.expect("the response loop always produces a result")
}

fn project_thread_read(
    message: &Value,
    expected_thread_id: &str,
    workspace: &Path,
) -> SupervisorHistory {
    let Some(thread_value) = message.get("result").and_then(|value| value.get("thread")) else {
        return malformed("thread/read response has no result.thread object");
    };
    let Some(actual_thread_id) = thread_value.get("id").and_then(Value::as_str) else {
        return malformed("thread/read response has no string thread.id");
    };
    if actual_thread_id != expected_thread_id {
        return malformed("thread/read returned a different thread id than requested");
    }
    let Some(raw_cwd) = thread_value.get("cwd").and_then(Value::as_str) else {
        return malformed("thread/read response has no string thread.cwd");
    };
    let response_cwd: PathBuf = match std::fs::canonicalize(raw_cwd) {
        Ok(path) => path,
        Err(error) => {
            return unavailable(
                "codex_app_server",
                "workspace_unavailable",
                format!("could not canonicalize supervisor workspace: {error}"),
            );
        }
    };
    let expected_cwd: PathBuf = match std::fs::canonicalize(workspace) {
        Ok(path) => path,
        Err(error) => {
            return unavailable(
                "codex_app_server",
                "workspace_unavailable",
                format!("could not canonicalize current workspace: {error}"),
            );
        }
    };
    if response_cwd != expected_cwd {
        return unavailable(
            "codex_app_server",
            "workspace_mismatch",
            "the requested Codex thread belongs to a different canonical workspace",
        );
    }
    let Some(turns) = thread_value.get("turns").and_then(Value::as_array) else {
        return malformed("thread/read response has no turns array");
    };

    let mut records: Vec<HistoryRecord> = Vec::new();
    let mut skipped_item_count: usize = 0;
    let mut sequence: u64 = 0;
    for turn in turns {
        let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
            return malformed("thread/read contains a turn without a string id");
        };
        let timestamp_user: Option<i64> = turn.get("startedAt").and_then(Value::as_i64);
        let timestamp_agent: Option<i64> = turn
            .get("completedAt")
            .and_then(Value::as_i64)
            .or(timestamp_user);
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            return malformed("thread/read contains a turn without an items array");
        };
        for item in items {
            let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                skipped_item_count = skipped_item_count.saturating_add(1);
                continue;
            };
            match item_type {
                "userMessage" => {
                    let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                        return malformed("a userMessage item has no string id");
                    };
                    let Some(content) = item.get("content").and_then(Value::as_array) else {
                        return malformed("a userMessage item has no content array");
                    };
                    let mut text_parts: Vec<&str> = Vec::new();
                    for part in content {
                        match part.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                let Some(text) = part.get("text").and_then(Value::as_str) else {
                                    return malformed("a userMessage text part has no string text");
                                };
                                text_parts.push(text);
                            }
                            _ => skipped_item_count = skipped_item_count.saturating_add(1),
                        }
                    }
                    if !text_parts.is_empty() {
                        sequence = sequence.saturating_add(1);
                        records.push(HistoryRecord {
                            source_provider: Provider::Codex,
                            source_session_id: expected_thread_id.to_string(),
                            source_record_id: format!("{turn_id}/{item_id}"),
                            sequence,
                            timestamp: timestamp_user,
                            role: HistoryRole::User,
                            kind: HistoryKind::UserMessage,
                            text: text_parts.join("\n"),
                        });
                    }
                }
                "agentMessage" => {
                    let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                        return malformed("an agentMessage item has no string id");
                    };
                    let Some(text) = item.get("text").and_then(Value::as_str) else {
                        return malformed("an agentMessage item has no string text");
                    };
                    match item.get("phase") {
                        None | Some(Value::Null) => {}
                        Some(Value::String(phase))
                            if phase == "commentary" || phase == "final_answer" => {}
                        _ => {
                            skipped_item_count = skipped_item_count.saturating_add(1);
                            continue;
                        }
                    }
                    sequence = sequence.saturating_add(1);
                    records.push(HistoryRecord {
                        source_provider: Provider::Codex,
                        source_session_id: expected_thread_id.to_string(),
                        source_record_id: format!("{turn_id}/{item_id}"),
                        sequence,
                        timestamp: timestamp_agent,
                        role: HistoryRole::Assistant,
                        kind: HistoryKind::AgentMessage,
                        text: text.to_string(),
                    });
                }
                _ => skipped_item_count = skipped_item_count.saturating_add(1),
            }
        }
    }

    SupervisorHistory::Available {
        adapter: "codex_app_server",
        adapter_version: 1,
        records,
        skipped_item_count,
    }
}

fn spawn_protocol_reader<R: Read + Send + 'static>(
    reader: R,
    sender: Sender<ReaderEvent>,
    max_bytes: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader: BufReader<R> = BufReader::new(reader);
        let mut total_bytes: usize = 0;
        let mut line: Vec<u8> = Vec::new();
        loop {
            let available: &[u8] = match reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = sender.send(ReaderEvent::ReadFailed(error.to_string()));
                    return;
                }
            };
            if available.is_empty() {
                if line.is_empty() {
                    let _ = sender.send(ReaderEvent::Closed);
                    return;
                }
                let _ = sender.send(ReaderEvent::ReadFailed(
                    "malformed JSON response: final line was not newline-terminated".to_string(),
                ));
                return;
            }
            let newline_index: Option<usize> =
                available.iter().position(|byte: &u8| *byte == b'\n');
            let take_count: usize = newline_index
                .map(|index: usize| index.saturating_add(1))
                .unwrap_or(available.len());
            if total_bytes.saturating_add(take_count) > max_bytes {
                let _ = sender.send(ReaderEvent::TooLarge);
                return;
            }
            line.extend_from_slice(&available[..take_count]);
            reader.consume(take_count);
            total_bytes = total_bytes.saturating_add(take_count);
            if newline_index.is_some() {
                match serde_json::from_slice::<Value>(&line) {
                    Ok(message) => {
                        if sender.send(ReaderEvent::Message(message)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(ReaderEvent::ReadFailed(format!(
                            "malformed JSON response: {error}"
                        )));
                        return;
                    }
                }
                line.clear();
            }
        }
    })
}

fn spawn_bounded_reader<R: Read + Send + 'static>(
    mut reader: R,
    max_bytes: usize,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes: Vec<u8> = Vec::with_capacity(max_bytes.min(8 * 1024));
        let mut buffer: [u8; 4096] = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let kept: usize = count.min(max_bytes.saturating_sub(bytes.len()));
                    if kept > 0 {
                        bytes.extend_from_slice(&buffer[..kept]);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        bytes
    })
}

fn terminate(child: &mut Child, child_pid: u32) {
    if matches!(child.try_wait(), Ok(Some(_status))) {
        return;
    }
    #[cfg(unix)]
    {
        let process_group: i32 = -(child_pid as i32);
        let result: i32 = unsafe { libc::kill(process_group, libc::SIGKILL) };
        if result != 0 {
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_pid;
        let _ = child.kill();
    }
}

fn rpc_error_message(message: &Value) -> String {
    message
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("codex app-server returned an unspecified RPC error")
        .to_string()
}

fn malformed(reason: impl Into<String>) -> SupervisorHistory {
    unavailable("codex_app_server", "malformed_response", reason)
}

fn unavailable(
    adapter: &'static str,
    reason_kind: &'static str,
    reason: impl Into<String>,
) -> SupervisorHistory {
    let mut reason: String = reason.into();
    const MAX_REASON_BYTES: usize = 2048;
    if reason.len() > MAX_REASON_BYTES {
        let mut boundary: usize = MAX_REASON_BYTES;
        while boundary > 0 && !reason.is_char_boundary(boundary) {
            boundary -= 1;
        }
        reason.truncate(boundary);
        reason.push_str("...[truncated]");
    }
    SupervisorHistory::Unavailable {
        adapter,
        reason_kind,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANTIGRAVITY_ID: &str = "0222067a-9e42-4b76-9649-66b84fd6bb26";

    fn response(workspace: &Path, thread_id: &str, items: Value) -> Value {
        json!({
            "id": THREAD_READ_REQUEST_ID,
            "result": {"thread": {
                "id": thread_id,
                "cwd": workspace,
                "turns": [{
                    "id": "turn-1",
                    "status": "completed",
                    "startedAt": 10,
                    "completedAt": 11,
                    "items": items
                }]
            }}
        })
    }

    #[test]
    fn projects_only_visible_user_and_agent_text() {
        let workspace: tempfile::TempDir = tempfile::tempdir().unwrap();
        let message: Value = response(
            workspace.path(),
            "thread-1",
            json!([
                {"id":"u1","type":"userMessage","content":[
                    {"type":"text","text":"hello"},
                    {"type":"image","url":"secret-image"}
                ]},
                {"id":"r1","type":"reasoning","content":["hidden"]},
                {"id":"c1","type":"commandExecution","aggregatedOutput":"secret-tool"},
                {"id":"a1","type":"agentMessage","text":"visible","phase":"final_answer"},
                {"id":"future","type":"futureProviderItem","text":"unknown"}
            ]),
        );
        let projected: SupervisorHistory =
            project_thread_read(&message, "thread-1", workspace.path());
        let SupervisorHistory::Available {
            records,
            skipped_item_count,
            ..
        } = projected
        else {
            panic!("expected available history");
        };
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].text, "hello");
        assert_eq!(records[1].text, "visible");
        assert_eq!(skipped_item_count, 4);
        let serialized: String = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("hidden"));
        assert!(!serialized.contains("secret-tool"));
        assert!(!serialized.contains("secret-image"));
        assert!(!serialized.contains("unknown"));
    }

    #[test]
    fn malformed_known_message_is_rejected_without_partial_projection() {
        let workspace: tempfile::TempDir = tempfile::tempdir().unwrap();
        let message: Value = response(
            workspace.path(),
            "thread-1",
            json!([
                {"id":"a1","type":"agentMessage","text":"would be partial"},
                {"id":"a2","type":"agentMessage"}
            ]),
        );
        let projected: SupervisorHistory =
            project_thread_read(&message, "thread-1", workspace.path());
        assert!(matches!(
            projected,
            SupervisorHistory::Unavailable {
                reason_kind: "malformed_response",
                ..
            }
        ));
    }

    #[test]
    fn mismatched_thread_and_workspace_are_rejected() {
        let workspace: tempfile::TempDir = tempfile::tempdir().unwrap();
        let other_workspace: tempfile::TempDir = tempfile::tempdir().unwrap();
        let wrong_thread: Value = response(workspace.path(), "other-thread", json!([]));
        assert!(matches!(
            project_thread_read(&wrong_thread, "thread-1", workspace.path()),
            SupervisorHistory::Unavailable {
                reason_kind: "malformed_response",
                ..
            }
        ));
        let wrong_workspace: Value = response(other_workspace.path(), "thread-1", json!([]));
        assert!(matches!(
            project_thread_read(&wrong_workspace, "thread-1", workspace.path()),
            SupervisorHistory::Unavailable {
                reason_kind: "workspace_mismatch",
                ..
            }
        ));
    }

    #[test]
    fn antigravity_projects_only_completed_visible_messages() {
        let transcript: Vec<u8> = concat!(
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-09-03T00:00:00Z","content":"question"}"#,
            "\n",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-09-03T00:00:01Z","content":"answer","thinking":"private"}"#,
            "\n",
            r#"{"step_index":2,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","content":"hidden"}"#,
            "\n",
            r#"{"step_index":3,"source":"MODEL","type":"PLANNER_RESPONSE","status":"RUNNING","content":"partial"}"#,
            "\n",
            r#"{"step_index":4,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"unfinished"}"#,
        )
        .as_bytes()
        .to_vec();

        let projected: SupervisorHistory =
            project_antigravity_transcript(&transcript, ANTIGRAVITY_ID);
        let SupervisorHistory::Available {
            adapter,
            adapter_version,
            records,
            skipped_item_count,
        } = projected
        else {
            panic!("expected available Antigravity history");
        };
        assert_eq!(adapter, "antigravity_transcript");
        assert_eq!(adapter_version, 1);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].text, "question");
        assert_eq!(records[1].text, "answer");
        assert_eq!(records[0].source_record_id, "step-0");
        assert_eq!(records[1].source_record_id, "step-1");
        assert_eq!(skipped_item_count, 3);
        let serialized: String = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("private"));
        assert!(!serialized.contains("hidden"));
        assert!(!serialized.contains("partial"));
        assert!(!serialized.contains("unfinished"));
    }

    #[test]
    fn antigravity_rejects_malformed_completed_visible_records() {
        let transcript: &[u8] =
            br#"{"step_index":0,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE"}
"#;
        assert!(matches!(
            project_antigravity_transcript(transcript, ANTIGRAVITY_ID),
            SupervisorHistory::Unavailable {
                reason_kind: "malformed_transcript",
                ..
            }
        ));
    }

    #[test]
    fn antigravity_reads_exact_workspace_validated_transcript() {
        let fixture: tempfile::TempDir = tempfile::tempdir().unwrap();
        let workspace: PathBuf = fixture.path().join("workspace");
        let state_root: PathBuf = fixture.path().join("antigravity-cli");
        let transcript_path: PathBuf = state_root
            .join("brain")
            .join(ANTIGRAVITY_ID)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(state_root.join("cache")).unwrap();
        std::fs::write(
            state_root.join("cache").join("last_conversations.json"),
            serde_json::to_vec(&json!({workspace.to_string_lossy(): ANTIGRAVITY_ID})).unwrap(),
        )
        .unwrap();
        std::fs::write(
            &transcript_path,
            concat!(
                r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"fixture question"}"#,
                "\n",
                r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"fixture answer"}"#,
                "\n",
            ),
        )
        .unwrap();

        let history: SupervisorHistory =
            read_antigravity_from_state_root(&state_root, ANTIGRAVITY_ID, &workspace);
        let SupervisorHistory::Available { records, .. } = history else {
            panic!("expected exact transcript to be available");
        };
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].text, "fixture answer");

        let other_id: &str = "849c7c61-7baf-4c6b-8767-5704603f08ff";
        assert!(matches!(
            read_antigravity_from_state_root(&state_root, other_id, &workspace),
            SupervisorHistory::Unavailable {
                reason_kind: "workspace_unverified",
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_rejects_transcript_symlink_escape() {
        use std::os::unix::fs::symlink;

        let fixture: tempfile::TempDir = tempfile::tempdir().unwrap();
        let workspace: PathBuf = fixture.path().join("workspace");
        let state_root: PathBuf = fixture.path().join("antigravity-cli");
        let logs: PathBuf = state_root
            .join("brain")
            .join(ANTIGRAVITY_ID)
            .join(".system_generated")
            .join("logs");
        let outside: PathBuf = fixture.path().join("outside.jsonl");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(state_root.join("cache")).unwrap();
        std::fs::write(&outside, b"{}\n").unwrap();
        symlink(&outside, logs.join("transcript.jsonl")).unwrap();
        std::fs::write(
            state_root.join("cache").join("last_conversations.json"),
            serde_json::to_vec(&json!({workspace.to_string_lossy(): ANTIGRAVITY_ID})).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            read_antigravity_from_state_root(&state_root, ANTIGRAVITY_ID, &workspace),
            SupervisorHistory::Unavailable {
                reason_kind: "unsafe_transcript_path",
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_fifo_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let fixture: tempfile::TempDir = tempfile::tempdir().unwrap();
        let fifo_path: PathBuf = fixture.path().join("transcript.jsonl");
        let fifo_path_c: CString = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
        let result: i32 = unsafe { libc::mkfifo(fifo_path_c.as_ptr(), 0o600) };
        assert_eq!(result, 0);

        let error: String = read_bounded_regular_file(&fifo_path, 1024).unwrap_err();
        assert!(error.contains("not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn transport_keeps_stdin_open_until_thread_read_response() {
        use std::os::unix::fs::PermissionsExt;

        let workspace: tempfile::TempDir = tempfile::tempdir().unwrap();
        let fixture_dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let program: PathBuf = fixture_dir.path().join("fake-codex");
        let initialize_response: String = json!({
            "id": INITIALIZE_REQUEST_ID,
            "result": {
                "userAgent": "fake",
                "platformFamily": "unix",
                "platformOs": "test",
                "codexHome": fixture_dir.path()
            }
        })
        .to_string();
        let thread_response: String = response(
            workspace.path(),
            "thread-1",
            json!([{"id":"u1","type":"userMessage","content":[
                {"type":"text","text":"from fake app-server"}
            ]}]),
        )
        .to_string();
        assert!(!initialize_response.contains('\''));
        assert!(!thread_response.contains('\''));
        let script: String = format!(
            "#!/bin/sh\nIFS= read -r initialize\nIFS= read -r initialized\nIFS= read -r thread_read\nprintf '%s\\n' '{initialize_response}'\nprintf '%s\\n' '{thread_response}'\nwhile IFS= read -r ignored; do :; done\n"
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();

        let result: SupervisorHistory = read_codex_with_program(
            program.as_os_str(),
            "thread-1",
            workspace.path(),
            Duration::from_secs(2),
        );
        let SupervisorHistory::Available { records, .. } = result else {
            panic!("expected the fake app-server response to be available");
        };
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, "from fake app-server");
    }
}
