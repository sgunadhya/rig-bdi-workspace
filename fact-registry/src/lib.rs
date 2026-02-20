use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicalAlertV1 {
    pub schema: String,
    pub id: String,
    pub title: String,
    pub severity: String,
    pub tags: Vec<String>,
    pub source: String,
    pub occurred_at: String,
}

pub fn validate_alert_v1(alert: &CanonicalAlertV1) -> Result<(), String> {
    if alert.schema != "alert.v1" {
        return Err(format!("unsupported schema '{}'", alert.schema));
    }
    if alert.id.trim().is_empty() {
        return Err("id is required".into());
    }
    if alert.title.trim().is_empty() {
        return Err("title is required".into());
    }
    match alert.severity.to_lowercase().as_str() {
        "low" | "medium" | "high" | "critical" => {}
        other => return Err(format!("invalid severity '{other}'")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_alert_v1() {
        let alert = CanonicalAlertV1 {
            schema: "alert.v1".into(),
            id: "inc-1".into(),
            title: "cpu high".into(),
            severity: "high".into(),
            tags: vec!["cpu".into()],
            source: "generic".into(),
            occurred_at: "1".into(),
        };
        assert!(validate_alert_v1(&alert).is_ok());
    }
}
