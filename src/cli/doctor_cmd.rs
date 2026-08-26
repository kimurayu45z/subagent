//! `subagent doctor`: an honest capability report.
//!
//! Nothing in this build mutates state, so `doctor` always succeeds; its
//! job is to report, per capability, whether it is implemented, merely
//! planned, or structurally unavailable in this build.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use super::report::{Report, ReportStatus};
use super::{OutputFormat, handle_clap_error};

#[derive(Debug, Clone, Parser)]
#[command(name = "subagent-doctor", no_binary_name = true)]
struct DoctorArgs {
    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapabilityState {
    Implemented,
    Planned,
    Unavailable,
}

impl CapabilityState {
    fn label(self) -> &'static str {
        match self {
            CapabilityState::Implemented => "implemented",
            CapabilityState::Planned => "planned",
            CapabilityState::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Capability {
    name: String,
    state: CapabilityState,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct DoctorBody {
    capabilities: Vec<Capability>,
}

fn capability(name: &str, state: CapabilityState, detail: &str) -> Capability {
    Capability {
        name: name.to_string(),
        state,
        detail: detail.to_string(),
    }
}

fn capabilities() -> Vec<Capability> {
    vec![
        capability(
            "cli-grammar",
            CapabilityState::Implemented,
            "argument parsing, the explicit `--` boundary, and OsString-preserving child argv",
        ),
        capability(
            "dry-run-plan",
            CapabilityState::Implemented,
            "resolves and displays the run plan without spawning a child",
        ),
        capability(
            "supervisor-detection-explicit-native",
            CapabilityState::Implemented,
            "resolves an explicit --supervisor codex:ID or claude:ID, or exactly one \
             unambiguous, non-empty CODEX_THREAD_ID or CLAUDE_CODE_SESSION_ID (design.md \
             section 5, steps 1 and 3)",
        ),
        capability(
            "supervisor-detection-managed-ref",
            CapabilityState::Planned,
            "SUBAGENT_SELF_REF managed-parent resolution (design.md section 5, step 2) is \
             not implemented and currently fails closed",
        ),
        capability(
            "supervisor-detection-hook-registry",
            CapabilityState::Planned,
            "provider hook-registry resolution (design.md section 5, step 4) is not \
             implemented yet",
        ),
        capability(
            "pair-ledger",
            CapabilityState::Planned,
            "the SQLite pair ledger (design.md section 10) is not implemented yet",
        ),
        capability(
            "context-capsule",
            CapabilityState::Planned,
            "context capsule materialization (design.md section 11) is not implemented yet",
        ),
        capability(
            "history-adapter-codex",
            CapabilityState::Planned,
            "the Codex app-server thread/read adapter (design.md section 8.1) is not implemented yet",
        ),
        capability(
            "history-adapter-claude",
            CapabilityState::Planned,
            "the Claude Code transcript/hook adapter (design.md section 8.2) is not implemented yet",
        ),
        capability(
            "summarizer-deterministic",
            CapabilityState::Planned,
            "the deterministic summarizer (design.md section 12.1) is not implemented yet",
        ),
        capability(
            "summarizer-model",
            CapabilityState::Planned,
            "the optional model summarizer (design.md section 12.2) is not implemented yet",
        ),
        capability(
            "child-adapter-claude",
            CapabilityState::Planned,
            "Claude Code child session assignment and resume (design.md section 13.1) is not implemented yet",
        ),
        capability(
            "child-adapter-codex",
            CapabilityState::Unavailable,
            "managed Codex execution requires app-server support that is not implemented; native resume is reported unavailable per design.md section 13.2",
        ),
        capability(
            "child-spawn",
            CapabilityState::Unavailable,
            "an ordinary managed run currently exits before spawning any child process (exit 125)",
        ),
    ]
}

pub(crate) fn execute(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let doctor_args: DoctorArgs = match DoctorArgs::try_parse_from(args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(clap_error) => return handle_clap_error(clap_error, out, err),
    };

    let body = DoctorBody {
        capabilities: capabilities(),
    };

    match doctor_args.format {
        OutputFormat::Json => {
            let report = Report::new("doctor", ReportStatus::Ok, body);
            let _ = writeln!(out, "{}", report.to_json_pretty());
        }
        OutputFormat::Text => {
            let _ = writeln!(out, "subagent doctor");
            for capability in &body.capabilities {
                let _ = writeln!(
                    out,
                    "  [{:<12}] {:<26} {}",
                    capability.state.label(),
                    capability.name,
                    capability.detail
                );
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_format_lists_every_capability_name() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&[], &mut out, &mut err);
        assert_eq!(code, ExitCode::SUCCESS);
        let text = String::from_utf8(out).unwrap();
        for capability in capabilities() {
            assert!(
                text.contains(&capability.name),
                "missing {}",
                capability.name
            );
        }
    }

    #[test]
    fn json_format_is_a_valid_versioned_report() {
        let args: Vec<OsString> = vec![OsString::from("--format"), OsString::from("json")];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, ExitCode::SUCCESS);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["kind"], "doctor");
        assert_eq!(value["status"], "ok");
        assert!(value["body"]["capabilities"].as_array().unwrap().len() >= capabilities().len());
    }

    #[test]
    fn child_spawn_capability_is_explicitly_unavailable() {
        let capabilities: Vec<Capability> = capabilities();
        let child_spawn: &Capability = capabilities
            .iter()
            .find(|c| c.name == "child-spawn")
            .unwrap();
        assert_eq!(child_spawn.state, CapabilityState::Unavailable);
    }

    #[test]
    fn supervisor_detection_reports_implemented_and_planned_parts_separately() {
        let capabilities: Vec<Capability> = capabilities();
        let explicit_native: &Capability = capabilities
            .iter()
            .find(|capability| capability.name == "supervisor-detection-explicit-native")
            .unwrap();
        let managed_ref: &Capability = capabilities
            .iter()
            .find(|capability| capability.name == "supervisor-detection-managed-ref")
            .unwrap();
        let hook_registry: &Capability = capabilities
            .iter()
            .find(|capability| capability.name == "supervisor-detection-hook-registry")
            .unwrap();

        assert_eq!(explicit_native.state, CapabilityState::Implemented);
        assert_eq!(managed_ref.state, CapabilityState::Planned);
        assert_eq!(hook_registry.state, CapabilityState::Planned);
    }
}
