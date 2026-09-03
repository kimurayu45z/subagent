//! Read-only supervisor conversation adapters.
//!
//! The Codex adapter uses only app-server's `initialize` and `thread/read`
//! methods. It projects an allowlist of visible message items and treats all
//! other item kinds as untrusted provider internals that must not cross the
//! supervisor/subordinate boundary.

use std::ffi::OsStr;
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
        Provider::Antigravity => unavailable(
            "antigravity_transcript",
            "not_implemented",
            "the Antigravity supervisor-history adapter is not implemented yet",
        ),
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
