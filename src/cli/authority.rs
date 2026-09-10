//! Provider-neutral authority profiles for unattended child execution.
//!
//! The wrapper owns the user-facing grant. Provider adapters translate it to
//! their current native CLI flags so broad authority is explicit in plans,
//! reports, command digests, and native-session compatibility checks.

use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AuthorityProfile {
    /// Preserve the provider's existing configuration and caller arguments.
    #[default]
    Inherit,
    /// Grant the provider's broad non-interactive tool authority.
    Full,
}

impl fmt::Display for AuthorityProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: &str = match self {
            AuthorityProfile::Inherit => "inherit",
            AuthorityProfile::Full => "full",
        };
        formatter.write_str(text)
    }
}
