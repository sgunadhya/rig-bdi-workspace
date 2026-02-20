use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EscalationResponse {
    Approve,
    Reject { reason: String },
    TakeOver,
}
