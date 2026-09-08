//! Managed Cursor Agent CLI JSON transport.
//!
//! `agent -p --output-format json` emits one terminal JSON object. This
//! module validates that object before the wrapper persists or resumes the
//! provider-issued session ID.

use std::fmt;

use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub session_id: String,
    pub response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    OutputTruncated,
    NonUtf8,
    MalformedJson,
    NotTerminalResult,
    UnsuccessfulResult,
    MissingSessionId,
    MalformedSessionId,
    SessionIdMismatch { expected: String, observed: String },
    MissingResponse,
    EmptyResponse,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::OutputTruncated => {
                formatter.write_str("Cursor JSON output exceeded the capture limit")
            }
            ProtocolError::NonUtf8 => formatter.write_str("Cursor JSON output was not UTF-8"),
            ProtocolError::MalformedJson => {
                formatter.write_str("Cursor emitted malformed terminal JSON")
            }
            ProtocolError::NotTerminalResult => {
                formatter.write_str("Cursor emitted no successful terminal result object")
            }
            ProtocolError::UnsuccessfulResult => {
                formatter.write_str("Cursor terminal result reported failure")
            }
            ProtocolError::MissingSessionId => {
                formatter.write_str("Cursor terminal result had no session_id")
            }
            ProtocolError::MalformedSessionId => {
                formatter.write_str("Cursor terminal result had a malformed session UUID")
            }
            ProtocolError::SessionIdMismatch { expected, observed } => write!(
                formatter,
                "Cursor reported session {observed:?}, but the workstream requires {expected:?}"
            ),
            ProtocolError::MissingResponse => {
                formatter.write_str("Cursor terminal result had no text response")
            }
            ProtocolError::EmptyResponse => {
                formatter.write_str("Cursor terminal result response was empty")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug, Deserialize)]
struct ResultEnvelope {
    #[serde(rename = "type")]
    kind: Option<String>,
    subtype: Option<String>,
    is_error: Option<bool>,
    result: Option<String>,
    session_id: Option<String>,
}

pub(crate) fn is_valid_session_id(value: &str) -> bool {
    Uuid::parse_str(value)
        .map(|parsed: Uuid| parsed.hyphenated().to_string() == value)
        .unwrap_or(false)
}

pub(crate) fn observe(
    output: &[u8],
    truncated: bool,
    expected_session_id: Option<&str>,
) -> Result<Observation, ProtocolError> {
    if truncated {
        return Err(ProtocolError::OutputTruncated);
    }
    let text: &str = std::str::from_utf8(output).map_err(|_| ProtocolError::NonUtf8)?;
    let result: ResultEnvelope =
        serde_json::from_str(text.trim()).map_err(|_| ProtocolError::MalformedJson)?;
    if result.kind.as_deref() != Some("result") || result.subtype.as_deref() != Some("success") {
        return Err(ProtocolError::NotTerminalResult);
    }
    if result.is_error != Some(false) {
        return Err(ProtocolError::UnsuccessfulResult);
    }
    let session_id: String = result.session_id.ok_or(ProtocolError::MissingSessionId)?;
    if !is_valid_session_id(&session_id) {
        return Err(ProtocolError::MalformedSessionId);
    }
    if let Some(expected) = expected_session_id
        && expected != session_id
    {
        return Err(ProtocolError::SessionIdMismatch {
            expected: expected.to_string(),
            observed: session_id,
        });
    }
    let response: String = result.result.ok_or(ProtocolError::MissingResponse)?;
    if response.is_empty() {
        return Err(ProtocolError::EmptyResponse);
    }
    Ok(Observation {
        session_id,
        response: response.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "3eba5a93-e2ca-4596-8ada-b3069d83ca25";
    const OTHER: &str = "019d300d-5f1b-7000-8000-000000000002";

    fn success(session_id: &str, response: &str) -> Vec<u8> {
        format!(
            "{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":{response:?},\"session_id\":\"{session_id}\",\"usage\":{{\"inputTokens\":1}}}}\n"
        )
        .into_bytes()
    }

    #[test]
    fn extracts_exact_session_and_response_while_ignoring_usage() {
        let observation: Observation = observe(&success(SESSION, "done"), false, Some(SESSION))
            .expect("valid Cursor result should parse");
        assert_eq!(observation.session_id, SESSION);
        assert_eq!(observation.response, b"done");
    }

    #[test]
    fn mismatch_and_truncation_fail_closed() {
        assert!(matches!(
            observe(&success(SESSION, "done"), false, Some(OTHER)),
            Err(ProtocolError::SessionIdMismatch { .. })
        ));
        assert_eq!(
            observe(&success(SESSION, "done"), true, None),
            Err(ProtocolError::OutputTruncated)
        );
    }

    #[test]
    fn malformed_or_unsuccessful_results_fail_closed() {
        assert_eq!(
            observe(b"not-json", false, None),
            Err(ProtocolError::MalformedJson)
        );
        assert_eq!(
            observe(
                br#"{"type":"result","subtype":"error","is_error":true,"result":"no","session_id":"3eba5a93-e2ca-4596-8ada-b3069d83ca25"}"#,
                false,
                None
            ),
            Err(ProtocolError::NotTerminalResult)
        );
        assert_eq!(
            observe(&success("not-a-uuid", "done"), false, None),
            Err(ProtocolError::MalformedSessionId)
        );
        assert_eq!(
            observe(&success(SESSION, ""), false, None),
            Err(ProtocolError::EmptyResponse)
        );
    }
}
