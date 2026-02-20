use crate::state::{AppState, EscalationResponse};
use agent_core::event_log::{Event, EventType};
use agent_core::facts::{AlertFact, AlertSource, Fact, Severity};
use agent_core::llm;
use agent_core::{executor, planner, rules, runbooks};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IncidentDto {
    pub id: String,
    pub status: String,
    pub severity: String,
    pub title: String,
    pub started_at: String,
    pub current_phase: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactDto {
    pub fact_id: String,
    pub fact_type: String,
    pub summary: String,
    pub severity: String,
    pub tags: Vec<String>,
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
    pub effect: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimelineEventDto {
    pub id: i64,
    pub event_type: String,
    pub description: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallDto {
    pub event_id: i64,
    pub incident_id: String,
    pub tool_name: String,
    pub phase: String,
    pub status: String,
    pub effect: String,
    pub summary: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuggestedFactDto {
    pub suggestion_event_id: i64,
    pub fact_id: String,
    pub summary: String,
    pub severity: String,
    pub tags: Vec<String>,
    pub rationale: String,
    pub timestamp: String,
}

pub fn list_incidents(state: &AppState) -> Result<Vec<IncidentDto>, String> {
    let ids = state.log.all_incidents()?;

    let mut out = Vec::new();
    for id in ids {
        out.push(summarize_incident(state, id)?);
    }
    Ok(out)
}

pub fn get_beliefs(state: &AppState, incident_id: String) -> Result<Vec<FactDto>, String> {
    state.log.events_for_incident(&incident_id).map(materialize_facts)
}

pub fn get_timeline(state: &AppState, incident_id: String) -> Result<Vec<TimelineEventDto>, String> {
    state.log.events_for_incident(&incident_id).map(|events| {
        events
            .into_iter()
            .map(|e| TimelineEventDto {
                id: e.id.unwrap_or(0),
                event_type: format!("{:?}", e.event_type),
                description: e.description,
                timestamp: e.timestamp,
            })
            .collect()
    })
}

pub fn get_current_plan(state: &AppState, incident_id: String) -> Result<PlanDto, String> {
    let events = state.log.events_for_incident(&incident_id)?;

    let mut steps: Vec<PlanStepDto> = Vec::new();
    for e in events {
        if !matches!(
            e.event_type,
            EventType::ActionIntent | EventType::ActionResult | EventType::Escalated
        ) {
            continue;
        }

        let Some(v) = e.details else {
            continue;
        };

        let Some(name) = v.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let effect = v
            .get("effect")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Observe");
        let status = v
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pending");

        if let Some(existing) = steps.iter_mut().rev().find(|s| s.name == name) {
            existing.status = status.to_string();
            existing.effect = effect.to_string();
        } else {
            steps.push(PlanStepDto {
                name: name.to_string(),
                effect: effect.to_string(),
                status: status.to_string(),
            });
        }
    }

    let current_step = steps
        .iter()
        .position(|step| step.status == "running")
        .unwrap_or_else(|| steps.len().saturating_sub(1));

    Ok(PlanDto { steps, current_step })
}

pub fn get_tool_calls(state: &AppState, incident_id: String) -> Result<Vec<ToolCallDto>, String> {
    let events = state.log.events_for_incident(&incident_id)?;
    let mut out = Vec::new();

    for e in events {
        if !matches!(e.event_type, EventType::ActionIntent | EventType::ActionResult) {
            continue;
        }
        let Some(v) = e.details else {
            continue;
        };

        let tool_name = v
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let effect = v
            .get("effect")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Observe")
            .to_string();
        let status = v
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pending")
            .to_string();
        let phase = if matches!(e.event_type, EventType::ActionIntent) {
            "intent".to_string()
        } else {
            "result".to_string()
        };

        out.push(ToolCallDto {
            event_id: e.id.unwrap_or(0),
            incident_id: e.incident_id,
            tool_name,
            phase,
            status,
            effect,
            summary: e.description,
            timestamp: e.timestamp,
        });
    }

    Ok(out)
}

pub fn get_suggested_facts(
    state: &AppState,
    incident_id: String,
) -> Result<Vec<SuggestedFactDto>, String> {
    let events = state.log.events_for_incident(&incident_id)?;
    let mut active: BTreeMap<i64, SuggestedFactDto> = BTreeMap::new();

    for e in events {
        match e.event_type {
            EventType::FactSuggested => {
                let Some(id) = e.id else {
                    continue;
                };
                let Some(details) = e.details else {
                    continue;
                };
                active.insert(
                    id,
                    SuggestedFactDto {
                        suggestion_event_id: id,
                        fact_id: details
                            .get("fact_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("suggested")
                            .to_string(),
                        summary: details
                            .get("title")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("Suggested fact")
                            .to_string(),
                        severity: details
                            .get("severity")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("high")
                            .to_string(),
                        tags: details
                            .get("tags")
                            .and_then(serde_json::Value::as_array)
                            .map(|xs| {
                                xs.iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(ToString::to_string)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                        rationale: details
                            .get("rationale")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("llm suggestion")
                            .to_string(),
                        timestamp: e.timestamp,
                    },
                );
            }
            EventType::FactSuggestionResolved => {
                if let Some(details) = e.details {
                    if let Some(id) = details
                        .get("suggestion_event_id")
                        .and_then(serde_json::Value::as_i64)
                    {
                        active.remove(&id);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(active.into_values().collect())
}

pub fn generate_fact_suggestions(state: &AppState, incident_id: String) -> Result<(), String> {
    let facts = materialize_fact_map(state.log.events_for_incident(&incident_id)?);
    let current_facts = facts.into_values().map(|(f, _)| f).collect::<Vec<_>>();
    if current_facts.is_empty() {
        return Err("no active facts for incident".into());
    }

    let llm_cfg = build_llm_config_from_env()
        .ok_or_else(|| "LLM is not configured (missing API key env)".to_string())?;
    let suggestions = llm::suggest_facts(&llm_cfg, &current_facts)?;

    for s in suggestions {
        state.log.append(&Event {
            id: None,
            incident_id: incident_id.clone(),
            event_type: EventType::FactSuggested,
            description: format!("llm suggested fact: {}", s.fact_id),
            details: Some(serde_json::json!({
                "fact_id": s.fact_id,
                "title": s.title,
                "severity": s.severity,
                "tags": s.tags,
                "rationale": s.rationale
            })),
            timestamp: now_string(),
        })?;
    }
    Ok(())
}

pub fn decide_fact_suggestion(
    state: &AppState,
    incident_id: String,
    suggestion_event_id: i64,
    decision: String,
) -> Result<(), String> {
    let events = state.log.events_for_incident(&incident_id)?;
    let suggested = events.into_iter().find(|e| {
        e.id == Some(suggestion_event_id) && matches!(e.event_type, EventType::FactSuggested)
    });
    let Some(suggested) = suggested else {
        return Err("suggestion event not found".into());
    };

    if decision.to_lowercase() == "approve" {
        let Some(details) = suggested.details else {
            return Err("suggestion payload missing".into());
        };
        let fact = Fact::Alert(AlertFact {
            id: details
                .get("fact_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("suggested")
                .to_string(),
            source: AlertSource::Generic,
            severity: parse_severity(
                details
                    .get("severity")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("high"),
            ),
            title: details
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Suggested fact")
                .to_string(),
            tags: details
                .get("tags")
                .and_then(serde_json::Value::as_array)
                .map(|xs| {
                    xs.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            received_at: now_string(),
        });
        state.log.append(&Event {
            id: None,
            incident_id: incident_id.clone(),
            event_type: EventType::FactAsserted,
            description: "approved llm suggested fact".into(),
            details: Some(serde_json::to_value(fact).map_err(|e| e.to_string())?),
            timestamp: now_string(),
        })?;
    }

    state.log.append(&Event {
        id: None,
        incident_id,
        event_type: EventType::FactSuggestionResolved,
        description: "fact suggestion resolved".into(),
        details: Some(serde_json::json!({
            "suggestion_event_id": suggestion_event_id,
            "decision": decision
        })),
        timestamp: now_string(),
    })?;

    Ok(())
}

pub fn respond_to_escalation(
    state: &AppState,
    incident_id: String,
    response: EscalationResponse,
) -> Result<(), String> {
    state
        .decision_tx
        .send((incident_id, response))
        .map_err(|e| e.to_string())
}

pub fn upsert_alert_fact(
    state: &AppState,
    incident_id: String,
    fact_id: String,
    title: String,
    severity: String,
    tags: Vec<String>,
) -> Result<(), String> {
    let fact = Fact::Alert(AlertFact {
        id: fact_id.clone(),
        source: AlertSource::Generic,
        severity: parse_severity(&severity),
        title,
        tags,
        received_at: now_string(),
    });

    state.log.append(&Event {
        id: None,
        incident_id,
        event_type: EventType::FactAsserted,
        description: format!("fact upserted: {fact_id}"),
        details: Some(serde_json::to_value(fact).map_err(|e| e.to_string())?),
        timestamp: now_string(),
    })?;
    Ok(())
}

pub fn retract_fact(state: &AppState, incident_id: String, fact_id: String) -> Result<(), String> {
    state.log.append(&Event {
        id: None,
        incident_id,
        event_type: EventType::FactRetracted,
        description: format!("fact retracted: {fact_id}"),
        details: Some(serde_json::json!({ "fact_id": fact_id })),
        timestamp: now_string(),
    })?;
    Ok(())
}

pub fn reprocess_incident(state: &AppState, incident_id: String) -> Result<(), String> {
    let events = state.log.events_for_incident(&incident_id)?;
    let fact_map = materialize_fact_map(events);
    let mut facts = fact_map.into_values();

    let Some((fact, _timestamp)) = facts.next() else {
        return Err("no active facts for incident".into());
    };

    let runbooks = vec![
        ("crashloop_runbook", runbooks::crashloop_runbook()),
        ("oomkill_runbook", runbooks::oomkill_runbook()),
    ];

    let pattern = rules::detect_pattern(&fact);
    let Some((runbook_name, selected)) = planner::select_runbook(pattern, &runbooks) else {
        return Err("no matching deterministic runbook".into());
    };

    state.log.append(&Event {
        id: None,
        incident_id: incident_id.clone(),
        event_type: EventType::PlanSelected,
        description: format!("reprocess selected runbook: {runbook_name}"),
        details: serde_json::to_value(&selected).ok(),
        timestamp: now_string(),
    })?;

    match executor::execute_plan(&state.log, &incident_id, &selected, &|action| {
        Ok(serde_json::json!({
            "status": "ok",
            "tool": action.name
        }))
    }) {
        Ok(()) => {
            state.log.append(&Event {
                id: None,
                incident_id,
                event_type: EventType::Resolved,
                description: "incident resolved by reprocess".into(),
                details: None,
                timestamp: now_string(),
            })?;
            Ok(())
        }
        Err((failed_step, reason)) => {
            state.log.append(&Event {
                id: None,
                incident_id,
                event_type: EventType::Escalated,
                description: "reprocess escalation required".into(),
                details: Some(serde_json::json!({
                    "name": failed_step.name,
                    "effect": format!("{:?}", failed_step.effect),
                    "status": "failed",
                    "reason": reason
                })),
                timestamp: now_string(),
            })?;
            Ok(())
        }
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn list_incidents_cmd(state: tauri::State<'_, AppState>) -> Result<Vec<IncidentDto>, String> {
    list_incidents(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn get_beliefs_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<Vec<FactDto>, String> {
    get_beliefs(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn get_timeline_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<Vec<TimelineEventDto>, String> {
    get_timeline(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn get_current_plan_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<PlanDto, String> {
    get_current_plan(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn get_tool_calls_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<Vec<ToolCallDto>, String> {
    get_tool_calls(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn get_suggested_facts_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<Vec<SuggestedFactDto>, String> {
    get_suggested_facts(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn respond_to_escalation_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
    response: EscalationResponse,
) -> Result<(), String> {
    respond_to_escalation(&state, incident_id, response)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn upsert_alert_fact_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
    fact_id: String,
    title: String,
    severity: String,
    tags: Vec<String>,
) -> Result<(), String> {
    upsert_alert_fact(&state, incident_id, fact_id, title, severity, tags)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn retract_fact_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
    fact_id: String,
) -> Result<(), String> {
    retract_fact(&state, incident_id, fact_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn reprocess_incident_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<(), String> {
    reprocess_incident(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn generate_fact_suggestions_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
) -> Result<(), String> {
    generate_fact_suggestions(&state, incident_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command(rename_all = "camelCase")]
pub fn decide_fact_suggestion_cmd(
    state: tauri::State<'_, AppState>,
    incident_id: String,
    suggestion_event_id: i64,
    decision: String,
) -> Result<(), String> {
    decide_fact_suggestion(&state, incident_id, suggestion_event_id, decision)
}

fn summarize_incident(state: &AppState, incident_id: String) -> Result<IncidentDto, String> {
    let events = state.log.events_for_incident(&incident_id)?;

    let mut status = "active".to_string();
    let mut severity = "high".to_string();
    let mut title = String::new();
    let mut started_at = String::new();
    let mut current_phase = "observing".to_string();

    for event in &events {
        if started_at.is_empty() {
            started_at = event.timestamp.clone();
        }

        match event.event_type {
            EventType::Resolved => {
                status = "resolved".into();
                current_phase = "resolved".into();
            }
            EventType::Escalated => {
                status = "escalated".into();
                current_phase = "escalating".into();
            }
            EventType::EscalationResponded => {
                current_phase = "human-response".into();
            }
            EventType::FactRetracted => {
                current_phase = "matching".into();
            }
            EventType::FactSuggested | EventType::FactSuggestionResolved => {
                current_phase = "matching".into();
            }
            EventType::PlanSelected => {
                current_phase = "planning".into();
            }
            EventType::ActionIntent | EventType::ActionResult => {
                current_phase = "executing".into();
            }
            EventType::FactAsserted => {
                current_phase = "matching".into();
                if let Some(details) = &event.details {
                    if let Ok(fact) = serde_json::from_value::<Fact>(details.clone()) {
                        match fact {
                            Fact::Alert(alert) => {
                                title = alert.title;
                                severity = severity_to_string(alert.severity);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(IncidentDto {
        id: incident_id,
        status,
        severity,
        title,
        started_at,
        current_phase,
    })
}

fn severity_to_string(severity: Severity) -> String {
    match severity {
        Severity::Low => "low".into(),
        Severity::Medium => "medium".into(),
        Severity::High => "high".into(),
        Severity::Critical => "critical".into(),
    }
}

fn fact_to_dto(fact: Fact, timestamp: String) -> FactDto {
    match fact {
        Fact::Alert(alert) => FactDto {
            fact_id: alert.id,
            fact_type: "Alert".into(),
            summary: alert.title,
            severity: severity_to_string(alert.severity),
            tags: alert.tags,
            timestamp,
        },
    }
}

fn parse_severity(value: &str) -> Severity {
    match value.to_lowercase().as_str() {
        "low" => Severity::Low,
        "medium" => Severity::Medium,
        "critical" => Severity::Critical,
        _ => Severity::High,
    }
}

fn materialize_facts(events: Vec<Event>) -> Vec<FactDto> {
    materialize_fact_map(events)
        .into_values()
        .map(|(fact, timestamp)| fact_to_dto(fact, timestamp))
        .collect()
}

fn materialize_fact_map(events: Vec<Event>) -> BTreeMap<String, (Fact, String)> {
    let mut current: BTreeMap<String, (Fact, String)> = BTreeMap::new();

    for event in events {
        match event.event_type {
            EventType::FactAsserted => {
                let ts = event.timestamp;
                if let Some(value) = event.details {
                    if let Ok(fact) = serde_json::from_value::<Fact>(value) {
                        let fact_id = match &fact {
                            Fact::Alert(alert) => alert.id.clone(),
                        };
                        current.insert(fact_id, (fact, ts));
                    }
                }
            }
            EventType::FactRetracted => {
                if let Some(details) = event.details {
                    if let Some(fact_id) = details.get("fact_id").and_then(serde_json::Value::as_str)
                    {
                        current.remove(fact_id);
                    }
                }
            }
            _ => {}
        }
    }

    current
}

pub fn append_escalation_response_event(
    state: &AppState,
    incident_id: &str,
    response: &EscalationResponse,
) -> Result<(), String> {
    state.log.append(&Event {
        id: None,
        incident_id: incident_id.to_string(),
        event_type: EventType::EscalationResponded,
        description: "escalation response recorded".into(),
        details: Some(serde_json::to_value(response).map_err(|e| e.to_string())?),
        timestamp: now_string(),
    })?;
    Ok(())
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "0".into();
    };
    duration.as_secs().to_string()
}

fn build_llm_config_from_env() -> Option<llm::LlmConfig> {
    let api_key_env = std::env::var("LLM_API_KEY_ENV").unwrap_or_else(|_| "OPENAI_API_KEY".into());
    if std::env::var(&api_key_env).is_err() {
        return None;
    }
    Some(llm::LlmConfig {
        provider: std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".into()),
        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
        api_key_env,
        temperature: std::env::var("LLM_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use agent_core::event_log::{Event, EventLog, EventType};
    use agent_core::facts::{AlertFact, AlertSource, Fact, Severity};
    use std::sync::Arc;

    fn db_path(name: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        format!("/tmp/rig-bdi-tests/{name}-{nanos}.db")
    }

    #[test]
    fn list_incidents_reads_active_ids() {
        let log = EventLog::open(&db_path("list-incidents")).expect("open");
        log.append(&Event {
            id: None,
            incident_id: "inc-1".into(),
            event_type: EventType::FactAsserted,
            description: "fact".into(),
            details: None,
            timestamp: "1".into(),
        })
        .expect("append");

        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        let incidents = list_incidents(&state).expect("list");
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].id, "inc-1");
    }

    #[test]
    fn get_beliefs_reads_asserted_facts() {
        let log = EventLog::open(&db_path("get-beliefs")).expect("open");
        let fact = Fact::Alert(AlertFact {
            id: "inc-2".into(),
            source: AlertSource::Generic,
            severity: Severity::High,
            title: "db latency high".into(),
            tags: vec!["db".into()],
            received_at: "10".into(),
        });

        log.append(&Event {
            id: None,
            incident_id: "inc-2".into(),
            event_type: EventType::FactAsserted,
            description: "fact".into(),
            details: Some(serde_json::to_value(fact).expect("fact json")),
            timestamp: "10".into(),
        })
        .expect("append");

        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        let beliefs = get_beliefs(&state, "inc-2".into()).expect("beliefs");
        assert_eq!(beliefs.len(), 1);
        assert_eq!(beliefs[0].fact_id, "inc-2");
        assert_eq!(beliefs[0].fact_type, "Alert");
        assert_eq!(beliefs[0].summary, "db latency high");
        assert_eq!(beliefs[0].severity, "high");
        assert_eq!(beliefs[0].tags, vec!["db"]);
    }

    #[test]
    fn get_current_plan_maps_action_events_to_steps() {
        let log = EventLog::open(&db_path("get-current-plan")).expect("open");

        log.append(&Event {
            id: None,
            incident_id: "inc-3".into(),
            event_type: EventType::ActionIntent,
            description: "intent".into(),
            details: Some(serde_json::json!({
                "name": "inspect-pod-logs",
                "effect": "Observe",
                "status": "running"
            })),
            timestamp: "20".into(),
        })
        .expect("append intent");

        log.append(&Event {
            id: None,
            incident_id: "inc-3".into(),
            event_type: EventType::ActionResult,
            description: "result".into(),
            details: Some(serde_json::json!({
                "name": "inspect-pod-logs",
                "effect": "Observe",
                "status": "done"
            })),
            timestamp: "21".into(),
        })
        .expect("append result");

        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        let plan = get_current_plan(&state, "inc-3".into()).expect("plan");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, "inspect-pod-logs");
        assert_eq!(plan.steps[0].status, "done");
    }

    #[test]
    fn get_tool_calls_reads_intent_and_result() {
        let log = EventLog::open(&db_path("get-tool-calls")).expect("open");

        log.append(&Event {
            id: None,
            incident_id: "inc-tools".into(),
            event_type: EventType::ActionIntent,
            description: "intent".into(),
            details: Some(serde_json::json!({
                "name": "inspect-pod-logs",
                "effect": "Observe",
                "status": "running"
            })),
            timestamp: "30".into(),
        })
        .expect("append intent");

        log.append(&Event {
            id: None,
            incident_id: "inc-tools".into(),
            event_type: EventType::ActionResult,
            description: "result".into(),
            details: Some(serde_json::json!({
                "name": "inspect-pod-logs",
                "effect": "Observe",
                "status": "done"
            })),
            timestamp: "31".into(),
        })
        .expect("append result");

        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        let rows = get_tool_calls(&state, "inc-tools".into()).expect("tool calls");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].phase, "intent");
        assert_eq!(rows[1].phase, "result");
        assert_eq!(rows[1].status, "done");
    }

    #[test]
    fn retract_fact_removes_it_from_materialized_beliefs() {
        let log = EventLog::open(&db_path("retract-fact")).expect("open");
        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        upsert_alert_fact(
            &state,
            "inc-4".into(),
            "fact-1".into(),
            "test title".into(),
            "high".into(),
            vec!["x".into()],
        )
        .expect("upsert");

        retract_fact(&state, "inc-4".into(), "fact-1".into()).expect("retract");

        let beliefs = get_beliefs(&state, "inc-4".into()).expect("beliefs");
        assert!(beliefs.is_empty());
    }

    #[test]
    fn reprocess_incident_appends_plan_and_resolution() {
        let log = EventLog::open(&db_path("reprocess-incident")).expect("open");
        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        upsert_alert_fact(
            &state,
            "inc-r1".into(),
            "inc-r1".into(),
            "Pod crashlooping".into(),
            "high".into(),
            vec!["crashloop".into()],
        )
        .expect("upsert");

        reprocess_incident(&state, "inc-r1".into()).expect("reprocess");
        let timeline = get_timeline(&state, "inc-r1".into()).expect("timeline");
        assert!(timeline.iter().any(|e| e.event_type == "PlanSelected"));
        assert!(timeline.iter().any(|e| e.event_type == "Resolved"));
    }

    #[test]
    fn suggestion_lifecycle_tracks_pending_queue() {
        let log = EventLog::open(&db_path("suggestion-lifecycle")).expect("open");
        let (tx, _rx) = std::sync::mpsc::channel();
        let state = AppState {
            log: Arc::new(log),
            decision_tx: tx,
        };

        state
            .log
            .append(&Event {
                id: None,
                incident_id: "inc-s1".into(),
                event_type: EventType::FactSuggested,
                description: "suggested".into(),
                details: Some(serde_json::json!({
                    "fact_id": "f1",
                    "title": "suspected deploy regression",
                    "severity": "high",
                    "tags": ["deploy"],
                    "rationale": "error spike after rollout"
                })),
                timestamp: "1".into(),
            })
            .expect("append suggested");

        let pending = get_suggested_facts(&state, "inc-s1".into()).expect("pending");
        assert_eq!(pending.len(), 1);

        decide_fact_suggestion(
            &state,
            "inc-s1".into(),
            pending[0].suggestion_event_id,
            "reject".into(),
        )
        .expect("decide");

        let pending_after = get_suggested_facts(&state, "inc-s1".into()).expect("pending after");
        assert!(pending_after.is_empty());
    }
}
