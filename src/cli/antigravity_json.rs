//! Managed Google Antigravity CLI stream-JSON transport.
//!
//! Antigravity's ordinary print mode does not incorporate piped stdin into a
//! positional prompt. Managed runs therefore send one typed user event over
//! `stream-json`, close stdin, and validate the terminal result before using
//! or persisting the observed native conversation ID.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub conversation_id: String,
    pub response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    OutputTruncated,
    NonUtf8,
    MalformedEvent,
    MissingConversationId,
    MalformedConversationId,
    ConflictingConversationId,
    ConversationIdMismatch { expected: String, observed: String },
    MissingResult,
    MultipleResults,
    UnsuccessfulResult(String),
    MissingResponse,
    EmptyResponse,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::OutputTruncated => {
                formatter.write_str("Antigravity stream-JSON output exceeded the capture limit")
            }
            ProtocolError::NonUtf8 => {
                formatter.write_str("Antigravity stream-JSON output was not UTF-8")
            }
            ProtocolError::MalformedEvent => {
                formatter.write_str("Antigravity emitted a malformed NDJSON event")
            }
            ProtocolError::MissingConversationId => {
                formatter.write_str("Antigravity emitted no native conversation ID")
            }
            ProtocolError::MalformedConversationId => {
                formatter.write_str("Antigravity emitted a malformed native conversation UUID")
            }
            ProtocolError::ConflictingConversationId => {
                formatter.write_str("Antigravity emitted conflicting native conversation IDs")
            }
            ProtocolError::ConversationIdMismatch { expected, observed } => write!(
                formatter,
                "Antigravity reported conversation {observed:?}, but the workstream requires {expected:?}"
            ),
            ProtocolError::MissingResult => {
                formatter.write_str("Antigravity emitted no terminal result event")
            }
            ProtocolError::MultipleResults => {
                formatter.write_str("Antigravity emitted multiple terminal result events")
            }
            ProtocolError::UnsuccessfulResult(status) => {
                write!(
                    formatter,
                    "Antigravity terminal result status was {status:?}"
                )
            }
            ProtocolError::MissingResponse => {
                formatter.write_str("Antigravity terminal result had no text response")
            }
            ProtocolError::EmptyResponse => {
                formatter.write_str("Antigravity terminal result response was empty")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug, Serialize)]
struct InputEvent<'a> {
    event: &'static str,
    message: InputMessage<'a>,
}

#[derive(Debug, Serialize)]
struct InputMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct OutputEvent {
    event: Option<String>,
    conversation_id: Option<String>,
    result: Option<ResultEvent>,
    step_update: Option<StepUpdate>,
}

#[derive(Debug, Deserialize)]
struct ResultEvent {
    conversation_id: Option<String>,
    status: Option<String>,
    response: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StepUpdate {
    conversation_id: Option<String>,
}

pub(crate) fn encode_user_event(prompt: &str) -> Result<Vec<u8>, serde_json::Error> {
    let event: InputEvent<'_> = InputEvent {
        event: "user",
        message: InputMessage {
            role: "user",
            content: prompt,
        },
    };
    let mut bytes: Vec<u8> = serde_json::to_vec(&event)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn is_valid_conversation_id(value: &str) -> bool {
    Uuid::parse_str(value)
        .map(|parsed: Uuid| parsed.hyphenated().to_string() == value)
        .unwrap_or(false)
}

pub(crate) fn observe(
    output: &[u8],
    truncated: bool,
    expected_conversation_id: Option<&str>,
) -> Result<Observation, ProtocolError> {
    if truncated {
        return Err(ProtocolError::OutputTruncated);
    }
    let text: &str = std::str::from_utf8(output).map_err(|_| ProtocolError::NonUtf8)?;
    let mut observed_id: Option<String> = None;
    let mut terminal_result: Option<ResultEvent> = None;

    for line in text.lines().filter(|line: &&str| !line.trim().is_empty()) {
        let event: OutputEvent =
            serde_json::from_str(line).map_err(|_| ProtocolError::MalformedEvent)?;
        if event.event.as_deref() == Some("init") {
            let value: &str = event
                .conversation_id
                .as_deref()
                .ok_or(ProtocolError::MissingConversationId)?;
            observe_id(&mut observed_id, value)?;
        }
        if event.event.as_deref() == Some("step_update")
            && let Some(step) = event.step_update.as_ref()
            && let Some(value) = step.conversation_id.as_deref()
        {
            observe_id(&mut observed_id, value)?;
        }
        if event.event.as_deref() == Some("result") {
            if terminal_result.is_some() {
                return Err(ProtocolError::MultipleResults);
            }
            let result: ResultEvent = event.result.ok_or(ProtocolError::MalformedEvent)?;
            if let Some(value) = result.conversation_id.as_deref() {
                observe_id(&mut observed_id, value)?;
            }
            terminal_result = Some(result);
        }
    }

    let conversation_id: String = observed_id.ok_or(ProtocolError::MissingConversationId)?;
    if let Some(expected) = expected_conversation_id
        && expected != conversation_id
    {
        return Err(ProtocolError::ConversationIdMismatch {
            expected: expected.to_string(),
            observed: conversation_id,
        });
    }
    let result: ResultEvent = terminal_result.ok_or(ProtocolError::MissingResult)?;
    let status: String = result.status.ok_or(ProtocolError::MalformedEvent)?;
    if status != "SUCCESS" {
        return Err(ProtocolError::UnsuccessfulResult(status));
    }
    let response: String = result.response.ok_or(ProtocolError::MissingResponse)?;
    if response.is_empty() {
        return Err(ProtocolError::EmptyResponse);
    }
    Ok(Observation {
        conversation_id,
        response: response.into_bytes(),
    })
}

fn observe_id(observed: &mut Option<String>, value: &str) -> Result<(), ProtocolError> {
    if !is_valid_conversation_id(value) {
        return Err(ProtocolError::MalformedConversationId);
    }
    match observed {
        Some(previous) if previous != value => Err(ProtocolError::ConflictingConversationId),
        Some(_) => Ok(()),
        None => {
            *observed = Some(value.to_string());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0222067a-9e42-4b76-9649-66b84fd6bb26";

    #[test]
    fn encodes_one_typed_user_event() {
        let encoded: Vec<u8> = encode_user_event("quoted \" task\nnext").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(value["event"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"], "quoted \" task\nnext");
        assert!(encoded.ends_with(b"\n"));
    }

    #[test]
    fn observes_success_and_tolerates_unknown_fields_and_events() {
        let output: String = format!(
            "{{\"event\":\"init\",\"conversation_id\":\"{ID}\",\"future\":1}}\n\
             {{\"event\":\"future_event\",\"anything\":true}}\n\
             {{\"event\":\"step_update\",\"step_update\":{{\"conversation_id\":\"{ID}\",\"step_type\":\"future\"}}}}\n\
             {{\"event\":\"result\",\"result\":{{\"conversation_id\":\"{ID}\",\"status\":\"SUCCESS\",\"response\":\"done\\n\"}}}}\n"
        );
        let observation: Observation = observe(output.as_bytes(), false, Some(ID)).unwrap();
        assert_eq!(observation.conversation_id, ID);
        assert_eq!(observation.response, b"done\n");
    }

    #[test]
    fn rejects_conflict_mismatch_failure_and_truncation() {
        let other: &str = "849c7c61-7baf-4c6b-8767-5704603f08ff";
        let conflict: String = format!(
            "{{\"event\":\"init\",\"conversation_id\":\"{ID}\"}}\n\
             {{\"event\":\"result\",\"result\":{{\"conversation_id\":\"{other}\",\"status\":\"SUCCESS\",\"response\":\"x\"}}}}\n"
        );
        assert_eq!(
            observe(conflict.as_bytes(), false, None),
            Err(ProtocolError::ConflictingConversationId)
        );
        let success: String = format!(
            "{{\"event\":\"result\",\"result\":{{\"conversation_id\":\"{ID}\",\"status\":\"SUCCESS\",\"response\":\"x\"}}}}\n"
        );
        assert!(matches!(
            observe(success.as_bytes(), false, Some(other)),
            Err(ProtocolError::ConversationIdMismatch { .. })
        ));
        let failure: String = format!(
            "{{\"event\":\"result\",\"result\":{{\"conversation_id\":\"{ID}\",\"status\":\"ERROR\",\"response\":\"x\"}}}}\n"
        );
        assert_eq!(
            observe(failure.as_bytes(), false, None),
            Err(ProtocolError::UnsuccessfulResult("ERROR".to_string()))
        );
        assert_eq!(
            observe(success.as_bytes(), true, None),
            Err(ProtocolError::OutputTruncated)
        );
        assert!(!is_valid_conversation_id(
            "0222067a9e424b76964966b84fd6bb26"
        ));
    }
}
