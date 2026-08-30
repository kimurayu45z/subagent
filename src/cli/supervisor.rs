//! Provider-neutral supervisor identity resolution.
//!
//! Implements the concrete types from `docs/design.md` section 3.1
//! (`SupervisorRef`) and the detection precedence from section 5, steps 1,
//! 2, 3, and 5:
//!
//! 1. `--supervisor <provider>:<session-id>`;
//! 2. `SUBAGENT_SELF_REF` (managed-parent reference) -- not implemented in
//!    this build, so its presence fails closed with an actionable
//!    diagnostic instead of silently falling through to step 3;
//! 3. exactly one unambiguous, non-empty native provider session id
//!    (`CODEX_THREAD_ID` or `CLAUDE_CODE_SESSION_ID`); or
//! 5. failure with an actionable diagnostic.
//!
//! Step 4 (a provider hook registry) is not implemented in this build; see
//! `doctor_cmd` for the honest capability report.

use std::ffi::{OsStr, OsString};
use std::fmt;

pub(crate) const SUBAGENT_SELF_REF_ENV: &str = "SUBAGENT_SELF_REF";
pub(crate) const CODEX_THREAD_ID_ENV: &str = "CODEX_THREAD_ID";
pub(crate) const CLAUDE_CODE_SESSION_ID_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// The provider that owns a supervisor session. See `docs/design.md`
/// section 3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
    OpenCode,
}

impl Provider {
    pub(crate) fn parse(raw: &str) -> Option<Provider> {
        match raw {
            "codex" => Some(Provider::Codex),
            "claude" => Some(Provider::Claude),
            "opencode" => Some(Provider::OpenCode),
            _ => None,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            Provider::OpenCode => "opencode",
        };
        f.write_str(text)
    }
}

/// How a [`SupervisorRef`] was detected. Only the mechanisms actually
/// implemented in this build have a variant; see the module doc comment for
/// the mechanisms that are deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DetectionSource {
    Explicit,
    NativeEnv,
}

/// Confidence in the resolved identity. This slice only returns exact
/// identities; unavailable detection is represented by an error instead of a
/// partially populated [`SupervisorRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DetectionConfidence {
    Exact,
}

impl fmt::Display for DetectionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            DetectionSource::Explicit => "explicit",
            DetectionSource::NativeEnv => "native-env",
        };
        f.write_str(text)
    }
}

/// A resolved supervisor identity: `docs/design.md` section 3.1's
/// `SupervisorRef`, restricted to the fields this build can populate
/// without workspace-identity or persistence support.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SupervisorRef {
    pub provider: Provider,
    pub session_id: String,
    pub detected_via: DetectionSource,
    pub confidence: DetectionConfidence,
}

impl fmt::Display for SupervisorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} (via {})",
            self.provider, self.session_id, self.detected_via
        )
    }
}

/// Every way supervisor resolution can fail closed, each carrying enough
/// detail for an actionable diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SupervisorResolutionError {
    InvalidExplicit,
    ManagedRefUnsupported,
    AmbiguousNativeIds,
    EmptyNativeId { var_name: &'static str },
    NonUtf8NativeId { var_name: &'static str },
    MissingIdentity,
}

impl fmt::Display for SupervisorResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SupervisorResolutionError::InvalidExplicit => write!(
                f,
                "invalid --supervisor value: expected codex:SESSION_ID, claude:SESSION_ID, or opencode:SESSION_ID"
            ),
            SupervisorResolutionError::ManagedRefUnsupported => write!(
                f,
                "{SUBAGENT_SELF_REF_ENV} is set, but resolving a managed-parent supervisor \
                 reference from it (docs/design.md section 5, step 2) is not implemented in \
                 this build; re-run with --supervisor codex:SESSION_ID or claude:SESSION_ID to \
                 name the supervisor explicitly"
            ),
            SupervisorResolutionError::AmbiguousNativeIds => write!(
                f,
                "both {CODEX_THREAD_ID_ENV} and {CLAUDE_CODE_SESSION_ID_ENV} are set; the immediate \
                 supervisor cannot be inferred safely -- re-run with --supervisor \
                 codex:SESSION_ID or claude:SESSION_ID to name the supervisor explicitly"
            ),
            SupervisorResolutionError::EmptyNativeId { var_name } => write!(
                f,
                "{var_name} is set but empty; an empty native session id is treated as invalid \
                 rather than absent -- re-run with --supervisor codex:SESSION_ID or \
                 claude:SESSION_ID, or unset {var_name}"
            ),
            SupervisorResolutionError::NonUtf8NativeId { var_name } => write!(
                f,
                "{var_name} contains a non-UTF-8 native session id and cannot be used safely -- \
                 re-run with --supervisor codex:SESSION_ID or claude:SESSION_ID, or unset \
                 {var_name}"
            ),
            SupervisorResolutionError::MissingIdentity => write!(
                f,
                "no supervisor identity found: re-run with --supervisor codex:SESSION_ID or \
                 claude:SESSION_ID, or run inside a Codex ({CODEX_THREAD_ID_ENV}) or Claude \
                 Code ({CLAUDE_CODE_SESSION_ID_ENV}) session"
            ),
        }
    }
}

impl std::error::Error for SupervisorResolutionError {}

/// The subset of the process environment relevant to supervisor detection,
/// injected explicitly so resolution can be exercised in tests without
/// mutating real process environment state (which is unsafe to do from
/// parallel test threads).
#[derive(Debug, Clone, Default)]
pub(crate) struct DetectionEnv {
    pub self_ref: Option<OsString>,
    pub codex_thread_id: Option<OsString>,
    pub claude_session_id: Option<OsString>,
}

impl DetectionEnv {
    pub(crate) fn from_process_env() -> Self {
        DetectionEnv {
            self_ref: std::env::var_os(SUBAGENT_SELF_REF_ENV),
            codex_thread_id: std::env::var_os(CODEX_THREAD_ID_ENV),
            claude_session_id: std::env::var_os(CLAUDE_CODE_SESSION_ID_ENV),
        }
    }
}

/// Resolves the immediate supervisor with the strict precedence documented
/// on this module: a valid explicit override always wins; otherwise a
/// present `SUBAGENT_SELF_REF` fails closed; otherwise exactly one
/// non-empty native provider session id is required.
pub(crate) fn resolve(
    explicit_raw: Option<&str>,
    env: &DetectionEnv,
) -> Result<SupervisorRef, SupervisorResolutionError> {
    if let Some(raw) = explicit_raw {
        return parse_explicit(raw);
    }
    if env.self_ref.is_some() {
        return Err(SupervisorResolutionError::ManagedRefUnsupported);
    }

    let codex_id: Option<&str> =
        check_native_id(CODEX_THREAD_ID_ENV, env.codex_thread_id.as_deref())?;
    let claude_id: Option<&str> =
        check_native_id(CLAUDE_CODE_SESSION_ID_ENV, env.claude_session_id.as_deref())?;

    match (codex_id, claude_id) {
        (Some(id), None) => Ok(SupervisorRef {
            provider: Provider::Codex,
            session_id: id.to_string(),
            detected_via: DetectionSource::NativeEnv,
            confidence: DetectionConfidence::Exact,
        }),
        (None, Some(id)) => Ok(SupervisorRef {
            provider: Provider::Claude,
            session_id: id.to_string(),
            detected_via: DetectionSource::NativeEnv,
            confidence: DetectionConfidence::Exact,
        }),
        (Some(_), Some(_)) => Err(SupervisorResolutionError::AmbiguousNativeIds),
        (None, None) => Err(SupervisorResolutionError::MissingIdentity),
    }
}

fn check_native_id<'a>(
    var_name: &'static str,
    value: Option<&'a OsStr>,
) -> Result<Option<&'a str>, SupervisorResolutionError> {
    match value {
        None => Ok(None),
        Some(id) if id.is_empty() => Err(SupervisorResolutionError::EmptyNativeId { var_name }),
        Some(id) => id
            .to_str()
            .map(Some)
            .ok_or(SupervisorResolutionError::NonUtf8NativeId { var_name }),
    }
}

fn parse_explicit(raw: &str) -> Result<SupervisorRef, SupervisorResolutionError> {
    let invalid = || SupervisorResolutionError::InvalidExplicit;
    let (provider_raw, session_id): (&str, &str) = raw.split_once(':').ok_or_else(invalid)?;
    let provider: Provider = Provider::parse(provider_raw).ok_or_else(invalid)?;
    if session_id.is_empty() {
        return Err(invalid());
    }
    Ok(SupervisorRef {
        provider,
        session_id: session_id.to_string(),
        detected_via: DetectionSource::Explicit,
        confidence: DetectionConfidence::Exact,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(self_ref: Option<&str>, codex: Option<&str>, claude: Option<&str>) -> DetectionEnv {
        DetectionEnv {
            self_ref: self_ref.map(OsString::from),
            codex_thread_id: codex.map(OsString::from),
            claude_session_id: claude.map(OsString::from),
        }
    }

    #[test]
    fn explicit_codex_reference_resolves() {
        let resolved = resolve(Some("codex:abc123"), &DetectionEnv::default()).unwrap();
        assert_eq!(resolved.provider, Provider::Codex);
        assert_eq!(resolved.session_id, "abc123");
        assert_eq!(resolved.detected_via, DetectionSource::Explicit);
        assert_eq!(resolved.confidence, DetectionConfidence::Exact);
    }

    #[test]
    fn explicit_claude_reference_resolves() {
        let resolved = resolve(Some("claude:xyz789"), &DetectionEnv::default()).unwrap();
        assert_eq!(resolved.provider, Provider::Claude);
        assert_eq!(resolved.session_id, "xyz789");
        assert_eq!(resolved.detected_via, DetectionSource::Explicit);
    }

    #[test]
    fn explicit_opencode_reference_resolves() {
        let resolved: SupervisorRef =
            resolve(Some("opencode:ses_xyz789"), &DetectionEnv::default()).unwrap();
        assert_eq!(resolved.provider, Provider::OpenCode);
        assert_eq!(resolved.session_id, "ses_xyz789");
        assert_eq!(resolved.detected_via, DetectionSource::Explicit);
    }

    #[test]
    fn explicit_reference_wins_over_native_env_and_self_ref() {
        let detection_env = env(
            Some("/tmp/manifest.json"),
            Some("thread-1"),
            Some("session-1"),
        );
        let resolved = resolve(Some("codex:override"), &detection_env).unwrap();
        assert_eq!(resolved.provider, Provider::Codex);
        assert_eq!(resolved.session_id, "override");
        assert_eq!(resolved.detected_via, DetectionSource::Explicit);
    }

    #[test]
    fn explicit_reference_rejects_unknown_provider() {
        let error = resolve(Some("unknown:session"), &DetectionEnv::default()).unwrap_err();
        assert!(matches!(error, SupervisorResolutionError::InvalidExplicit));
    }

    #[test]
    fn explicit_reference_rejects_missing_session_id() {
        let error = resolve(Some("codex:"), &DetectionEnv::default()).unwrap_err();
        assert!(matches!(error, SupervisorResolutionError::InvalidExplicit));
    }

    #[test]
    fn explicit_reference_rejects_missing_colon() {
        let error = resolve(Some("codex"), &DetectionEnv::default()).unwrap_err();
        assert!(matches!(error, SupervisorResolutionError::InvalidExplicit));
    }

    #[test]
    fn managed_ref_fails_closed_instead_of_falling_through() {
        let detection_env = env(Some("/tmp/manifest.json"), Some("thread-1"), None);
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(error, SupervisorResolutionError::ManagedRefUnsupported);
    }

    #[test]
    fn managed_ref_fails_closed_even_when_no_native_id_is_present() {
        let detection_env = env(Some("/tmp/manifest.json"), None, None);
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(error, SupervisorResolutionError::ManagedRefUnsupported);
    }

    #[test]
    fn native_codex_thread_id_resolves_when_alone() {
        let detection_env = env(None, Some("thread-1"), None);
        let resolved = resolve(None, &detection_env).unwrap();
        assert_eq!(resolved.provider, Provider::Codex);
        assert_eq!(resolved.session_id, "thread-1");
        assert_eq!(resolved.detected_via, DetectionSource::NativeEnv);
    }

    #[test]
    fn native_claude_session_id_resolves_when_alone() {
        let detection_env = env(None, None, Some("session-1"));
        let resolved = resolve(None, &detection_env).unwrap();
        assert_eq!(resolved.provider, Provider::Claude);
        assert_eq!(resolved.session_id, "session-1");
        assert_eq!(resolved.detected_via, DetectionSource::NativeEnv);
    }

    #[test]
    fn both_native_ids_present_is_ambiguous() {
        let detection_env = env(None, Some("thread-1"), Some("session-1"));
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(error, SupervisorResolutionError::AmbiguousNativeIds);
    }

    #[test]
    fn no_identity_present_is_a_missing_identity_error() {
        let error = resolve(None, &DetectionEnv::default()).unwrap_err();
        assert_eq!(error, SupervisorResolutionError::MissingIdentity);
    }

    #[test]
    fn present_but_empty_codex_id_is_invalid_not_absent() {
        let detection_env = env(None, Some(""), None);
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(
            error,
            SupervisorResolutionError::EmptyNativeId {
                var_name: CODEX_THREAD_ID_ENV
            }
        );
    }

    #[test]
    fn present_but_empty_claude_id_is_invalid_not_absent() {
        let detection_env = env(None, None, Some(""));
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(
            error,
            SupervisorResolutionError::EmptyNativeId {
                var_name: CLAUDE_CODE_SESSION_ID_ENV
            }
        );
    }

    #[test]
    fn empty_codex_id_is_reported_even_when_claude_id_is_valid() {
        let detection_env = env(None, Some(""), Some("session-1"));
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(
            error,
            SupervisorResolutionError::EmptyNativeId {
                var_name: CODEX_THREAD_ID_ENV
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_codex_id_is_rejected_instead_of_ignoring_ambiguity() {
        use std::os::unix::ffi::OsStringExt;

        let detection_env = DetectionEnv {
            self_ref: None,
            codex_thread_id: Some(OsString::from_vec(vec![0xff])),
            claude_session_id: Some(OsString::from("session-1")),
        };
        let error = resolve(None, &detection_env).unwrap_err();
        assert_eq!(
            error,
            SupervisorResolutionError::NonUtf8NativeId {
                var_name: CODEX_THREAD_ID_ENV
            }
        );
    }

    #[test]
    fn display_messages_are_actionable() {
        assert!(
            SupervisorResolutionError::ManagedRefUnsupported
                .to_string()
                .contains("--supervisor")
        );
        assert!(
            SupervisorResolutionError::MissingIdentity
                .to_string()
                .contains("--supervisor")
        );
        assert!(
            SupervisorResolutionError::EmptyNativeId {
                var_name: CODEX_THREAD_ID_ENV
            }
            .to_string()
            .contains("--supervisor")
        );
        assert!(
            SupervisorResolutionError::AmbiguousNativeIds
                .to_string()
                .contains("--supervisor")
        );
        assert!(
            SupervisorResolutionError::NonUtf8NativeId {
                var_name: CODEX_THREAD_ID_ENV
            }
            .to_string()
            .contains("--supervisor")
        );
    }
}
