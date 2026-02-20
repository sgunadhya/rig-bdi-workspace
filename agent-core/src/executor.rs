use crate::event_log::{Event, EventLog, EventType};
use crate::runbooks::ActionSchema;

pub fn execute_plan<F>(
    log: &EventLog,
    incident_id: &str,
    steps: &[ActionSchema],
    tool_executor: &F,
) -> Result<(), (ActionSchema, String)>
where
    F: Fn(ActionSchema) -> Result<serde_json::Value, String> + Send + Sync + 'static,
{
    for step in steps {
        let _ = log.append(&Event {
            id: None,
            incident_id: incident_id.to_string(),
            event_type: EventType::ActionIntent,
            description: format!("intent: {}", step.name),
            details: Some(serde_json::json!({
                "name": step.name,
                "effect": format!("{:?}", step.effect),
                "status": "running",
                "mcp": {
                    "tool_name": step.name,
                    "phase": "intent",
                    "request": {
                        "name": step.name,
                        "effect": format!("{:?}", step.effect),
                    }
                }
            })),
            timestamp: now_string(),
        });

        match tool_executor(step.clone()) {
            Ok(output) => {
                let _ = log.append(&Event {
                    id: None,
                    incident_id: incident_id.to_string(),
                    event_type: EventType::ActionResult,
                    description: format!("action succeeded: {}", step.name),
                    details: Some(serde_json::json!({
                        "name": step.name,
                        "effect": format!("{:?}", step.effect),
                        "status": "done",
                        "result": output,
                        "mcp": {
                            "tool_name": step.name,
                            "phase": "result",
                            "ok": true,
                            "output": output
                        }
                    })),
                    timestamp: now_string(),
                });
            }
            Err(err) => {
                let err_msg = err.clone();
                let _ = log.append(&Event {
                    id: None,
                    incident_id: incident_id.to_string(),
                    event_type: EventType::ActionResult,
                    description: format!("action failed: {}", step.name),
                    details: Some(serde_json::json!({
                        "name": step.name,
                        "effect": format!("{:?}", step.effect),
                        "status": "failed",
                        "error": err_msg.clone(),
                        "mcp": {
                            "tool_name": step.name,
                            "phase": "result",
                            "ok": false,
                            "error": err_msg
                        }
                    })),
                    timestamp: now_string(),
                });
                return Err((step.clone(), err));
            }
        }
    }

    Ok(())
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "0".into();
    };
    duration.as_secs().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_log::EventType;
    use crate::runbooks::ActionSchema;
    use rig_effects::Effect;

    fn db_path(name: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        format!("/tmp/rig-bdi-tests/{name}-{nanos}.db")
    }

    #[test]
    fn failure_emits_action_result_with_failed_status() {
        let log = EventLog::open(&db_path("executor-failure")).expect("open");
        let steps = vec![ActionSchema {
            name: "failing-action".into(),
            effect: Effect::Mutate,
        }];

        let result = execute_plan(&log, "inc-fail", &steps, &|_step| {
            Err("boom".to_string())
        });
        assert!(result.is_err());

        let events = log.events_for_incident("inc-fail").expect("events");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].event_type, EventType::ActionIntent));
        assert!(matches!(events[1].event_type, EventType::ActionResult));

        let details = events[1].details.as_ref().expect("details");
        assert_eq!(
            details.get("status").and_then(serde_json::Value::as_str),
            Some("failed")
        );
        assert_eq!(
            details.get("error").and_then(serde_json::Value::as_str),
            Some("boom")
        );
    }
}
