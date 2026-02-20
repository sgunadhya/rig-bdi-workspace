# Rig BDI Workspace (Alpha)

Effect-aware BDI incident response prototype with:
- deterministic rules/runbooks
- optional Rig uncertain path (LLM interpret/propose)
- event-sourced timeline and tool traces
- Tauri + Leptos validation app

Status: alpha prototype. Suitable for evaluation and extension, not production hardening.

## What Works

- `rig-effects` crate for effect semantics and recovery classification.
- `agent-core` BDI loop with deterministic planning/execution and WAL event log.
- optional Rig LLM path for unknown patterns (`llm: Some(config)`).
- `agent-server` webhook ingestion and headless runtime.
- `src-tauri` + `incident-ui` desktop validator:
- incident list, timeline, beliefs, plan, tool usage, escalation response
- fact upsert/retract
- "Re-run Pipeline" action for existing incident facts
- fact registry + adapters:
- canonical `alert.v1` validation (`fact-registry`)
- adapters for `generic` and `alertmanager` webhook payloads

## Repo Layout

- `fact-registry`: canonical fact schema and validation.
- `rig-effects`, `rig-effects-derive`: effect model and derive support.
- `agent-core`: facts, rules, planner, executor, event log, Rig wiring.
- `agent-server`: webhook adapters and server.
- `src-tauri`: Tauri backend runtime + commands.
- `incident-ui`: Leptos frontend (built into `ui/` by `trunk`).

## Quick Start

Requirements:
- Rust stable
- `trunk` (`cargo install trunk`)
- system dependencies required by Tauri on your OS

Run desktop app:

```bash
make run-app
```

Send sample generic incident:

```bash
make sample-webhook
```

Send sample Alertmanager incident:

```bash
curl -X POST http://127.0.0.1:8080/webhook/alertmanager \
  -H 'Content-Type: application/json' \
  -d '{
    "alerts":[
      {
        "fingerprint":"fp-1",
        "labels":{"alertname":"PodCrashLooping","severity":"high","namespace":"prod"},
        "annotations":{"summary":"checkout crashlooping"}
      }
    ]
  }'
```

## LLM (Rig) Configuration

The uncertain path is enabled only when an API key env var exists.

Supported env vars:
- `LLM_PROVIDER` (default: `openai`)
- `LLM_MODEL` (default: `gpt-4o-mini`)
- `LLM_API_KEY_ENV` (default: `OPENAI_API_KEY`)
- `LLM_TEMPERATURE` (default: `0.2`)
- `OPENAI_BASE_URL` (optional OpenAI-compatible endpoint)

OpenAI:

```bash
export OPENAI_API_KEY=...
make run-app
```

LM Studio (OpenAI-compatible server):

```bash
export OPENAI_API_KEY=lm-studio
export OPENAI_BASE_URL=http://127.0.0.1:1234/v1
export LLM_MODEL=<loaded-model-name>
make run-app
```

## Canonical Fact Contract

Current canonical alert schema is `alert.v1`:

```json
{
  "schema": "alert.v1",
  "id": "inc-123",
  "title": "Pod crashlooping",
  "severity": "high",
  "tags": ["k8s", "crashloop"],
  "source": "generic",
  "occurred_at": "1730000000"
}
```

Validation is implemented in `fact-registry/src/lib.rs`.

## Commands and Affordances

Tauri commands:
- `list_incidents_cmd`
- `get_beliefs_cmd`
- `get_timeline_cmd`
- `get_current_plan_cmd`
- `get_tool_calls_cmd`
- `respond_to_escalation_cmd`
- `upsert_alert_fact_cmd`
- `retract_fact_cmd`
- `reprocess_incident_cmd`

Realtime events:
- `beliefs-updated`
- `plan-selected`
- `action-completed`
- `escalation-required`
- `incident-resolved`

## Security and Production Notes

- No auth on webhook routes by default.
- No rate limiting or tenancy isolation yet.
- No secrets management integration yet.
- Policy gating is basic; irreversible action governance is incomplete.
- Adapter library and fact registry are early-stage.

Treat this repository as an architecture and integration baseline.

## License

MIT (`LICENSE`).
