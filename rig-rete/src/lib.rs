//! Trait definitions for a RETE-based production rule system.
//!
//! Defines the interfaces for facts, conditions, rules, working memory,
//! conflict resolution, and the RETE network engine. The actual RETE algorithm
//! is an implementation detail that lives downstream.
//!
//! Depends on [`rig_effects`] — rules fire actions that have effects.

#![allow(async_fn_in_trait)]

use chrono::{DateTime, Utc};
use rig_effects::Effectful;
use std::fmt::Debug;
use std::hash::Hash;

#[cfg(feature = "derive")]
pub use rig_rete_derive::Fact;

// ── Fact ─────────────────────────────────────────────────────────────────────

/// A fact in working memory. Represents a typed assertion about the world.
///
/// Facts are the atomic unit of belief in the BDI architecture. They are
/// immutable once asserted; updates are expressed as retract + re-assert.
///
/// # Required bounds
/// Implementors must also satisfy `Clone + Eq + Hash + Debug`.
pub trait Fact: Clone + Eq + Hash + Debug + Send + Sync + 'static {
    /// Unique identifier for this fact instance.
    /// Used as the key in working memory.
    type Id: Clone + Eq + Hash + Debug + Send + Sync;

    /// Return the unique identifier for this fact.
    fn id(&self) -> &Self::Id;

    /// Timestamp when this fact was asserted / last observed.
    fn timestamp(&self) -> DateTime<Utc>;
}

// ── Condition ─────────────────────────────────────────────────────────────────

/// A condition that can match against facts in working memory.
///
/// Conditions are the LHS atoms of production rules. Each condition tests a
/// single fact and, on success, produces variable bindings that flow through
/// to the rule's action generator.
pub trait Condition<F: Fact>: Send + Sync {
    /// Variable bindings produced by a successful match.
    type Bindings: Clone + Debug + Send + Sync;

    /// Test whether a fact satisfies this condition.
    ///
    /// Returns `Some(bindings)` on success, `None` on failure.
    fn matches(&self, fact: &F) -> Option<Self::Bindings>;

    /// Human-readable description for debugging and logging.
    fn description(&self) -> &str;
}

// ── Rule ─────────────────────────────────────────────────────────────────────

/// A production rule: a set of conditions (LHS) and actions to fire (RHS).
///
/// A rule is activated when **all** of its conditions are simultaneously
/// satisfied by facts in working memory. The activated conflict set is then
/// resolved by a [`ConflictStrategy`] before any rule fires.
pub trait Rule: Send + Sync {
    /// The fact type this rule operates on.
    type Fact: Fact;
    /// The action type produced when the rule fires.
    type Action: Effectful;
    /// The bindings type produced by condition matching.
    type Bindings: Clone + Debug + Send + Sync;

    /// Unique identifier for this rule (must be stable across restarts).
    fn id(&self) -> &str;

    /// The conditions that must all match for this rule to activate.
    ///
    /// An empty slice means the rule is unconditionally active (fires on every
    /// fact assertion). All conditions share the same `Bindings` type so they
    /// cooperate to build a unified binding environment.
    fn conditions(&self) -> &[Box<dyn Condition<Self::Fact, Bindings = Self::Bindings>>];

    /// The actions to execute when this rule fires, parameterised by bindings.
    fn actions(&self, bindings: &Self::Bindings) -> Vec<Self::Action>;

    /// Priority for conflict resolution. Higher value fires first when multiple
    /// rules are simultaneously activated.
    fn priority(&self) -> i32;

    /// Human-readable description of this rule's intent.
    fn description(&self) -> &str;
}

// ── RuleMatch ────────────────────────────────────────────────────────────────

/// A match result: a rule that is ready to fire with specific bindings.
///
/// Constructed by the RETE network when all conditions of a rule are satisfied.
/// Placed in the conflict set awaiting selection by a [`ConflictStrategy`].
#[derive(Debug)]
pub struct RuleMatch<R: Rule> {
    /// The activated rule.
    pub rule: R,
    /// Variable bindings produced by condition matching.
    pub bindings: R::Bindings,
    /// IDs of the facts that caused this match.
    pub matched_facts: Vec<<R::Fact as Fact>::Id>,
    /// Wall-clock time when this match was created.
    pub timestamp: DateTime<Utc>,
}

// ── WorkingMemory ─────────────────────────────────────────────────────────────

/// Working memory: the set of currently active facts.
///
/// This is the agent's belief set. It maps fact IDs to facts and provides
/// the interface through which the RETE network is notified of changes.
pub trait WorkingMemory<F: Fact>: Send + Sync {
    /// Assert a new fact into working memory.
    ///
    /// If a fact with the same ID already exists it is replaced.
    /// Returns `true` if the fact is **new** (was not already present),
    /// `false` if it replaced an existing fact.
    fn assert_fact(&mut self, fact: F) -> bool;

    /// Retract (remove) a fact from working memory by ID.
    ///
    /// Returns the removed fact if it existed, `None` otherwise.
    fn retract_fact(&mut self, id: &F::Id) -> Option<F>;

    /// Check whether a fact with the given ID is currently in memory.
    fn contains(&self, id: &F::Id) -> bool;

    /// Look up a fact by ID.
    fn get(&self, id: &F::Id) -> Option<&F>;

    /// Iterate over all facts currently in working memory.
    fn facts(&self) -> Box<dyn Iterator<Item = &F> + '_>;

    /// Number of facts currently in memory.
    fn len(&self) -> usize;

    /// Returns `true` if working memory contains no facts.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ── ConflictStrategy ─────────────────────────────────────────────────────────

/// Conflict resolution strategy: when multiple rules match, pick which fires.
///
/// The strategy receives the full conflict set (all activated, unfired matches)
/// and selects at most one to fire. Returning `None` suspends firing this cycle
/// (quiescence / inhibition).
pub trait ConflictStrategy<R: Rule>: Send + Sync {
    /// Select the best match from the conflict set.
    ///
    /// Returns `None` if no rule should fire.
    fn select(&self, matches: &[RuleMatch<R>]) -> Option<&RuleMatch<R>>;
}

// ── ReteNetwork ───────────────────────────────────────────────────────────────

/// The RETE network interface.
///
/// Efficiently matches facts against rules using the RETE algorithm.
/// Maintains an internal set of activated rule matches updated incrementally
/// as facts are asserted and retracted.
///
/// The RETE algorithm itself is an implementation detail behind this trait —
/// naive linear scan, compiled networks, and parallel RETE are all valid.
pub trait ReteNetwork<F: Fact, R: Rule<Fact = F>>: Send + Sync {
    /// Add a rule to the network.
    ///
    /// The network takes ownership of the rule and begins matching it against
    /// subsequently asserted facts.
    fn add_rule(&mut self, rule: R);

    /// Remove a rule from the network by ID.
    ///
    /// Returns the removed rule if it existed, `None` otherwise. All matches
    /// derived from this rule are also removed from the activated set.
    fn remove_rule(&mut self, rule_id: &str) -> Option<R>;

    /// Notify the network that a fact was asserted into working memory.
    ///
    /// The network incrementally updates its internal state and returns any
    /// **newly** activated rule matches caused by this assertion.
    fn on_assert(&mut self, fact: &F) -> Vec<RuleMatch<R>>;

    /// Notify the network that a fact was retracted from working memory.
    ///
    /// Returns the rule matches that are **no longer valid** as a result of
    /// this retraction so callers can remove them from the conflict set.
    fn on_retract(&mut self, fact_id: &F::Id) -> Vec<RuleMatch<R>>;

    /// Get all currently activated (ready-to-fire) rule matches.
    ///
    /// This is the conflict set. Use a [`ConflictStrategy`] to select one.
    fn activated(&self) -> &[RuleMatch<R>];
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rig_effects::{Effect, Effectful};
    use std::collections::{HashMap, HashSet};

    // ── Mock Fact ────────────────────────────────────────────────────────────

    #[derive(Clone, Eq, PartialEq, Hash, Debug)]
    struct TestFact {
        id: u32,
        value: String,
        ts: DateTime<Utc>,
    }

    impl Fact for TestFact {
        type Id = u32;

        fn id(&self) -> &u32 {
            &self.id
        }

        fn timestamp(&self) -> DateTime<Utc> {
            self.ts
        }
    }

    fn make_fact(id: u32, value: &str) -> TestFact {
        TestFact {
            id,
            value: value.into(),
            ts: Utc::now(),
        }
    }

    // ── Mock Bindings & Conditions ───────────────────────────────────────────

    #[derive(Clone, Debug)]
    struct EmptyBindings;

    struct AlwaysMatchCond;

    impl Condition<TestFact> for AlwaysMatchCond {
        type Bindings = EmptyBindings;

        fn matches(&self, _fact: &TestFact) -> Option<EmptyBindings> {
            Some(EmptyBindings)
        }

        fn description(&self) -> &str {
            "always matches"
        }
    }

    struct NeverMatchCond;

    impl Condition<TestFact> for NeverMatchCond {
        type Bindings = EmptyBindings;

        fn matches(&self, _fact: &TestFact) -> Option<EmptyBindings> {
            None
        }

        fn description(&self) -> &str {
            "never matches"
        }
    }

    struct ValueMatchCond(String);

    impl Condition<TestFact> for ValueMatchCond {
        type Bindings = EmptyBindings;

        fn matches(&self, fact: &TestFact) -> Option<EmptyBindings> {
            if fact.value == self.0 {
                Some(EmptyBindings)
            } else {
                None
            }
        }

        fn description(&self) -> &str {
            "matches specific value"
        }
    }

    // ── Mock Action ──────────────────────────────────────────────────────────

    #[derive(Clone, Debug, PartialEq)]
    struct NoopAction;

    impl Effectful for NoopAction {
        fn effect(&self) -> Effect {
            Effect::Pure
        }
    }

    // ── Mock Rules ───────────────────────────────────────────────────────────

    // Full-featured test rule: stores conditions but is NOT Clone because
    // Box<dyn Condition<...>> is not Clone. Used for Rule-trait unit tests only.
    struct TestRule {
        id: String,
        priority: i32,
        conditions: Vec<Box<dyn Condition<TestFact, Bindings = EmptyBindings>>>,
    }

    impl Rule for TestRule {
        type Fact = TestFact;
        type Action = NoopAction;
        type Bindings = EmptyBindings;

        fn id(&self) -> &str {
            &self.id
        }

        fn conditions(&self) -> &[Box<dyn Condition<TestFact, Bindings = EmptyBindings>>] {
            &self.conditions
        }

        fn actions(&self, _bindings: &EmptyBindings) -> Vec<NoopAction> {
            vec![NoopAction]
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        fn description(&self) -> &str {
            "test rule"
        }
    }

    // Simplified Clone rule used for ConflictStrategy / ReteNetwork tests.
    // No stored conditions (vacuously matches everything).
    #[derive(Clone, Debug)]
    struct SimpleRule {
        id: String,
        priority: i32,
    }

    impl Rule for SimpleRule {
        type Fact = TestFact;
        type Action = NoopAction;
        type Bindings = EmptyBindings;

        fn id(&self) -> &str {
            &self.id
        }

        fn conditions(&self) -> &[Box<dyn Condition<TestFact, Bindings = EmptyBindings>>] {
            &[]
        }

        fn actions(&self, _bindings: &EmptyBindings) -> Vec<NoopAction> {
            vec![NoopAction]
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        fn description(&self) -> &str {
            "simple rule"
        }
    }

    // ── Mock WorkingMemory ───────────────────────────────────────────────────

    #[derive(Default)]
    struct MockWorkingMemory {
        facts: HashMap<u32, TestFact>,
    }

    impl WorkingMemory<TestFact> for MockWorkingMemory {
        fn assert_fact(&mut self, fact: TestFact) -> bool {
            let id = *fact.id();
            self.facts.insert(id, fact).is_none()
        }

        fn retract_fact(&mut self, id: &u32) -> Option<TestFact> {
            self.facts.remove(id)
        }

        fn contains(&self, id: &u32) -> bool {
            self.facts.contains_key(id)
        }

        fn get(&self, id: &u32) -> Option<&TestFact> {
            self.facts.get(id)
        }

        fn facts(&self) -> Box<dyn Iterator<Item = &TestFact> + '_> {
            Box::new(self.facts.values())
        }

        fn len(&self) -> usize {
            self.facts.len()
        }
    }

    // ── Mock ConflictStrategy ────────────────────────────────────────────────

    struct PriorityStrategy;

    impl ConflictStrategy<SimpleRule> for PriorityStrategy {
        fn select(&self, matches: &[RuleMatch<SimpleRule>]) -> Option<&RuleMatch<SimpleRule>> {
            matches.iter().max_by_key(|m| m.rule.priority())
        }
    }

    // ── Mock ReteNetwork ─────────────────────────────────────────────────────

    // Minimal linear-scan network using SimpleRule (which is Clone).
    // Rules with no conditions activate for every asserted fact.
    #[derive(Default)]
    struct MockReteNetwork {
        rules: Vec<SimpleRule>,
        activated: Vec<RuleMatch<SimpleRule>>,
    }

    impl ReteNetwork<TestFact, SimpleRule> for MockReteNetwork {
        fn add_rule(&mut self, rule: SimpleRule) {
            self.rules.push(rule);
        }

        fn remove_rule(&mut self, rule_id: &str) -> Option<SimpleRule> {
            if let Some(pos) = self.rules.iter().position(|r| r.id() == rule_id) {
                self.activated.retain(|m| m.rule.id() != rule_id);
                Some(self.rules.remove(pos))
            } else {
                None
            }
        }

        fn on_assert(&mut self, fact: &TestFact) -> Vec<RuleMatch<SimpleRule>> {
            let ts = Utc::now();
            let fact_id = *fact.id();

            let new_matches: Vec<RuleMatch<SimpleRule>> = self
                .rules
                .iter()
                .map(|rule| RuleMatch {
                    rule: rule.clone(),
                    bindings: EmptyBindings,
                    matched_facts: vec![fact_id],
                    timestamp: ts,
                })
                .collect();

            for m in &new_matches {
                self.activated.push(RuleMatch {
                    rule: m.rule.clone(),
                    bindings: EmptyBindings,
                    matched_facts: m.matched_facts.clone(),
                    timestamp: m.timestamp,
                });
            }

            new_matches
        }

        fn on_retract(&mut self, fact_id: &u32) -> Vec<RuleMatch<SimpleRule>> {
            let removed: Vec<RuleMatch<SimpleRule>> = self
                .activated
                .iter()
                .filter(|m| m.matched_facts.contains(fact_id))
                .map(|m| RuleMatch {
                    rule: m.rule.clone(),
                    bindings: EmptyBindings,
                    matched_facts: m.matched_facts.clone(),
                    timestamp: m.timestamp,
                })
                .collect();

            self.activated
                .retain(|m| !m.matched_facts.contains(fact_id));

            removed
        }

        fn activated(&self) -> &[RuleMatch<SimpleRule>] {
            &self.activated
        }
    }

    // ── Fact trait tests ─────────────────────────────────────────────────────

    #[test]
    fn fact_id_returns_correct_id() {
        let f = make_fact(42, "hello");
        assert_eq!(*f.id(), 42);
    }

    #[test]
    fn fact_timestamp_round_trips() {
        let ts = Utc::now();
        let f = TestFact {
            id: 1,
            value: "x".into(),
            ts,
        };
        assert_eq!(f.timestamp(), ts);
    }

    #[test]
    fn fact_satisfies_required_supertraits() {
        let f = make_fact(1, "a");
        let f2 = f.clone();
        assert_eq!(f, f2);

        let mut set = HashSet::new();
        set.insert(f.clone());
        assert!(set.contains(&f2));

        // Debug must not panic.
        let _ = format!("{:?}", f);
    }

    // ── Condition trait tests ─────────────────────────────────────────────────

    #[test]
    fn always_match_condition_matches_any_fact() {
        let cond = AlwaysMatchCond;
        assert!(cond.matches(&make_fact(1, "anything")).is_some());
        assert!(cond.matches(&make_fact(99, "")).is_some());
    }

    #[test]
    fn never_match_condition_rejects_all_facts() {
        let cond = NeverMatchCond;
        assert!(cond.matches(&make_fact(1, "anything")).is_none());
    }

    #[test]
    fn value_condition_matches_only_expected_value() {
        let cond = ValueMatchCond("critical".into());
        assert!(cond.matches(&make_fact(1, "critical")).is_some());
        assert!(cond.matches(&make_fact(2, "warning")).is_none());
    }

    #[test]
    fn condition_description_is_non_empty() {
        assert!(!AlwaysMatchCond.description().is_empty());
        assert!(!NeverMatchCond.description().is_empty());
        assert!(!ValueMatchCond("x".into()).description().is_empty());
    }

    // ── Rule trait tests ──────────────────────────────────────────────────────

    #[test]
    fn rule_id_priority_description_are_correct() {
        let rule = TestRule {
            id: "pod-restart-rule".into(),
            priority: 10,
            conditions: vec![],
        };
        assert_eq!(rule.id(), "pod-restart-rule");
        assert_eq!(rule.priority(), 10);
        assert_eq!(rule.description(), "test rule");
    }

    #[test]
    fn rule_conditions_slice_has_correct_length_and_descriptions() {
        let rule = TestRule {
            id: "r".into(),
            priority: 0,
            conditions: vec![
                Box::new(AlwaysMatchCond),
                Box::new(ValueMatchCond("foo".into())),
            ],
        };
        assert_eq!(rule.conditions().len(), 2);
        assert_eq!(rule.conditions()[0].description(), "always matches");
        assert_eq!(rule.conditions()[1].description(), "matches specific value");
    }

    #[test]
    fn rule_empty_conditions_returns_empty_slice() {
        let rule = SimpleRule {
            id: "unconditional".into(),
            priority: 1,
        };
        assert!(rule.conditions().is_empty());
    }

    #[test]
    fn rule_actions_returns_correct_actions() {
        let rule = TestRule {
            id: "r".into(),
            priority: 0,
            conditions: vec![],
        };
        let actions = rule.actions(&EmptyBindings);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0], NoopAction);
    }

    // ── RuleMatch tests ───────────────────────────────────────────────────────

    #[test]
    fn rule_match_stores_all_fields() {
        let rule = SimpleRule {
            id: "rule-1".into(),
            priority: 5,
        };
        let ts = Utc::now();
        let rm = RuleMatch {
            rule: rule.clone(),
            bindings: EmptyBindings,
            matched_facts: vec![1u32, 2u32],
            timestamp: ts,
        };

        assert_eq!(rm.rule.id(), "rule-1");
        assert_eq!(rm.rule.priority(), 5);
        assert_eq!(rm.matched_facts, vec![1, 2]);
        assert_eq!(rm.timestamp, ts);
    }

    // ── WorkingMemory trait tests ─────────────────────────────────────────────

    #[test]
    fn assert_fact_returns_true_for_new_fact() {
        let mut mem = MockWorkingMemory::default();
        assert!(mem.assert_fact(make_fact(1, "a")));
    }

    #[test]
    fn assert_fact_returns_false_for_duplicate_id() {
        let mut mem = MockWorkingMemory::default();
        mem.assert_fact(make_fact(1, "a"));
        assert!(!mem.assert_fact(make_fact(1, "b")));
    }

    #[test]
    fn assert_fact_replaces_value_for_existing_id() {
        let mut mem = MockWorkingMemory::default();
        mem.assert_fact(make_fact(1, "original"));
        mem.assert_fact(make_fact(1, "updated"));
        assert_eq!(mem.get(&1).unwrap().value, "updated");
    }

    #[test]
    fn retract_fact_removes_and_returns_it() {
        let mut mem = MockWorkingMemory::default();
        mem.assert_fact(make_fact(1, "hello"));

        assert!(mem.contains(&1));
        let removed = mem.retract_fact(&1);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().value, "hello");
        assert!(!mem.contains(&1));
    }

    #[test]
    fn retract_fact_returns_none_for_missing_id() {
        let mut mem = MockWorkingMemory::default();
        assert!(mem.retract_fact(&99).is_none());
    }

    #[test]
    fn contains_and_get_work_correctly() {
        let mut mem = MockWorkingMemory::default();
        mem.assert_fact(make_fact(7, "data"));

        assert!(mem.contains(&7));
        assert!(!mem.contains(&8));
        assert_eq!(mem.get(&7).unwrap().value, "data");
        assert!(mem.get(&8).is_none());
    }

    #[test]
    fn len_and_is_empty_reflect_current_state() {
        let mut mem = MockWorkingMemory::default();
        assert!(mem.is_empty());
        assert_eq!(mem.len(), 0);

        mem.assert_fact(make_fact(1, "a"));
        assert!(!mem.is_empty());
        assert_eq!(mem.len(), 1);

        mem.assert_fact(make_fact(2, "b"));
        assert_eq!(mem.len(), 2);

        mem.retract_fact(&1);
        assert_eq!(mem.len(), 1);
    }

    #[test]
    fn facts_iterator_yields_all_facts() {
        let mut mem = MockWorkingMemory::default();
        mem.assert_fact(make_fact(1, "a"));
        mem.assert_fact(make_fact(2, "b"));
        mem.assert_fact(make_fact(3, "c"));

        let mut ids: Vec<u32> = mem.facts().map(|f| *f.id()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn facts_iterator_is_empty_when_memory_is_empty() {
        let mem = MockWorkingMemory::default();
        assert_eq!(mem.facts().count(), 0);
    }

    // ── ConflictStrategy trait tests ──────────────────────────────────────────

    fn make_match(id: &str, priority: i32) -> RuleMatch<SimpleRule> {
        RuleMatch {
            rule: SimpleRule {
                id: id.into(),
                priority,
            },
            bindings: EmptyBindings,
            matched_facts: vec![],
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn priority_strategy_selects_highest_priority() {
        let matches = vec![
            make_match("low", 1),
            make_match("high", 100),
            make_match("mid", 50),
        ];

        let selected = PriorityStrategy.select(&matches).unwrap();
        assert_eq!(selected.rule.id(), "high");
    }

    #[test]
    fn priority_strategy_returns_none_for_empty_conflict_set() {
        assert!(PriorityStrategy.select(&[]).is_none());
    }

    #[test]
    fn priority_strategy_single_match_is_always_selected() {
        let matches = vec![make_match("only", 42)];
        assert_eq!(
            PriorityStrategy.select(&matches).unwrap().rule.id(),
            "only"
        );
    }

    // ── ReteNetwork trait tests ───────────────────────────────────────────────

    #[test]
    fn rete_network_starts_with_no_activated_matches() {
        let net = MockReteNetwork::default();
        assert!(net.activated().is_empty());
    }

    #[test]
    fn rete_network_add_and_remove_rules() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "r1".into(),
            priority: 1,
        });
        net.add_rule(SimpleRule {
            id: "r2".into(),
            priority: 2,
        });

        let removed = net.remove_rule("r1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id(), "r1");

        assert!(net.remove_rule("nonexistent").is_none());
    }

    #[test]
    fn rete_network_on_assert_activates_all_unconditional_rules() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "r1".into(),
            priority: 1,
        });
        net.add_rule(SimpleRule {
            id: "r2".into(),
            priority: 2,
        });

        let new_matches = net.on_assert(&make_fact(1, "alert"));

        assert_eq!(new_matches.len(), 2, "both rules should activate");
        assert_eq!(net.activated().len(), 2);
    }

    #[test]
    fn rete_network_on_assert_records_matched_fact_id() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "r1".into(),
            priority: 0,
        });

        let new_matches = net.on_assert(&make_fact(42, "x"));
        assert_eq!(new_matches[0].matched_facts, vec![42]);
    }

    #[test]
    fn rete_network_on_retract_removes_relevant_matches() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "r1".into(),
            priority: 0,
        });

        net.on_assert(&make_fact(1, "x"));
        assert_eq!(net.activated().len(), 1);

        let invalidated = net.on_retract(&1);
        assert_eq!(invalidated.len(), 1);
        assert!(net.activated().is_empty());
    }

    #[test]
    fn rete_network_retract_nonexistent_fact_leaves_activated_intact() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "r1".into(),
            priority: 0,
        });
        net.on_assert(&make_fact(1, "x"));

        let invalidated = net.on_retract(&99);
        assert!(invalidated.is_empty());
        assert_eq!(net.activated().len(), 1);
    }

    #[test]
    fn rete_network_remove_rule_clears_its_matches() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "r1".into(),
            priority: 0,
        });
        net.on_assert(&make_fact(1, "x"));
        assert_eq!(net.activated().len(), 1);

        net.remove_rule("r1");
        assert!(net.activated().is_empty());
    }

    #[test]
    fn conflict_strategy_integrates_with_rete_network() {
        let mut net = MockReteNetwork::default();
        net.add_rule(SimpleRule {
            id: "low-prio".into(),
            priority: 1,
        });
        net.add_rule(SimpleRule {
            id: "high-prio".into(),
            priority: 10,
        });

        net.on_assert(&make_fact(1, "trigger"));

        let selected = PriorityStrategy.select(net.activated()).unwrap();
        assert_eq!(selected.rule.id(), "high-prio");
    }
}
