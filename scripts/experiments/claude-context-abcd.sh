#!/usr/bin/env bash
set -euo pipefail

# Controlled A-D mechanism benchmark for direct native resume, managed resume,
# pointer delivery, and inline delivery. The experiment never cleans up its
# temporary root so failed and successful runs remain inspectable.

require_command() {
    local command_name="$1"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        printf 'missing required command: %s\n' "$command_name" >&2
        exit 127
    fi
}

require_command claude
require_command subagent
require_command uuidgen

model="${CLAUDE_MODEL:-haiku}"
supervisor="codex:claude-context-abcd-20260828"
experiment_root="$(mktemp -d "${TMPDIR:-/tmp}/subagent-claude-context-20260828.XXXXXX")"
workspace="$experiment_root/workspace"
provider_state="$experiment_root/claude-config"
fact_uuid="$(uuidgen | tr '[:upper:]' '[:lower:]')"
fact="ContextLease${fact_uuid%%-*}"

mkdir -p "$workspace" "$provider_state"
chmod 700 "$experiment_root" "$workspace" "$provider_state"

# Keep provider transcripts out of the normal Claude configuration directory.
# Authentication is referenced, not copied or modified. Safe mode disables
# project/user customizations while retaining normal authenticated execution.
credential_file="${HOME}/.claude/.credentials.json"
if [[ ! -r "$credential_file" ]]; then
    printf 'Claude credential file is not readable: %s\n' "$credential_file" >&2
    printf 'preserved experiment root: %s\n' "$experiment_root" >&2
    exit 126
fi
ln -s "$credential_file" "$provider_state/.credentials.json"

for state_name in b c d; do
    mkdir -p "$experiment_root/state-$state_name"
    chmod 700 "$experiment_root/state-$state_name"
done

unset CODEX_THREAD_ID
unset CLAUDE_CODE_SESSION_ID
unset SUBAGENT_SELF_REF
unset SUBAGENT_CHAIN_ID
unset SUBAGENT_DEPTH

common_claude_args=(
    --model "$model"
    --output-format json
    --permission-mode dontAsk
    --tools Read
    --allowedTools Read
    --disallowedTools Edit Write Bash 'mcp__*'
    --max-turns 4
    --max-budget-usd 0.50
    --disable-slash-commands
    --safe-mode
)

run_case() {
    local label="$1"
    shift
    local started_at="$SECONDS"
    local child_exit=0

    printf 'starting %s\n' "$label"
    set +e
    "$@" >"$experiment_root/$label.stdout" 2>"$experiment_root/$label.stderr"
    child_exit=$?
    set -e
    printf '%s\n' "$child_exit" >"$experiment_root/$label.exit"
    printf '%s\n' "$((SECONDS - started_at))" >"$experiment_root/$label.wall-seconds"
    printf 'finished %s: exit=%s wall=%ss\n' \
        "$label" "$child_exit" "$((SECONDS - started_at))"
}

printf '%s\n' "$fact" >"$experiment_root/fact.txt"
printf 'model=%s\nsupervisor=%s\nworkspace=%s\n' \
    "$model" "$supervisor" "$workspace" >"$experiment_root/experiment.env"

cd "$workspace"
export CLAUDE_CONFIG_DIR="$provider_state"

# A: direct Claude native session continuity.
direct_session_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
run_case a1-direct-fresh \
    claude -p \
    "Act as a read-only Rust API reviewer. The fixture accepted the type identifier '$fact'. Confirm by replying with exactly that identifier and nothing else." \
    --session-id "$direct_session_id" "${common_claude_args[@]}"
run_case a2-direct-resume \
    claude -p \
    "Reply with exactly the accepted Rust type identifier from the previous turn and nothing else." \
    --resume "$direct_session_id" "${common_claude_args[@]}"

# B: the same native continuity through the wrapper, with no pair/supervisor
# context materialization.
run_case b1-managed-fresh \
    env SUBAGENT_STATE_DIR="$experiment_root/state-b" \
    subagent --id claude-haiku-context-benchmark \
    --supervisor "$supervisor" --context none \
    --workstream managed-resume --fresh -- \
    claude -p \
    "Act as a read-only Rust API reviewer. The fixture accepted the type identifier '$fact'. Confirm by replying with exactly that identifier and nothing else." \
    "${common_claude_args[@]}"
run_case b2-managed-resume \
    env SUBAGENT_STATE_DIR="$experiment_root/state-b" \
    subagent --id claude-haiku-context-benchmark \
    --supervisor "$supervisor" --context none \
    --workstream managed-resume --resume -- \
    claude -p \
    "Reply with exactly the accepted Rust type identifier from the previous turn and nothing else." \
    "${common_claude_args[@]}"

# C: create pair evidence, then start a separate native session with pointer
# delivery. The task explicitly asks Claude to pull summary.md with Read.
run_case c1-pointer-seed \
    env SUBAGENT_STATE_DIR="$experiment_root/state-c" \
    subagent --id claude-haiku-context-benchmark \
    --supervisor "$supervisor" --context none -- \
    claude -p \
    "Act as a read-only Rust API reviewer. The fixture accepted the type identifier '$fact'. Confirm by replying with exactly that identifier and nothing else." \
    "${common_claude_args[@]}" --no-session-persistence
run_case c2-pointer-read \
    env SUBAGENT_STATE_DIR="$experiment_root/state-c" \
    subagent --id claude-haiku-context-benchmark \
    --supervisor "$supervisor" --context pair --context-delivery pointer -- \
    claude -p \
    "Use Read on summary.md in the context capsule path supplied in the bootstrap. Return exactly the previously accepted Rust type identifier and nothing else. If Read is denied or the identifier is absent, return exactly CAPSULE_UNAVAILABLE." \
    "${common_claude_args[@]}" --no-session-persistence

# D: create the same pair evidence, then start a separate native session with
# the bounded summary included in the bootstrap.
run_case d1-inline-seed \
    env SUBAGENT_STATE_DIR="$experiment_root/state-d" \
    subagent --id claude-haiku-context-benchmark \
    --supervisor "$supervisor" --context none -- \
    claude -p \
    "Act as a read-only Rust API reviewer. The fixture accepted the type identifier '$fact'. Confirm by replying with exactly that identifier and nothing else." \
    "${common_claude_args[@]}" --no-session-persistence
run_case d2-inline-read \
    env SUBAGENT_STATE_DIR="$experiment_root/state-d" \
    subagent --id claude-haiku-context-benchmark \
    --supervisor "$supervisor" --context pair --context-delivery inline -- \
    claude -p \
    "Return exactly the previously accepted Rust type identifier from the inline context bootstrap and nothing else. If it is absent, return exactly CAPSULE_UNAVAILABLE." \
    "${common_claude_args[@]}" --no-session-persistence

printf '%s\n' "$experiment_root" >"${TMPDIR:-/tmp}/subagent-claude-context-latest"
printf 'preserved experiment root: %s\n' "$experiment_root"
