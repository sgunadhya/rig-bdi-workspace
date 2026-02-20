pub mod commands;
pub mod runtime;
pub mod state;

use agent_core::event_log::{Event, EventType};
use agent_core::facts::{AlertFact, AlertSource, Fact, Severity};
use crate::state::{AppState, RuntimeChannels};
#[cfg(feature = "tauri-app")]
use tauri::Manager;
use std::sync::Arc;

pub fn build_state() -> Result<(AppState, RuntimeChannels), String> {
    let log = Arc::new(agent_core::event_log::EventLog::open("incidents.db")?);
    let (decision_tx, decision_rx) = std::sync::mpsc::channel();

    Ok((
        AppState { log, decision_tx },
        RuntimeChannels { decision_rx },
    ))
}

pub fn run() -> Result<(), String> {
    let (state, _channels) = build_state()?;
    runtime::start(&state);

    let _ = commands::list_incidents(&state)?;
    Ok(())
}

#[cfg(feature = "tauri-app")]
pub fn run_tauri() {
    tauri::Builder::default()
        .setup(|app| {
            let (state, channels) =
                build_state().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            if state
                .log
                .latest_event_id()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                .is_none()
            {
                seed_demo_data(&state).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }

            runtime::start_tauri_runtime(&state, channels, app.handle());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_incidents_cmd,
            commands::get_beliefs_cmd,
            commands::get_timeline_cmd,
            commands::get_current_plan_cmd,
            commands::get_tool_calls_cmd,
            commands::get_suggested_facts_cmd,
            commands::respond_to_escalation_cmd,
            commands::upsert_alert_fact_cmd,
            commands::retract_fact_cmd,
            commands::reprocess_incident_cmd,
            commands::generate_fact_suggestions_cmd,
            commands::decide_fact_suggestion_cmd
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub fn run_demo() -> Result<(), String> {
    let (state, _channels) = build_state()?;
    seed_demo_data(&state)?;

    let incidents = commands::list_incidents(&state)?;
    println!(
        "incidents:\n{}",
        serde_json::to_string_pretty(&incidents).map_err(|e| e.to_string())?
    );

    if let Some(first) = incidents.first() {
        let beliefs = commands::get_beliefs(&state, first.id.clone())?;
        println!(
            "beliefs:\n{}",
            serde_json::to_string_pretty(&beliefs).map_err(|e| e.to_string())?
        );

        let plan = commands::get_current_plan(&state, first.id.clone())?;
        println!(
            "plan:\n{}",
            serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?
        );

        let timeline = commands::get_timeline(&state, first.id.clone())?;
        println!(
            "timeline:\n{}",
            serde_json::to_string_pretty(&timeline).map_err(|e| e.to_string())?
        );
    }

    Ok(())
}

fn seed_demo_data(state: &AppState) -> Result<(), String> {
    let incident_id = "demo-incident-1".to_string();
    let fact = Fact::Alert(AlertFact {
        id: incident_id.clone(),
        source: AlertSource::Generic,
        severity: Severity::High,
        title: "Pod crashlooping".into(),
        tags: vec!["demo".into()],
        received_at: "1700000000".into(),
    });

    state.log.append(&Event {
        id: None,
        incident_id: incident_id.clone(),
        event_type: EventType::FactAsserted,
        description: "fact asserted".into(),
        details: Some(serde_json::to_value(fact).map_err(|e| e.to_string())?),
        timestamp: "1700000000".into(),
    })?;

    state.log.append(&Event {
        id: None,
        incident_id: incident_id.clone(),
        event_type: EventType::ActionIntent,
        description: "intent: inspect-pod-logs".into(),
        details: Some(serde_json::json!({
            "name": "inspect-pod-logs",
            "effect": "Observe",
            "status": "running"
        })),
        timestamp: "1700000001".into(),
    })?;

    state.log.append(&Event {
        id: None,
        incident_id,
        event_type: EventType::ActionResult,
        description: "action succeeded: inspect-pod-logs".into(),
        details: Some(serde_json::json!({
            "name": "inspect-pod-logs",
            "effect": "Observe",
            "status": "done"
        })),
        timestamp: "1700000002".into(),
    })?;

    Ok(())
}
