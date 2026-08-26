//! `SubagentId`: the user-controlled logical subordinate identity described
//! in `docs/design.md` section 3.2.

use std::fmt;

/// Maximum length, in characters, of a valid `SubagentId`, matching the
/// design grammar `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}`.
pub(crate) const MAX_ID_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct SubagentId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidSubagentId {
    value: String,
}

impl fmt::Display for InvalidSubagentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid subagent id {:?}: must match [a-zA-Z0-9][a-zA-Z0-9._-]{{0,{}}}",
            self.value,
            MAX_ID_LEN - 1
        )
    }
}

impl std::error::Error for InvalidSubagentId {}

impl SubagentId {
    pub(crate) fn parse(raw: &str) -> Result<Self, InvalidSubagentId> {
        if Self::is_valid(raw) {
            Ok(SubagentId(raw.to_string()))
        } else {
            Err(InvalidSubagentId {
                value: raw.to_string(),
            })
        }
    }

    fn is_valid(raw: &str) -> bool {
        if raw.is_empty() || raw.len() > MAX_ID_LEN {
            return false;
        }
        let mut chars = raw.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_ascii_alphanumeric() {
            return false;
        }
        chars.all(|c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SubagentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_minimal_single_character_id() {
        assert!(SubagentId::parse("a").is_ok());
        assert!(SubagentId::parse("9").is_ok());
    }

    #[test]
    fn accepts_mixed_allowed_characters() {
        assert!(SubagentId::parse("claude-opus-architect").is_ok());
        assert!(SubagentId::parse("reviewer_2.beta").is_ok());
    }

    #[test]
    fn rejects_empty_id() {
        assert!(SubagentId::parse("").is_err());
    }

    #[test]
    fn rejects_id_starting_with_non_alphanumeric() {
        assert!(SubagentId::parse("-reviewer").is_err());
        assert!(SubagentId::parse(".reviewer").is_err());
        assert!(SubagentId::parse("_reviewer").is_err());
    }

    #[test]
    fn rejects_id_with_disallowed_characters() {
        assert!(SubagentId::parse("reviewer/1").is_err());
        assert!(SubagentId::parse("reviewer 1").is_err());
        assert!(SubagentId::parse("reviewer:1").is_err());
    }

    #[test]
    fn accepts_id_at_max_length() {
        let id = "a".repeat(MAX_ID_LEN);
        assert!(SubagentId::parse(&id).is_ok());
    }

    #[test]
    fn rejects_id_over_max_length() {
        let id = "a".repeat(MAX_ID_LEN + 1);
        assert!(SubagentId::parse(&id).is_err());
    }

    #[test]
    fn error_message_names_the_invalid_value() {
        let err = SubagentId::parse("bad id").unwrap_err();
        assert!(err.to_string().contains("bad id"));
    }
}
