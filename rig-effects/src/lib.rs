use serde::{Deserialize, Serialize};
use std::future::Future;

/// Effect classification for operations.
/// Ordered by increasing severity of side effects.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    /// No side effects. Safe to retry, reorder, cache.
    Pure,
    /// Reads external state. Idempotent but results may differ.
    Observe,
    /// Changes external state. Can be undone via compensation.
    Mutate,
    /// Cannot be undone. Commitment point.
    Irreversible,
}

impl Effect {
    /// Derive recovery strategy from effect type.
    pub fn recovery(&self) -> Recovery {
        match self {
            Effect::Pure | Effect::Observe => Recovery::Retry,
            Effect::Mutate => Recovery::CheckAndRetry,
            Effect::Irreversible => Recovery::ManualReview,
        }
    }

    /// Can the planner safely backtrack past this effect?
    pub fn backtrackable(&self) -> bool {
        matches!(self, Effect::Pure | Effect::Observe | Effect::Mutate)
    }

    /// Cost multiplier for planning — prefer plans with fewer severe effects.
    pub fn cost_weight(&self) -> u32 {
        match self {
            Effect::Pure => 1,
            Effect::Observe => 2,
            Effect::Mutate => 10,
            Effect::Irreversible => 100,
        }
    }
}

/// Recovery strategy after failure, derived from Effect type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recovery {
    /// Safe to re-execute.
    Retry,
    /// Must verify external state before retrying.
    CheckAndRetry,
    /// Requires human review.
    ManualReview,
}

/// Any operation that has a classified effect.
pub trait Effectful {
    fn effect(&self) -> Effect;
}

/// An operation that can be undone.
/// Only meaningful for `Effect::Mutate` — Pure/Observe don't need it,
/// Irreversible can't do it.
pub trait Compensable: Effectful {
    /// State captured before execution, needed to undo later.
    type Snapshot: Clone + Send + Sync;
    /// Error type.
    type Error: std::error::Error + Send + Sync;

    /// Capture state before execution.
    fn snapshot(&self) -> impl Future<Output = Result<Self::Snapshot, Self::Error>> + Send;

    /// Undo the effect using captured state.
    fn compensate(
        &self,
        snapshot: Self::Snapshot,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Metadata for an action schema used by the planner.
/// Wraps any action with its effect classification and STRIPS-like semantics.
#[derive(Clone, Debug)]
pub struct ActionMeta<A> {
    pub action: A,
    pub effect: Effect,
    pub name: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_is_retryable() {
        assert_eq!(Effect::Pure.recovery(), Recovery::Retry);
        assert!(Effect::Pure.backtrackable());
    }

    #[test]
    fn irreversible_requires_review() {
        assert_eq!(Effect::Irreversible.recovery(), Recovery::ManualReview);
        assert!(!Effect::Irreversible.backtrackable());
    }

    #[test]
    fn effect_cost_ordering() {
        assert!(Effect::Pure.cost_weight() < Effect::Observe.cost_weight());
        assert!(Effect::Observe.cost_weight() < Effect::Mutate.cost_weight());
        assert!(Effect::Mutate.cost_weight() < Effect::Irreversible.cost_weight());
    }

    #[test]
    fn serde_roundtrip() {
        let effect = Effect::Mutate;
        let json = serde_json::to_string(&effect).unwrap();
        let back: Effect = serde_json::from_str(&json).unwrap();
        assert_eq!(effect, back);
    }
}
