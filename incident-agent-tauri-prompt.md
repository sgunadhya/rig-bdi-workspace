# Incident Response Agent — Tauri + Leptos UI Specification

## Prerequisites

This project wraps `agent-core` and `agent-server` from the companion `bdi-agent-architecture-prompt.md`. The agent must work headless first. This UI is optional.

## Architecture

```
Tauri Process
├── agent-core BDI loop (background tokio task)
│   ├── emits Tauri events → Leptos frontend subscribes
│   └── receives human decisions ← Leptos frontend invokes
├── axum webhook server (:8080)
└── #[tauri::command] handlers ← Leptos frontend invokes
```

Three crossing points:
1. **Tauri events** (push): `beliefs-updated`, `plan-selected`, `action-completed`, `escalation-required`, `incident-resolved`
2. **Tauri commands** (request/response): `list_incidents`, `get_beliefs`, `get_timeline`, `get_current_plan`, `respond_to_escalation`
3. **DTOs** (shared types): serde structs crossing the WASM bridge

## Project Structure

```
incident-agent/
├── Cargo.toml                        # workspace (includes rig-effects, agent-core, agent-server)
├── src/                              # Leptos frontend (WASM)
│   ├── main.rs
│   ├── app.rs
│   ├── bridge.rs                     # typed invoke/listen wrappers
│   ├── dto.rs                        # shared DTOs
│   └── components/
│       ├── dashboard.rs
│       ├── incident_list.rs
│       ├── belief_viewer.rs
│       ├── plan_viewer.rs
│       ├── timeline.rs
│       ├── escalation_panel.rs
│       └── metrics_panel.rs
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs
│       ├── state.rs
│       ├── runtime.rs
│       └── commands.rs               # all commands in one file (small surface)
└── index.html
```

## Shared DTOs (src/dto.rs)

Used by both Tauri commands and Leptos components.

```rust
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IncidentDto {
    pub id: String,
    pub status: String,          // "active", "resolved", "escalated"
    pub severity: String,
    pub title: String,
    pub started_at: String,
    pub current_phase: String,   // "observing", "matching", "planning", "executing", "escalating"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactDto {
    pub fact_type: String,       // "Pod", "Alert", "Deploy", "Metric"
    pub summary: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanDto {
    pub steps: Vec<PlanStepDto>,
    pub current_step: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanStepDto {
    pub name: String,
    pub effect: String,          // "Pure", "Observe", "Mutate", "Irreversible"
    pub status: String,          // "pending", "running", "done", "failed", "compensated"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimelineEventDto {
    pub id: i64,
    pub event_type: String,
    pub description: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EscalationResponse {
    Approve,
    Reject { reason: String },
    TakeOver,
}
```

## Tauri Backend

### state.rs

```rust
use std::sync::Arc;
use tokio::sync::mpsc;
use agent_core::event_log::EventLog;

pub struct AppState {
    pub log: Arc<EventLog>,
    pub decision_tx: mpsc::Sender<(String, EscalationResponse)>, // (incident_id, response)
}
```

### runtime.rs — Spawn Agent + Forward Events

```rust
use tauri::{AppHandle, Manager};

pub fn start(app: AppHandle, log: Arc<EventLog>) {
    let (webhook_tx, webhook_stream) = agent_core::streams::webhook_channel(256);
    let (escalation_tx, mut escalation_rx) = tokio::sync::mpsc::channel(32);

    // Webhook server
    tokio::spawn(async move {
        let router = agent_server::webhook::webhook_router(webhook_tx);
        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
        axum::serve(listener, router).await.unwrap();
    });

    // Forward escalations → frontend
    let handle = app.clone();
    tokio::spawn(async move {
        while let Some(req) = escalation_rx.recv().await {
            let _ = handle.emit("escalation-required", &req);
        }
    });

    // BDI agent
    tokio::spawn(async move {
        agent_core::agent::run_agent(
            webhook_stream,
            agent_core::agent::AgentConfig {
                max_replan_attempts: 3,
                runbooks: vec![
                    ("crashloop_runbook", agent_core::runbooks::crashloop_runbook()),
                    ("oomkill_runbook", agent_core::runbooks::oomkill_runbook()),
                ],
                goal_props: vec!["recovery_verified".into()],
            },
            agent_core::event_log::EventLog::open("incidents.db").unwrap(),
            escalation_tx,
            |action| Box::pin(async move {
                Ok(serde_json::json!({"status": "ok"}))
            }),
        ).await;
    });
}
```

### commands.rs — All Commands (Thin Surface)

```rust
use tauri::State;
use crate::state::AppState;
use shared::dto::*;

#[tauri::command]
async fn list_incidents(state: State<'_, AppState>) -> Result<Vec<IncidentDto>, String> {
    state.log.active_incidents()
        .map(|ids| ids.into_iter().map(|id| IncidentDto { id, ..Default::default() }).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_timeline(state: State<'_, AppState>, incident_id: String) -> Result<Vec<TimelineEventDto>, String> {
    state.log.events_for_incident(&incident_id)
        .map(|events| events.into_iter().map(|e| TimelineEventDto {
            id: e.id.unwrap_or(0),
            event_type: format!("{:?}", e.event_type),
            description: e.description,
            timestamp: e.timestamp.to_rfc3339(),
        }).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn respond_to_escalation(
    state: State<'_, AppState>,
    incident_id: String,
    response: EscalationResponse,
) -> Result<(), String> {
    state.decision_tx.send((incident_id, response)).await.map_err(|e| e.to_string())
}
```

### lib.rs — Registration

```rust
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let log = Arc::new(agent_core::event_log::EventLog::open("incidents.db")?);
            let (decision_tx, _decision_rx) = tokio::sync::mpsc::channel(32);
            let state = AppState { log: log.clone(), decision_tx };
            app.manage(state);
            runtime::start(app.handle().clone(), log);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_incidents,
            commands::get_timeline,
            commands::respond_to_escalation,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri");
}
```

## Leptos Frontend

### bridge.rs — Typed Wrappers

```rust
use wasm_bindgen::prelude::*;
use serde::{Serialize, de::DeserializeOwned};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> JsValue;
}

pub async fn call<A: Serialize, R: DeserializeOwned>(cmd: &str, args: &A) -> Result<R, String> {
    let args = serde_wasm_bindgen::to_value(args).map_err(|e| e.to_string())?;
    let result = invoke(cmd, args).await;
    serde_wasm_bindgen::from_value(result).map_err(|e| e.to_string())
}

pub async fn fetch_incidents() -> Result<Vec<IncidentDto>, String> {
    call("list_incidents", &()).await
}

pub async fn fetch_timeline(id: &str) -> Result<Vec<TimelineEventDto>, String> {
    call("get_timeline", &serde_json::json!({"incidentId": id})).await
}

pub async fn submit_escalation(id: &str, response: EscalationResponse) -> Result<(), String> {
    call("respond_to_escalation", &serde_json::json!({
        "incidentId": id, "response": response
    })).await
}
```

### Key Components

**dashboard.rs** — Three-panel layout: incidents list (left), selected incident detail (center), beliefs (right).

**incident_list.rs** — Fetches via `fetch_incidents()`, listens for `incident-resolved` Tauri events to refresh reactively.

**belief_viewer.rs** — Shows current Ascent-derived facts as a live table. Refreshes on `beliefs-updated` event.

**plan_viewer.rs** — Shows current plan steps with effect type badges (color-coded: green=Pure, blue=Observe, orange=Mutate, red=Irreversible). Current step highlighted. Failed/compensated steps marked.

**timeline.rs** — Vertical timeline of EventLog entries for selected incident. Each event has a colored dot by type. Auto-scrolls to latest.

**escalation_panel.rs** — Appears when `escalation-required` event fires. Shows reason + summary. Three buttons: Approve, Reject (with reason input), Take Over. Sends decision via `submit_escalation()`.

**metrics_panel.rs** — Aggregate stats: total incidents, MTTR, resolution rate, escalation rate. Computed from EventLog queries.

## Build & Run

```bash
cargo install create-tauri-app --locked
cargo create-tauri-app incident-agent  # Select Rust + Leptos

# Add workspace deps
# Copy rig-effects, agent-core, agent-server as path deps

cargo tauri dev    # development
cargo tauri build  # production
```

## Implementation Order

1. **Skip UI until agent-server works headless.** Verify BDI loop via curl + logs first.
2. **Tauri scaffold** — create-tauri-app with Leptos, verify blank app runs.
3. **state.rs + runtime.rs** — wire agent into Tauri lifecycle.
4. **commands.rs** — implement list_incidents and get_timeline first.
5. **bridge.rs + dto.rs** — typed frontend bridge.
6. **incident_list.rs + timeline.rs** — first visible components.
7. **escalation_panel.rs** — the most important interaction (human-in-the-loop).
8. **belief_viewer.rs + plan_viewer.rs** — observability.
9. **metrics_panel.rs** — last, computed from existing data.

## Key Principles

- **UI is read-heavy, write-light.** Most interaction is observing. Only escalation responses are writes.
- **Tauri events for reactivity.** No polling. Agent pushes state changes.
- **DTOs separate concerns.** Frontend never imports agent-core types directly.
- **Effect types visible in UI.** Color-coded badges on plan steps make the effect system tangible for operators.
- **Escalation is the critical path.** The panel must be clear, fast, and hard to misclick.
