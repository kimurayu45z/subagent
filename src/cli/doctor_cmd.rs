//! `subagent doctor`: an honest capability report.
//!
//! `doctor` itself does not mutate state. Its job is to report, per
//! capability, whether it is implemented, merely planned, or structurally
//! unavailable in this build.

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
}

impl CapabilityState {
    fn label(self) -> &'static str {
        match self {
            CapabilityState::Implemented => "implemented",
            CapabilityState::Planned => "planned",
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
            "resolves an explicit --supervisor codex:ID, claude:ID, or opencode:ID, or exactly one \
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
            "pair-identity-store",
            CapabilityState::Implemented,
            "the SQLite workspace/supervisor-session/pair identity tables (design.md section \
             10) are implemented: `subagent pairs` lists them read-only for the current \
             workspace, and a conversation-memory run idempotently ensures one row per scope",
        ),
        capability(
            "pair-exchange-ledger",
            CapabilityState::Implemented,
            "completed managed requests and stdout responses are recorded in the SQLite invocation ledger",
        ),
        capability(
            "pair-inheritance",
            CapabilityState::Implemented,
            "--inherit-from persists a bounded, one-way history edge between distinct pairs in the same workspace and supervisor conversation",
        ),
        capability(
            "child-session-store",
            CapabilityState::Implemented,
            "SQLite schema version 6 stores workstream-scoped provider-native child sessions for Claude Code, Codex, and OpenCode, lifecycle state, and versioned command-profile hashes",
        ),
        capability(
            "child-session-resume-claude",
            CapabilityState::Implemented,
            "Claude Code supports explicit --workstream with exactly one of --fresh or --resume; exact active-session and profile matching fail closed before spawn",
        ),
        capability(
            "child-session-resume-codex",
            CapabilityState::Implemented,
            "Codex supports explicit --workstream with exactly one of --fresh or --resume; JSONL observation persists and verifies the exact native thread ID",
        ),
        capability(
            "child-session-resume-opencode",
            CapabilityState::Implemented,
            "OpenCode supports explicit --workstream with exactly one of --fresh or --resume; JSONL observation persists and verifies the exact native session ID",
        ),
        capability(
            "task-request-projection",
            CapabilityState::Implemented,
            "request memory keeps the positional task prompt and caller stdin while excluding provider launch flags",
        ),
        capability(
            "context-capsule",
            CapabilityState::Implemented,
            "owner-only manifest, deterministic summary, pair-history JSONL, and available supervisor-history JSONL are materialized per invocation; pointer delivery is the default and inline delivery is explicit",
        ),
        capability(
            "redaction-common-credentials",
            CapabilityState::Implemented,
            "common credential assignments, bearer tokens, and known token prefixes are redacted; non-UTF-8 bodies are tagged as unscannable, and this is not a complete secret classifier",
        ),
        capability(
            "history-adapter-codex",
            CapabilityState::Implemented,
            "a bounded read-only Codex app-server thread/read adapter projects visible user and agent messages through an allowlist",
        ),
        capability(
            "history-adapter-claude",
            CapabilityState::Planned,
            "the Claude Code transcript/hook adapter (design.md section 8.2) is not implemented yet",
        ),
        capability(
            "history-adapter-opencode",
            CapabilityState::Planned,
            "the OpenCode supervisor-history adapter is not implemented yet; use an explicit opencode:SESSION_ID supervisor reference",
        ),
        capability(
            "summarizer-deterministic",
            CapabilityState::Implemented,
            "a bounded model-free summary of recent pair exchanges is materialized for pull-based reading and is injected through child stdin only with inline delivery",
        ),
        capability(
            "summarizer-model",
            CapabilityState::Implemented,
            "opt-in haiku and luna summarizers run only above a byte threshold with provider-side tools disabled where supported, bounded I/O, a hard timeout, and deterministic fallback",
        ),
        capability(
            "child-adapter-claude",
            CapabilityState::Implemented,
            "claude -p/--print is supported with argument-preserving stdin bootstrap injection and wrapper-managed --session-id/--resume injection; caller-native continuity flags remain rejected",
        ),
        capability(
            "child-adapter-codex",
            CapabilityState::Implemented,
            "codex exec is supported with argument-preserving stdin bootstrap injection; tracked workstreams add a bounded JSONL observation transport and exact native resume",
        ),
        capability(
            "child-adapter-opencode",
            CapabilityState::Implemented,
            "opencode run is supported with argument-preserving stdin bootstrap injection; tracked workstreams add a bounded JSONL observation transport and exact --session resume",
        ),
        capability(
            "child-spawn",
            CapabilityState::Implemented,
            "stdout/stderr forwarding and capture are bounded; tracked Codex and OpenCode JSONL is rendered after completion, while observation failures preserve captured output and child exit status",
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
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["kind"], "doctor");
        assert_eq!(value["status"], "ok");
        assert!(value["body"]["capabilities"].as_array().unwrap().len() >= capabilities().len());
    }

    #[test]
    fn child_spawn_capability_is_implemented() {
        let capabilities: Vec<Capability> = capabilities();
        let child_spawn: &Capability = capabilities
            .iter()
            .find(|c| c.name == "child-spawn")
            .unwrap();
        assert_eq!(child_spawn.state, CapabilityState::Implemented);
    }

    #[test]
    fn managed_claude_resume_capability_is_implemented() {
        let capabilities: Vec<Capability> = capabilities();
        let managed_resume: &Capability = capabilities
            .iter()
            .find(|capability| capability.name == "child-session-resume-claude")
            .unwrap();
        assert_eq!(managed_resume.state, CapabilityState::Implemented);
    }

    #[test]
    fn managed_codex_resume_capability_is_implemented() {
        let capabilities: Vec<Capability> = capabilities();
        let managed_resume: &Capability = capabilities
            .iter()
            .find(|capability| capability.name == "child-session-resume-codex")
            .unwrap();
        assert_eq!(managed_resume.state, CapabilityState::Implemented);
    }

    #[test]
    fn managed_opencode_resume_and_child_adapter_are_implemented() {
        let capabilities: Vec<Capability> = capabilities();
        for name in ["child-session-resume-opencode", "child-adapter-opencode"] {
            let capability: &Capability = capabilities
                .iter()
                .find(|capability: &&Capability| capability.name == name)
                .unwrap();
            assert_eq!(capability.state, CapabilityState::Implemented);
        }
        let history: &Capability = capabilities
            .iter()
            .find(|capability: &&Capability| capability.name == "history-adapter-opencode")
            .unwrap();
        assert_eq!(history.state, CapabilityState::Planned);
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
