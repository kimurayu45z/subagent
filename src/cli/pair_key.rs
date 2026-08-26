//! `PairKey`: `docs/design.md` section 3.4's
//! `hash(schema_version, workspace_identity, supervisor_provider,
//! supervisor_session_id, subagent_id)`.
//!
//! The hash is SHA-256 over a fixed domain prefix followed by each field,
//! individually framed with a `u64` little-endian byte length before its raw
//! bytes. Length framing prevents two different field splits that happen to
//! concatenate to the same raw bytes (for example workspace bytes `"Xab"`
//! plus session id `"Ysession"`) from colliding with a different split
//! (workspace bytes `"X"` plus session id `"abYsession"`).
//!
//! The pair-key schema version below is deliberately independent from
//! [`crate::cli::report::REPORT_SCHEMA_VERSION`] and from the on-disk
//! ledger's `LEDGER_SCHEMA_VERSION`/`PRAGMA user_version`: each versions a
//! different artifact, and bumping one must not silently reinterpret the
//! others.

use sha2::{Digest, Sha256};

use super::id::SubagentId;
use super::supervisor::Provider;

/// Versions the byte layout hashed into a [`PairKey`], independent of any
/// other schema version in this crate.
pub(crate) const PAIR_KEY_SCHEMA_VERSION: u32 = 1;

/// Domain-separates pair-key hashing from any other SHA-256 use that might
/// be added to this crate later.
const PAIR_KEY_DOMAIN_PREFIX: &[u8] = b"subagent.pair-key.v1\n";

/// The default pair scope key from `docs/design.md` section 3.4: a SHA-256
/// digest binding the pair-key schema version, the canonical workspace
/// identity, the supervisor provider and session id, and the logical
/// `SubagentId` together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PairKey([u8; 32]);

impl PairKey {
    /// Computes the pair key for the current supervisor conversation scope.
    ///
    /// `workspace_identity_bytes` must be the raw identity bytes from
    /// [`super::workspace::WorkspaceRef::identity_bytes`], never a lossy
    /// text projection, so that a non-UTF-8 workspace path still produces a
    /// stable, correct key.
    pub(crate) fn compute(
        workspace_identity_bytes: &[u8],
        supervisor_provider: Provider,
        supervisor_session_id: &str,
        subagent_id: &SubagentId,
    ) -> PairKey {
        let mut hasher = Sha256::new();
        hasher.update(PAIR_KEY_DOMAIN_PREFIX);
        write_framed(&mut hasher, &PAIR_KEY_SCHEMA_VERSION.to_le_bytes());
        write_framed(&mut hasher, workspace_identity_bytes);
        write_framed(&mut hasher, supervisor_provider.to_string().as_bytes());
        write_framed(&mut hasher, supervisor_session_id.as_bytes());
        write_framed(&mut hasher, subagent_id.as_str().as_bytes());
        PairKey(hasher.finalize().into())
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> PairKey {
        PairKey(bytes)
    }

    pub(crate) fn to_hex(self) -> String {
        let mut hex = String::with_capacity(self.0.len() * 2);
        for byte in &self.0 {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

impl std::fmt::Display for PairKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl serde::Serialize for PairKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

/// Writes `bytes` prefixed with its own length as a `u64` little-endian
/// integer, so field boundaries are unambiguous regardless of content.
fn write_framed(hasher: &mut Sha256, bytes: &[u8]) {
    let length: u64 = bytes.len() as u64;
    hasher.update(length.to_le_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> SubagentId {
        SubagentId::parse(raw).unwrap()
    }

    /// Known-answer vectors computed independently (outside this crate) from
    /// the documented byte layout: the fixed domain prefix, then each field
    /// framed as `u64_le(len) || bytes` in the order schema version,
    /// workspace identity, provider, session id, subagent id. A change to
    /// this test's expected digests means the on-disk key derivation
    /// changed, which is a breaking change for every previously stored
    /// pair.
    #[test]
    fn fixed_vector_codex_pair() {
        let key = PairKey::compute(
            b"/workspace/example",
            Provider::Codex,
            "session-123",
            &id("reviewer"),
        );
        assert_eq!(
            key.to_hex(),
            "0994b96091ab0041cd03b0db8d4958365ca620ac2acaf4601f74041c96cb4196"
        );
    }

    #[test]
    fn fixed_vector_claude_pair_with_empty_workspace_bytes() {
        let key = PairKey::compute(b"", Provider::Claude, "s", &id("a"));
        assert_eq!(
            key.to_hex(),
            "e9bd03db7d27b3396597631f2eef4020c6a575645a1ef2d77cdd683a06132d2c"
        );
    }

    #[test]
    fn fixed_vector_claude_pair_with_realistic_fields() {
        let key = PairKey::compute(
            b"/home/user/project",
            Provider::Claude,
            "abc-DEF_123",
            &id("claude-opus-architect"),
        );
        assert_eq!(
            key.to_hex(),
            "2bb4af24397e7dc47875b93c493234485593120cdfebf5d7fb09a9253ac5279a"
        );
    }

    #[test]
    fn field_boundary_shift_between_workspace_and_session_id_changes_the_key() {
        let split_a = PairKey::compute(b"Xab", Provider::Codex, "Ysession", &id("reviewer"));
        let split_b = PairKey::compute(b"X", Provider::Codex, "abYsession", &id("reviewer"));
        assert_ne!(split_a, split_b);
    }

    #[test]
    fn field_boundary_shift_between_session_id_and_subagent_id_changes_the_key() {
        let split_a = PairKey::compute(b"/w", Provider::Codex, "ab", &id("c"));
        let split_b = PairKey::compute(b"/w", Provider::Codex, "a", &id("bc"));
        assert_ne!(split_a, split_b);
    }

    #[test]
    fn different_provider_changes_the_key_even_with_identical_other_fields() {
        let codex = PairKey::compute(b"/w", Provider::Codex, "session", &id("reviewer"));
        let claude = PairKey::compute(b"/w", Provider::Claude, "session", &id("reviewer"));
        assert_ne!(codex, claude);
    }

    #[test]
    fn different_workspace_identity_changes_the_key() {
        let a = PairKey::compute(b"/workspace/a", Provider::Codex, "session", &id("reviewer"));
        let b = PairKey::compute(b"/workspace/b", Provider::Codex, "session", &id("reviewer"));
        assert_ne!(a, b);
    }

    #[test]
    fn computation_is_deterministic() {
        let a = PairKey::compute(b"/w", Provider::Codex, "session", &id("reviewer"));
        let b = PairKey::compute(b"/w", Provider::Codex, "session", &id("reviewer"));
        assert_eq!(a, b);
    }

    #[test]
    fn hex_round_trips_through_from_bytes() {
        let key = PairKey::compute(b"/w", Provider::Codex, "session", &id("reviewer"));
        let round_tripped = PairKey::from_bytes(*key.as_bytes());
        assert_eq!(key, round_tripped);
        assert_eq!(key.to_hex(), round_tripped.to_hex());
    }
}
