use agent_core::facts::{AlertFact, AlertSource, Fact, Severity};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use fact_registry::{CanonicalAlertV1, validate_alert_v1};

pub trait FactAdapter: Send + Sync + 'static {
    fn parse(&self, payload: &serde_json::Value) -> Result<CanonicalAlertV1, String>;
}

pub struct GenericAdapter;
pub struct AlertmanagerAdapter;

impl FactAdapter for GenericAdapter {
    fn parse(&self, payload: &serde_json::Value) -> Result<CanonicalAlertV1, String> {
        let alert = CanonicalAlertV1 {
            schema: "alert.v1".into(),
            id: payload
                .get("id")
                .or_else(|| payload.get("incident_id"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            title: payload
                .get("title")
                .or_else(|| payload.get("alert_title"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            severity: payload
                .get("severity")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("high")
                .to_string(),
            tags: payload
                .get("tags")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            source: "generic".into(),
            occurred_at: current_timestamp(),
        };
        validate_alert_v1(&alert)?;
        Ok(alert)
    }
}

impl FactAdapter for AlertmanagerAdapter {
    fn parse(&self, payload: &serde_json::Value) -> Result<CanonicalAlertV1, String> {
        let first = payload
            .get("alerts")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.first())
            .ok_or_else(|| "alertmanager payload missing alerts[0]".to_string())?;

        let labels = first.get("labels").cloned().unwrap_or_else(|| serde_json::json!({}));
        let annotations = first
            .get("annotations")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let title = annotations
            .get("summary")
            .or_else(|| annotations.get("description"))
            .or_else(|| labels.get("alertname"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("alertmanager alert")
            .to_string();

        let severity = labels
            .get("severity")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("high")
            .to_string();

        let id = first
            .get("fingerprint")
            .or_else(|| labels.get("alertname"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let mut tags = Vec::new();
        if let Some(obj) = labels.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    tags.push(format!("{k}:{s}"));
                }
            }
        }

        let alert = CanonicalAlertV1 {
            schema: "alert.v1".into(),
            id,
            title,
            severity,
            tags,
            source: "alertmanager".into(),
            occurred_at: current_timestamp(),
        };
        validate_alert_v1(&alert)?;
        Ok(alert)
    }
}

pub fn webhook_router(tx: std::sync::mpsc::Sender<Fact>) -> Router {
    Router::new()
        .route("/webhook/generic", post(handle_generic))
        .route("/webhook/datadog", post(handle_datadog))
        .route("/webhook/pagerduty", post(handle_pagerduty))
        .route("/webhook/alertmanager", post(handle_alertmanager))
        .with_state(tx)
}

pub fn parse_generic(payload: &serde_json::Value) -> Result<Fact, String> {
    parse_with_adapter(payload, GenericAdapter, AlertSource::Generic)
}

fn parse_datadog(payload: &serde_json::Value) -> Result<Fact, String> {
    parse_with_adapter(payload, GenericAdapter, AlertSource::Datadog)
}

fn parse_pagerduty(payload: &serde_json::Value) -> Result<Fact, String> {
    parse_with_adapter(payload, GenericAdapter, AlertSource::PagerDuty)
}

fn parse_alertmanager(payload: &serde_json::Value) -> Result<Fact, String> {
    parse_with_adapter(payload, AlertmanagerAdapter, AlertSource::Generic)
}

async fn handle_generic(
    State(tx): State<std::sync::mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    send_fact(&tx, parse_generic(&payload))
}

async fn handle_datadog(
    State(tx): State<std::sync::mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    send_fact(&tx, parse_datadog(&payload))
}

async fn handle_pagerduty(
    State(tx): State<std::sync::mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    send_fact(&tx, parse_pagerduty(&payload))
}

async fn handle_alertmanager(
    State(tx): State<std::sync::mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    send_fact(&tx, parse_alertmanager(&payload))
}

fn parse_with_adapter(
    payload: &serde_json::Value,
    adapter: impl FactAdapter,
    source: AlertSource,
) -> Result<Fact, String> {
    let canonical = adapter.parse(payload)?;
    Ok(Fact::Alert(AlertFact {
        id: canonical.id,
        source,
        severity: map_severity(&canonical.severity),
        title: canonical.title,
        tags: canonical.tags,
        received_at: canonical.occurred_at,
    }))
}

fn send_fact(tx: &std::sync::mpsc::Sender<Fact>, fact: Result<Fact, String>) -> StatusCode {
    let Ok(fact) = fact else {
        return StatusCode::BAD_REQUEST;
    };
    match tx.send(fact) {
        Ok(_) => StatusCode::ACCEPTED,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn map_severity(value: &str) -> Severity {
    match value.to_lowercase().as_str() {
        "low" => Severity::Low,
        "medium" => Severity::Medium,
        "critical" => Severity::Critical,
        _ => Severity::High,
    }
}

fn current_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "0".into();
    };
    duration.as_secs().to_string()
}
