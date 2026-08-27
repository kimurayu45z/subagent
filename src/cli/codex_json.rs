//! Managed Codex workstream transport.
//!
//! Ordinary `codex exec` remains byte-transparent. A tracked workstream adds
//! Codex's `--json` flag, captures the JSONL transport, verifies the exact
//! native thread ID, and restores the last completed agent message as normal
//! stdout.

use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub thread_id: String,
    pub final_message: Option<Vec<u8>>,
    pub turn_completed: bool,
    pub turn_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    OutputTruncated,
    NonUtf8,
    MalformedJson,
    MissingThread,
    MalformedThread,
    ConflictingThread,
    ThreadMismatch { expected: String, observed: String },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::OutputTruncated => {
                formatter.write_str("Codex JSONL output exceeded the capture limit")
            }
            ProtocolError::NonUtf8 => formatter.write_str("Codex JSONL output was not UTF-8"),
            ProtocolError::MalformedJson => {
                formatter.write_str("Codex emitted a malformed JSONL event")
            }
            ProtocolError::MissingThread => {
                formatter.write_str("Codex emitted no thread.started event")
            }
            ProtocolError::MalformedThread => {
                formatter.write_str("Codex emitted a malformed native thread id")
            }
            ProtocolError::ConflictingThread => {
                formatter.write_str("Codex emitted conflicting native thread ids")
            }
            ProtocolError::ThreadMismatch { expected, observed } => write!(
                formatter,
                "Codex resumed thread {observed:?}, but the workstream requires {expected:?}"
            ),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub(crate) fn observe(
    raw: &[u8],
    truncated: bool,
    expected_thread_id: Option<&str>,
) -> Result<Observation, ProtocolError> {
    if truncated {
        return Err(ProtocolError::OutputTruncated);
    }
    let text: &str = std::str::from_utf8(raw).map_err(|_| ProtocolError::NonUtf8)?;
    let mut thread_id: Option<String> = None;
    let mut final_message: Option<Vec<u8>> = None;
    let mut turn_completed: bool = false;
    let mut turn_failed: bool = false;

    for line in text.lines().filter(|line: &&str| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line).map_err(|_| ProtocolError::MalformedJson)?;
        match event.get("type").and_then(Value::as_str) {
            Some("thread.started") => {
                let observed: &str = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or(ProtocolError::MalformedThread)?;
                Uuid::parse_str(observed).map_err(|_| ProtocolError::MalformedThread)?;
                if thread_id
                    .as_deref()
                    .is_some_and(|prior: &str| prior != observed)
                {
                    return Err(ProtocolError::ConflictingThread);
                }
                thread_id = Some(observed.to_string());
            }
            Some("item.completed") => {
                let item: Option<&Value> = event.get("item");
                if item
                    .and_then(|value: &Value| value.get("type"))
                    .and_then(Value::as_str)
                    == Some("agent_message")
                    && let Some(message) = item
                        .and_then(|value: &Value| value.get("text"))
                        .and_then(Value::as_str)
                {
                    final_message = Some(message.as_bytes().to_vec());
                }
            }
            Some("turn.completed") => turn_completed = true,
            Some("turn.failed") => turn_failed = true,
            _ => {}
        }
    }

    let thread_id: String = thread_id.ok_or(ProtocolError::MissingThread)?;
    if let Some(expected) = expected_thread_id
        && thread_id != expected
    {
        return Err(ProtocolError::ThreadMismatch {
            expected: expected.to_string(),
            observed: thread_id,
        });
    }
    Ok(Observation {
        thread_id,
        final_message,
        turn_completed,
        turn_failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "019d300d-5f1b-7000-8000-000000000001";

    #[test]
    fn extracts_exact_thread_and_last_agent_message() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{THREAD}\"}}\n\
             {{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"first\"}}}}\n\
             {{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"final\"}}}}\n\
             {{\"type\":\"turn.completed\"}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, Some(THREAD)).unwrap();
        assert_eq!(observation.thread_id, THREAD);
        assert_eq!(observation.final_message, Some(b"final".to_vec()));
        assert!(observation.turn_completed);
        assert!(!observation.turn_failed);
    }

    #[test]
    fn exact_resume_mismatch_and_truncation_fail_closed() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{THREAD}\"}}\n\
             {{\"type\":\"turn.completed\"}}\n"
        )
        .into_bytes();
        assert!(matches!(
            observe(&raw, false, Some("019d300d-5f1b-7000-8000-000000000002")),
            Err(ProtocolError::ThreadMismatch { .. })
        ));
        assert_eq!(
            observe(&raw, true, None),
            Err(ProtocolError::OutputTruncated)
        );
    }

    #[test]
    fn incomplete_turn_remains_observable_but_unconfirmed() {
        let raw: Vec<u8> =
            format!("{{\"type\":\"thread.started\",\"thread_id\":\"{THREAD}\"}}\n").into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert!(!observation.turn_completed);
        assert!(observation.final_message.is_none());
    }

    #[test]
    fn failed_turn_is_observable_but_never_confirmed_by_the_caller() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{THREAD}\"}}\n\
             {{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"partial\"}}}}\n\
             {{\"type\":\"turn.failed\"}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert!(observation.turn_failed);
        assert!(!observation.turn_completed);
        assert_eq!(observation.final_message, Some(b"partial".to_vec()));
    }
}
