use agent_core::event_log::EventLog;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EscalationResponse {
    Approve,
    Reject { reason: String },
    TakeOver,
}

#[derive(Clone)]
pub struct AppState {
    pub log: Arc<EventLog>,
    pub decision_tx: std::sync::mpsc::Sender<(String, EscalationResponse)>,
}

pub struct RuntimeChannels {
    pub decision_rx: std::sync::mpsc::Receiver<(String, EscalationResponse)>,
}
