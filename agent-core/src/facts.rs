use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertSource {
    Generic,
    Datadog,
    PagerDuty,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlertFact {
    pub id: String,
    pub source: AlertSource,
    pub severity: Severity,
    pub title: String,
    pub tags: Vec<String>,
    pub received_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fact {
    Alert(AlertFact),
}
