#[cfg(feature = "tauri-app")]
use crate::commands;
use crate::state::AppState;
#[cfg(feature = "tauri-app")]
use crate::state::{EscalationResponse, RuntimeChannels};
#[cfg(feature = "tauri-app")]
use agent_core::event_log::Event;
use agent_core::event_log::EventType;

pub trait EventSink: Send + Sync + 'static {
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

pub fn start(state: &AppState) {
    start_with_sink(state, NoopSink);
}

pub fn start_with_sink(state: &AppState, sink: impl EventSink) {
    let state_clone = state.clone();
    std::thread::spawn(move || {
        let mut last_id = state_clone.log.latest_event_id().ok().flatten().unwrap_or(0);

        loop {
            emit_updates(&state_clone, &sink, &mut last_id);
            std::thread::sleep(std::time::Duration::from_millis(750));
        }
    });
}

#[cfg(feature = "tauri-app")]
pub fn start_tauri_runtime(state: &AppState, channels: RuntimeChannels, app: tauri::AppHandle) {
    use tauri::Manager;

    start_with_sink(state, TauriSink::new(app.clone()));

    let (webhook_tx, webhook_stream) = agent_core::streams::webhook_channel(256);
    let (escalation_tx, escalation_rx) = std::sync::mpsc::channel();

    let log_for_agent = (*state.log).clone();
    std::thread::spawn(move || {
        let config = agent_core::agent::AgentConfig {
            max_replan_attempts: 3,
            runbooks: vec![
                ("crashloop_runbook", agent_core::runbooks::crashloop_runbook()),
                ("oomkill_runbook", agent_core::runbooks::oomkill_runbook()),
            ],
            all_actions: vec![
                agent_core::runbooks::crashloop_runbook(),
                agent_core::runbooks::oomkill_runbook(),
            ]
            .into_iter()
            .flatten()
            .collect(),
            goal_props: vec!["recovery_verified".into()],
            llm: build_llm_config_from_env(),
        };

        agent_core::agent::run_agent(webhook_stream, config, log_for_agent, escalation_tx, |_action| {
            Ok(serde_json::json!({"status": "ok"}))
        });
    });

    let app_for_escalation = app.clone();
    std::thread::spawn(move || {
        while let Ok(req) = escalation_rx.recv() {
            let _ = app_for_escalation.emit_all(
                "escalation-required",
                serde_json::json!({
                    "incident_id": req.incident_id,
                    "reason": req.reason,
                }),
            );
        }
    });

    let state_for_decisions = state.clone();
    std::thread::spawn(move || {
        while let Ok((incident_id, response)) = channels.decision_rx.recv() {
            let _ = commands::append_escalation_response_event(&state_for_decisions, &incident_id, &response);
            if let EscalationResponse::Approve = response {
                let _ = state_for_decisions.log.append(&Event {
                    id: None,
                    incident_id,
                    event_type: EventType::Resolved,
                    description: "resolved by human approval".into(),
                    details: None,
                    timestamp: now_string(),
                });
            }
        }
    });

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        runtime.block_on(async move {
            let app = agent_server::webhook::webhook_router(webhook_tx);
            let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
                .await
                .expect("bind :8080");
            let _ = axum::serve(listener, app).await;
        });
    });
}

fn emit_updates(state: &AppState, sink: &impl EventSink, last_id: &mut i64) {
    let active_count = state.log.active_incidents().map(|v| v.len()).unwrap_or(0);
    sink.emit_json(
        "beliefs-updated",
        serde_json::json!({ "active_incident_count": active_count }),
    );

    let Ok(events) = state.log.events_after(*last_id) else {
        return;
    };

    for e in events {
        if let Some(id) = e.id {
            if id > *last_id {
                *last_id = id;
            }
        }

        match e.event_type {
            EventType::PlanSelected => sink.emit_json(
                "plan-selected",
                serde_json::json!({"incident_id": e.incident_id, "description": e.description}),
            ),
            EventType::ActionResult => sink.emit_json(
                "action-completed",
                serde_json::json!({"incident_id": e.incident_id, "description": e.description}),
            ),
            EventType::Escalated => sink.emit_json(
                "escalation-required",
                serde_json::json!({"incident_id": e.incident_id, "description": e.description}),
            ),
            EventType::Resolved => sink.emit_json(
                "incident-resolved",
                serde_json::json!({"incident_id": e.incident_id, "description": e.description}),
            ),
            _ => {}
        }
    }
}

#[cfg(feature = "tauri-app")]
fn build_llm_config_from_env() -> Option<agent_core::llm::LlmConfig> {
    let api_key_env = std::env::var("LLM_API_KEY_ENV").unwrap_or_else(|_| "OPENAI_API_KEY".into());
    if std::env::var(&api_key_env).is_err() {
        return None;
    }

    Some(agent_core::llm::LlmConfig {
        provider: std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".into()),
        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
        api_key_env,
        temperature: std::env::var("LLM_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.2),
    })
}

#[cfg(feature = "tauri-app")]
fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "0".into();
    };
    duration.as_secs().to_string()
}

struct NoopSink;

impl EventSink for NoopSink {
    fn emit_json(&self, _event: &str, _payload: serde_json::Value) {}
}

#[cfg(feature = "tauri-app")]
pub struct TauriSink {
    app: tauri::AppHandle,
}

#[cfg(feature = "tauri-app")]
impl TauriSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

#[cfg(feature = "tauri-app")]
impl EventSink for TauriSink {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        use tauri::Manager;
        let _ = self.app.emit_all(event, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::event_log::{Event, EventLog, EventType};
    use std::sync::{Arc, Mutex};

    fn db_path(name: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        format!("/tmp/rig-bdi-tests/{name}-{nanos}.db")
    }

    #[derive(Default)]
    struct CaptureSink {
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl EventSink for CaptureSink {
        fn emit_json(&self, event: &str, _payload: serde_json::Value) {
            if let Ok(mut guard) = self.seen.lock() {
                guard.push(event.to_string());
            }
        }
    }

    #[test]
    fn emits_required_event_names() {
        let log = EventLog::open(&db_path("runtime-events")).expect("open");
        let incident_id = "inc-runtime";
        let seed = |event_type, description| Event {
            id: None,
            incident_id: incident_id.into(),
            event_type,
            description,
            details: None,
            timestamp: "1".into(),
        };

        log.append(&seed(EventType::FactAsserted, "fact".into()))
            .expect("append");
        log.append(&seed(EventType::PlanSelected, "plan".into()))
            .expect("append");
        log.append(&seed(EventType::ActionResult, "action".into()))
            .expect("append");
        log.append(&seed(EventType::Escalated, "escalate".into()))
            .expect("append");

        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        let sink = CaptureSink::default();
        let mut last_id = 0;
        emit_updates(&state, &sink, &mut last_id);

        let seen = sink.seen.lock().expect("lock").clone();
        assert!(seen.contains(&"beliefs-updated".into()));
        assert!(seen.contains(&"plan-selected".into()));
        assert!(seen.contains(&"action-completed".into()));
        assert!(seen.contains(&"escalation-required".into()));
    }
}
