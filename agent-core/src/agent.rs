use crate::event_log::{Event, EventLog, EventType};
use crate::facts::Fact;
use crate::llm::{self, LlmConfig};
use crate::planner;
use crate::rules;
use crate::runbooks::{ActionSchema, Runbook};
use crate::{executor, runbooks};
use std::sync::mpsc::Receiver;

#[derive(Clone)]
pub struct AgentConfig {
    pub max_replan_attempts: usize,
    pub runbooks: Vec<(&'static str, Runbook)>,
    pub all_actions: Vec<ActionSchema>,
    pub goal_props: Vec<String>,
    pub llm: Option<LlmConfig>,
}

#[derive(Clone, Debug)]
pub struct EscalationRequest {
    pub incident_id: String,
    pub reason: String,
}

pub fn run_agent<F>(
    webhook_stream: Receiver<Fact>,
    config: AgentConfig,
    log: EventLog,
    escalation_tx: std::sync::mpsc::Sender<EscalationRequest>,
    tool_executor: F,
) where
    F: Fn(ActionSchema) -> Result<serde_json::Value, String> + Send + Sync + 'static,
{
    let fallback = known_actions(&config);
    let mut recent_facts: Vec<Fact> = Vec::new();

    while let Ok(fact) = webhook_stream.recv() {
        let incident_id = incident_id_from_fact(&fact);
        recent_facts.push(fact.clone());
        if recent_facts.len() > 16 {
            let _ = recent_facts.remove(0);
        }

        let _ = log.append(&Event {
            id: None,
            incident_id: incident_id.clone(),
            event_type: EventType::FactAsserted,
            description: "fact asserted".into(),
            details: serde_json::to_value(&fact).ok(),
            timestamp: now_string(),
        });

        let pattern = rules::detect_pattern(&fact);
        let selection = planner::select_runbook(pattern, &config.runbooks);

        let selected = if let Some((runbook_name, runbook)) = selection {
            let _ = log.append(&Event {
                id: None,
                incident_id: incident_id.clone(),
                event_type: EventType::PlanSelected,
                description: format!("selected runbook: {runbook_name}"),
                details: serde_json::to_value(&runbook).ok(),
                timestamp: now_string(),
            });
            runbook
        } else if let Some(llm_cfg) = config.llm.as_ref() {
            match llm::interpret(llm_cfg, &recent_facts)
                .and_then(|interp| {
                    llm::propose_and_validate(
                        llm_cfg,
                        &interp.hypothesis,
                        &interp.goal,
                        &interp.candidate_actions,
                        &fallback,
                    )
                    .map(|actions| (interp, actions))
                }) {
                Ok((interp, actions)) if !actions.is_empty() => {
                    let _ = log.append(&Event {
                        id: None,
                        incident_id: incident_id.clone(),
                        event_type: EventType::PlanSelected,
                        description: format!(
                            "LLM-proposed plan: {} steps ({})",
                            actions.len(),
                            interp.hypothesis
                        ),
                        details: serde_json::to_value(&actions).ok(),
                        timestamp: now_string(),
                    });
                    actions
                }
                Ok((_interp, _)) => {
                    escalate_no_plan(&log, &escalation_tx, incident_id.clone(), "no valid llm plan");
                    continue;
                }
                Err(_) => {
                    escalate_no_plan(&log, &escalation_tx, incident_id.clone(), "no valid llm plan");
                    continue;
                }
            }
        } else {
            escalate_no_plan(&log, &escalation_tx, incident_id.clone(), "no matching runbook");
            continue;
        };

        match executor::execute_plan(&log, &incident_id, &selected, &tool_executor) {
            Ok(()) => {
                let _ = log.append(&Event {
                    id: None,
                    incident_id,
                    event_type: EventType::Resolved,
                    description: "incident resolved".into(),
                    details: None,
                    timestamp: now_string(),
                });
            }
            Err((failed_step, reason)) => {
                let req = EscalationRequest {
                    incident_id: incident_id.clone(),
                    reason: reason.clone(),
                };
                let _ = escalation_tx.send(req);
                let _ = log.append(&Event {
                    id: None,
                    incident_id,
                    event_type: EventType::Escalated,
                    description: "escalation required".into(),
                    details: Some(serde_json::json!({
                        "name": failed_step.name,
                        "effect": format!("{:?}", failed_step.effect),
                        "status": "failed",
                        "reason": reason
                    })),
                    timestamp: now_string(),
                });
            }
        }
    }
}

fn known_actions(config: &AgentConfig) -> Vec<ActionSchema> {
    if !config.all_actions.is_empty() {
        return config.all_actions.clone();
    }

    let mut out = Vec::new();
    for (_name, runbook) in &config.runbooks {
        for action in runbook {
            if !out.iter().any(|a: &ActionSchema| a.name == action.name) {
                out.push(action.clone());
            }
        }
    }
    if out.is_empty() {
        out.extend(runbooks::crashloop_runbook());
    }
    out
}

fn escalate_no_plan(
    log: &EventLog,
    escalation_tx: &std::sync::mpsc::Sender<EscalationRequest>,
    incident_id: String,
    reason: &str,
) {
    let req = EscalationRequest {
        incident_id: incident_id.clone(),
        reason: reason.to_string(),
    };
    let _ = escalation_tx.send(req);
    let _ = log.append(&Event {
        id: None,
        incident_id,
        event_type: EventType::Escalated,
        description: "escalation required".into(),
        details: Some(serde_json::json!({
            "status": "failed",
            "reason": reason
        })),
        timestamp: now_string(),
    });
}

fn incident_id_from_fact(fact: &Fact) -> String {
    match fact {
        Fact::Alert(alert) => alert.id.clone(),
    }
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "0".into();
    };
    duration.as_secs().to_string()
}
