//! Managed OpenCode workstream transport.
//!
//! `opencode run --format json` emits newline-delimited JSON events. This
//! module is a pure parser that turns that NDJSON stream into a compact
//! `Observation`. It does not spawn processes, enforce capture limits, or
//! select which child adapter runs; those remain the caller's responsibility.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub session_id: String,
    pub final_message: Option<Vec<u8>>,
    pub turn_completed: bool,
    pub turn_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    OutputTruncated,
    NonUtf8,
    MalformedJson,
    MissingSessionId,
    MalformedSessionId,
    ConflictingSessionId,
    SessionIdMismatch { expected: String, observed: String },
    MalformedTextEvent,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::OutputTruncated => {
                formatter.write_str("OpenCode JSON output exceeded the capture limit")
            }
            ProtocolError::NonUtf8 => formatter.write_str("OpenCode JSON output was not UTF-8"),
            ProtocolError::MalformedJson => {
                formatter.write_str("OpenCode emitted a malformed NDJSON event")
            }
            ProtocolError::MissingSessionId => {
                formatter.write_str("OpenCode emitted no top-level sessionID")
            }
            ProtocolError::MalformedSessionId => {
                formatter.write_str("OpenCode emitted a malformed top-level sessionID")
            }
            ProtocolError::ConflictingSessionId => {
                formatter.write_str("OpenCode emitted conflicting top-level sessionIDs")
            }
            ProtocolError::SessionIdMismatch { expected, observed } => write!(
                formatter,
                "OpenCode reported session {observed:?}, but the workstream requires {expected:?}"
            ),
            ProtocolError::MalformedTextEvent => {
                formatter.write_str("OpenCode emitted a malformed text event")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Validates the exact `sessionID` grammar: 1..=256 ASCII bytes, prefixed
/// `ses_`, with every remaining byte an ASCII alphanumeric, underscore, or
/// hyphen.
pub(crate) fn is_valid_session_id(candidate: &str) -> bool {
    const PREFIX: &str = "ses_";
    let byte_len: usize = candidate.len();
    if !(1..=256).contains(&byte_len) {
        return false;
    }
    if !candidate.is_ascii() {
        return false;
    }
    let Some(rest) = candidate.strip_prefix(PREFIX) else {
        return false;
    };
    !rest.is_empty()
        && rest
            .bytes()
            .all(|byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub(crate) fn observe(
    raw: &[u8],
    truncated: bool,
    expected_session_id: Option<&str>,
) -> Result<Observation, ProtocolError> {
    if truncated {
        return Err(ProtocolError::OutputTruncated);
    }
    let text: &str = std::str::from_utf8(raw).map_err(|_| ProtocolError::NonUtf8)?;
    let mut session_id: Option<String> = None;
    let mut text_parts: Vec<String> = Vec::new();
    let mut turn_completed: bool = false;
    let mut turn_failed: bool = false;

    for line in text.lines().filter(|line: &&str| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line).map_err(|_| ProtocolError::MalformedJson)?;

        let observed: &str = event
            .get("sessionID")
            .ok_or(ProtocolError::MissingSessionId)?
            .as_str()
            .ok_or(ProtocolError::MalformedSessionId)?;
        if !is_valid_session_id(observed) {
            return Err(ProtocolError::MalformedSessionId);
        }
        if session_id
            .as_deref()
            .is_some_and(|prior: &str| prior != observed)
        {
            return Err(ProtocolError::ConflictingSessionId);
        }
        session_id = Some(observed.to_string());

        match event.get("type").and_then(Value::as_str) {
            Some("step_start") => {
                turn_completed = false;
            }
            Some("text") => {
                let part_text: &str = event
                    .get("part")
                    .and_then(|part: &Value| part.get("text"))
                    .and_then(Value::as_str)
                    .ok_or(ProtocolError::MalformedTextEvent)?;
                if !part_text.is_empty() {
                    text_parts.push(part_text.to_string());
                }
            }
            Some("step_finish") => {
                turn_completed = true;
            }
            Some("error") => turn_failed = true,
            _ => {}
        }
    }

    let session_id: String = session_id.ok_or(ProtocolError::MissingSessionId)?;
    if let Some(expected) = expected_session_id
        && session_id != expected
    {
        return Err(ProtocolError::SessionIdMismatch {
            expected: expected.to_string(),
            observed: session_id,
        });
    }

    let final_message: Option<Vec<u8>> = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n").into_bytes())
    };

    Ok(Observation {
        session_id,
        final_message,
        turn_completed,
        turn_failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "ses_9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c";
    const OTHER_SESSION: &str = "ses_00000000000000000000000000000001";

    #[test]
    fn success_extracts_session_and_final_message() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"hello\"}}}}\n\
             {{\"type\":\"step_finish\",\"sessionID\":\"{SESSION}\"}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, Some(SESSION)).unwrap();
        assert_eq!(observation.session_id, SESSION);
        assert_eq!(observation.final_message, Some(b"hello".to_vec()));
        assert!(observation.turn_completed);
        assert!(!observation.turn_failed);
    }

    #[test]
    fn split_text_across_events_joins_with_newline_and_skips_empty_parts() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"first\"}}}}\n\
             {{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"\"}}}}\n\
             {{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"second\"}}}}\n\
             {{\"type\":\"step_finish\",\"sessionID\":\"{SESSION}\"}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert_eq!(observation.final_message, Some(b"first\nsecond".to_vec()));
    }

    #[test]
    fn expected_session_mismatch_fails_closed() {
        let raw: Vec<u8> =
            format!("{{\"type\":\"step_finish\",\"sessionID\":\"{SESSION}\"}}\n").into_bytes();
        assert_eq!(
            observe(&raw, false, Some(OTHER_SESSION)),
            Err(ProtocolError::SessionIdMismatch {
                expected: OTHER_SESSION.to_string(),
                observed: SESSION.to_string(),
            })
        );
    }

    #[test]
    fn conflicting_session_ids_fail_closed() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"a\"}}}}\n\
             {{\"type\":\"step_finish\",\"sessionID\":\"{OTHER_SESSION}\"}}\n"
        )
        .into_bytes();
        assert_eq!(
            observe(&raw, false, None),
            Err(ProtocolError::ConflictingSessionId)
        );
    }

    #[test]
    fn malformed_session_id_variants_fail_closed() {
        let bad_ids: [&str; 6] = [
            "sess_abc123",   // wrong prefix
            "ses_",          // prefix without an identifier
            "ses_abc def",   // disallowed space
            "ses_abc.def",   // disallowed period
            "ses_caf\u{e9}", // non-ASCII byte
            "",              // empty string, not even prefixed
        ];
        for bad_id in bad_ids {
            let raw: Vec<u8> =
                format!("{{\"type\":\"step_finish\",\"sessionID\":\"{bad_id}\"}}\n").into_bytes();
            assert_eq!(
                observe(&raw, false, None),
                Err(ProtocolError::MalformedSessionId),
                "expected malformed session id for {bad_id:?}"
            );
        }
    }

    #[test]
    fn session_id_length_boundary_is_enforced() {
        let valid_long: String = format!("ses_{}", "a".repeat(252));
        assert_eq!(valid_long.len(), 256);
        let raw: Vec<u8> =
            format!("{{\"type\":\"step_finish\",\"sessionID\":\"{valid_long}\"}}\n").into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert_eq!(observation.session_id, valid_long);

        let invalid_long: String = format!("ses_{}", "a".repeat(253));
        assert_eq!(invalid_long.len(), 257);
        let raw: Vec<u8> =
            format!("{{\"type\":\"step_finish\",\"sessionID\":\"{invalid_long}\"}}\n").into_bytes();
        assert_eq!(
            observe(&raw, false, None),
            Err(ProtocolError::MalformedSessionId)
        );
    }

    #[test]
    fn malformed_text_event_fails_closed() {
        let bad_bodies: [&str; 3] = [
            "{\"type\":\"text\",\"sessionID\":\"{SESSION}\"}", // missing part
            "{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":\"oops\"}", // part not object
            "{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{\"text\":123}}", // text not string
        ];
        for body in bad_bodies {
            let line: String = body.replace("{SESSION}", SESSION);
            let raw: Vec<u8> = format!("{line}\n").into_bytes();
            assert_eq!(
                observe(&raw, false, None),
                Err(ProtocolError::MalformedTextEvent),
                "expected malformed text event for {line:?}"
            );
        }
    }

    #[test]
    fn truncated_output_fails_closed_before_parsing() {
        let raw: Vec<u8> = b"not even valid json".to_vec();
        assert_eq!(
            observe(&raw, true, None),
            Err(ProtocolError::OutputTruncated)
        );
    }

    #[test]
    fn incomplete_turn_remains_observable_but_unconfirmed() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"partial\"}}}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert!(!observation.turn_completed);
        assert!(!observation.turn_failed);
        assert_eq!(observation.final_message, Some(b"partial".to_vec()));
    }

    #[test]
    fn a_later_open_step_resets_an_earlier_completion() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"step_start\",\"sessionID\":\"{SESSION}\"}}\n\
             {{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"first\"}}}}\n\
             {{\"type\":\"step_finish\",\"sessionID\":\"{SESSION}\"}}\n\
             {{\"type\":\"step_start\",\"sessionID\":\"{SESSION}\"}}\n\
             {{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"partial second\"}}}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert!(!observation.turn_completed);
        assert!(!observation.turn_failed);
        assert_eq!(
            observation.final_message,
            Some(b"first\npartial second".to_vec())
        );
    }

    #[test]
    fn error_event_marks_turn_failed_without_completion() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"text\",\"sessionID\":\"{SESSION}\",\"part\":{{\"text\":\"partial\"}}}}\n\
             {{\"type\":\"error\",\"sessionID\":\"{SESSION}\"}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert!(observation.turn_failed);
        assert!(!observation.turn_completed);
        assert_eq!(observation.final_message, Some(b"partial".to_vec()));
    }

    #[test]
    fn missing_session_id_fails_closed() {
        let raw: Vec<u8> =
            "{\"type\":\"step_finish\"}\n{\"type\":\"text\",\"part\":{\"text\":\"hi\"}}\n"
                .as_bytes()
                .to_vec();
        assert_eq!(
            observe(&raw, false, None),
            Err(ProtocolError::MissingSessionId)
        );
    }

    #[test]
    fn non_utf8_output_fails_closed() {
        let raw: Vec<u8> = vec![0xff, 0xfe, 0xfd];
        assert_eq!(observe(&raw, false, None), Err(ProtocolError::NonUtf8));
    }

    #[test]
    fn malformed_json_line_fails_closed() {
        let raw: Vec<u8> = b"{not json}\n".to_vec();
        assert_eq!(
            observe(&raw, false, None),
            Err(ProtocolError::MalformedJson)
        );
    }

    #[test]
    fn unknown_event_types_are_ignored() {
        let raw: Vec<u8> = format!(
            "{{\"type\":\"tool_call\",\"sessionID\":\"{SESSION}\",\"anything\":true}}\n\
             {{\"type\":\"step_finish\",\"sessionID\":\"{SESSION}\"}}\n"
        )
        .into_bytes();
        let observation: Observation = observe(&raw, false, None).unwrap();
        assert_eq!(observation.session_id, SESSION);
        assert!(observation.turn_completed);
        assert!(observation.final_message.is_none());
    }
}
