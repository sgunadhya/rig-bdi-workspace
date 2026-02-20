//! Typed invoke/listen wrappers.
//! These are placeholders until the Leptos+Tauri runtime wiring is enabled.

use crate::dto::{EscalationResponse, IncidentDto, TimelineEventDto};

pub async fn fetch_incidents() -> Result<Vec<IncidentDto>, String> {
    Err("bridge not yet wired".into())
}

pub async fn fetch_timeline(_id: &str) -> Result<Vec<TimelineEventDto>, String> {
    Err("bridge not yet wired".into())
}

pub async fn submit_escalation(_id: &str, _response: EscalationResponse) -> Result<(), String> {
    Err("bridge not yet wired".into())
}
