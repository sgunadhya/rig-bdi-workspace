use rig_effects::Effect;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSchema {
    pub name: String,
    pub effect: Effect,
}

pub type Runbook = Vec<ActionSchema>;

pub fn crashloop_runbook() -> Runbook {
    vec![
        ActionSchema {
            name: "inspect-pod-logs".into(),
            effect: Effect::Observe,
        },
        ActionSchema {
            name: "rollback-deployment".into(),
            effect: Effect::Mutate,
        },
    ]
}

pub fn oomkill_runbook() -> Runbook {
    vec![
        ActionSchema {
            name: "inspect-memory-metrics".into(),
            effect: Effect::Observe,
        },
        ActionSchema {
            name: "tune-memory-limits".into(),
            effect: Effect::Mutate,
        },
    ]
}
