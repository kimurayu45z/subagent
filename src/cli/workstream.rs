//! Explicit task-chain identity for provider-native child-session continuity.

use std::fmt;

use super::id::{MAX_ID_LEN, is_valid_logical_name};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct WorkstreamId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidWorkstreamId {
    value: String,
}

impl fmt::Display for InvalidWorkstreamId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid workstream id {:?}: must match [a-zA-Z0-9][a-zA-Z0-9._-]{{0,{}}}",
            self.value,
            MAX_ID_LEN - 1
        )
    }
}

impl std::error::Error for InvalidWorkstreamId {}

impl WorkstreamId {
    pub(crate) fn parse(raw: &str) -> Result<WorkstreamId, InvalidWorkstreamId> {
        if is_valid_logical_name(raw) {
            Ok(WorkstreamId(raw.to_string()))
        } else {
            Err(InvalidWorkstreamId {
                value: raw.to_string(),
            })
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkstreamId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_shared_logical_name_grammar() {
        assert!(WorkstreamId::parse("issue-42.review").is_ok());
        assert!(WorkstreamId::parse("a").is_ok());
    }

    #[test]
    fn rejects_empty_overlong_or_path_like_names() {
        assert!(WorkstreamId::parse("").is_err());
        assert!(WorkstreamId::parse(&"a".repeat(MAX_ID_LEN + 1)).is_err());
        assert!(WorkstreamId::parse("../other").is_err());
    }

    #[test]
    fn error_names_the_invalid_value() {
        let error: InvalidWorkstreamId = WorkstreamId::parse("bad work").unwrap_err();
        assert!(error.to_string().contains("bad work"));
    }
}
