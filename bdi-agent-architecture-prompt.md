# BDI Incident Response Agent — Architecture & Build Specification

## Design Philosophy

Maximize use of proven crates with thin, well-known crossing points. Minimize custom framework code. The only novel contribution is effect-annotated infrastructure actions with compensation semantics — everything else composes existing libraries.

**Core thesis:** A BDI agent is a loop that consumes a stream of facts, derives patterns via Datalog, searches for action plans via graph search, and executes with effect-tracked backtracking. Every one of these concerns has a mature Rust realization. Don't rebuild them — compose them.

## Architecture at a Glance

```
Observation          Pattern Matching       Planning              Execution
─────────────       ────────────────       ────────             ─────────
tokio_stream::Stream  →  ascent (Datalog)  →  pathfinding (A*)  →  rig-effects (Effect + Compensable)
kube-rs watcher          rules as           closures:              rig::tool::Tool (dual impl)
reqwest polling          ascent! macro       successors             WAL via rusqlite
axum webhooks            lattice priority    heuristic              backtrack on failure
                                             success                escalate via mpsc channel
                              │
                              │ no match?
                              ▼
                         rig (LLM layer)
                         ┌─────────────────────────────────────────┐
                         │ 1. INTERPRET — novel situation analysis  │
                         │    rig::agent::Agent + belief summary    │
                         │    → produces Goal + candidate actions   │
                         │                                          │
                         │ 2. ANALYZE — root cause within runbooks  │
                         │    rig::agent::Agent + observation tools │
                         │    → enriches belief state               │
                         │                                          │
                         │ 3. PROPOSE — action generation           │
                         │    LLM suggests, pathfinding validates   │
                         │    → plan with verified preconditions    │
                         └─────────────────────────────────────────┘
```

Rig enters at three specific points. Ascent handles certainty (known patterns, deterministic rules). Rig handles uncertainty (novel patterns, root cause analysis, action proposals). The deterministic path works without Rig. The LLM path adds capability but is never load-bearing for known incidents.

### Crossing Points Inventory

| Concern | Crossing Point | Realization | Methods |
|---------|---------------|-------------|---------|
| Observation | `Stream<Item = Fact>` | `tokio_stream` | `poll_next` |
| Pattern matching | `ascent!` macro relations | `ascent` crate | `prog.run()`, read derived relations |
| Conflict resolution | Lattice in Datalog | `ascent` lattice support | None — declarative in rules |
| Planning | `successors`, `heuristic`, `success` closures | `pathfinding` crate | `astar()` / `bfs()` / `idastar()` |
| Effect tracking | `Effect` enum, `Compensable` trait | `rig-effects` (custom) | `effect()`, `snapshot()`, `compensate()` |
| Tool execution | `rig::tool::Tool` + `Effectful` | `rig` + `rig-effects` | `call()` + `effect()` — dual impl |
| LLM interpretation | `rig::agent::Agent` | `rig` crate | `prompt()` — novel situation analysis |
| LLM analysis | `rig::agent::Agent` + tools | `rig` crate | `prompt()` with tool use — root cause |
| LLM plan proposal | `rig::completion::CompletionModel` | `rig` crate | `complete()` — suggest actions, pathfinding validates |
| Event persistence | `EventLog` struct | `rusqlite` | `append()`, `events_since()` |
| Escalation | `mpsc::channel` | `tokio::sync` | `send()`, `recv()` |

Eleven concerns. One custom trait (`Compensable`). Rig provides three crossing points: interpretation, analysis-with-tools, and plan proposal. Each uses Rig's existing `Agent` or `CompletionModel` traits — no custom LLM abstractions.

---

## Crate Structure

```
incident-agent-workspace/
├── Cargo.toml                   # workspace
├── rig-effects/                 # custom: effect types + compensation
│   ├── Cargo.toml
│   └── src/lib.rs
├── rig-effects-derive/          # custom: derive macros
│   ├── Cargo.toml
│   └── src/lib.rs
├── agent-core/                  # domain: facts, rules, tools, runbooks, BDI loop
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── facts.rs             # domain fact types
│       ├── rules.rs             # ascent! Datalog rules
│       ├── tools.rs             # effectful infrastructure tools (dual: rig::Tool + Effectful)
│       ├── runbooks.rs          # action schemas for planning
│       ├── streams.rs           # fact streams from kube-rs, datadog, webhooks
│       ├── planner.rs           # wires runbooks into pathfinding closures
│       ├── llm.rs               # Rig agent builders: interpreter, analyzer, proposer
│       ├── executor.rs          # WAL execution with backtracking
│       ├── event_log.rs         # SQLite event log
│       └── agent.rs             # BDI loop: stream → ascent → plan/interpret → execute
├── agent-server/                # webhook server + headless CLI
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── webhook.rs
│       └── main.rs              # headless entry point
├── src-tauri/                   # Tauri backend (optional desktop UI)
└── src/                         # Leptos frontend (optional desktop UI)
```

### Workspace Cargo.toml

```toml
[workspace]
resolver = "2"
members = [
    "rig-effects",
    "rig-effects-derive",
    "agent-core",
    "agent-server",
    "src-tauri",
]
```

---

## Crate 1: `rig-effects`

The only custom crate. Effect annotations for side-effectful operations, enabling safe backtracking over real infrastructure.

**Zero external dependencies beyond serde.** This is the novel contribution — nobody has typed effect tracking for infrastructure actions with compensation semantics.

### Cargo.toml

```toml
[package]
name = "rig-effects"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
```

### src/lib.rs

```rust
use serde::{Serialize, Deserialize};
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
    fn compensate(&self, snapshot: Self::Snapshot) -> impl Future<Output = Result<(), Self::Error>> + Send;
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
```

### Companion crate: `rig-effects-derive`

```rust
// Derive macro for Effectful
// Usage:
//   #[derive(Effectful)]
//   #[effect(Mutate)]
//   struct RestartDeployment { ... }
//
// Generates:
//   impl Effectful for RestartDeployment {
//       fn effect(&self) -> Effect { Effect::Mutate }
//   }
```

### Tests for `rig-effects`

```rust
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
```

---

## Crate 2: `agent-core`

Domain-specific implementation. All infrastructure knowledge lives here.

### Cargo.toml

```toml
[package]
name = "agent-core"
version = "0.1.0"
edition = "2021"

[dependencies]
rig-effects = { path = "../rig-effects" }
rig-core = "0.x"                         # rig LLM framework

# Datalog
ascent = "0.7"

# Planning (graph search)
pathfinding = "4"

# Async runtime & streams
tokio = { version = "1", features = ["full"] }
tokio-stream = { version = "0.1", features = ["sync"] }
futures = "0.3"

# Kubernetes
kube = { version = "0.x", features = ["runtime", "derive"] }
k8s-openapi = { version = "0.x", features = ["latest"] }

# HTTP (webhooks, Datadog, Prometheus APIs)
reqwest = { version = "0.12", features = ["json"] }
axum = "0.8"

# Persistence
rusqlite = { version = "0.32", features = ["bundled"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }

# Logging
tracing = "0.1"
```

### facts.rs — Domain Fact Types

These are plain Rust structs. No framework trait — Ascent relations are tuples.

```rust
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

// ─── Base fact types ───

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PodFact {
    pub name: String,
    pub namespace: String,
    pub phase: PodPhase,
    pub restart_count: u32,
    pub termination_reason: Option<String>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PodPhase { Running, Pending, Failed, Succeeded, Unknown }

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlertFact {
    pub id: String,
    pub source: AlertSource,
    pub severity: Severity,
    pub title: String,
    pub tags: Vec<String>,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertSource { Datadog, PagerDuty, Grafana, CloudWatch, Generic }

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Severity { Info, Low, Medium, High, Critical }

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeployFact {
    pub name: String,
    pub namespace: String,
    pub image: String,
    pub replicas: u32,
    pub available: u32,
    pub revision: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricFact {
    pub name: String,
    pub value: f64,
    pub labels: HashMap<String, String>,
    pub observed_at: DateTime<Utc>,
}

// ─── Union type for stream ───

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Fact {
    Pod(PodFact),
    Alert(AlertFact),
    Deploy(DeployFact),
    Metric(MetricFact),
}
```

### rules.rs — Ascent Datalog Rules

This is the pattern matching engine. Known incident patterns encoded declaratively.

```rust
use ascent::ascent;

ascent! {
    // ─── Input relations (base facts from streams) ───

    relation pod(String, String, String, u32);
    // pod(name, namespace, phase, restart_count)

    relation alert(String, String, String);
    // alert(id, source, severity)

    relation deploy(String, String, String, u32, u32);
    // deploy(name, namespace, revision, replicas, available)

    relation metric(String, f64);
    // metric(name, value)

    relation already_handling(String);
    // already_handling(incident_pattern_id) — prevents re-firing

    // ─── Derived relations (incident patterns) ───

    relation crashloop_detected(String, String);
    // crashloop_detected(pod_name, namespace)
    crashloop_detected(name, ns) <--
        pod(name, ns, phase, restarts),
        (phase == "Running" || phase == "Failed"),
        (restarts > 5),
        !already_handling(format!("crashloop:{}", name));

    relation oomkill_detected(String, String);
    // oomkill_detected(pod_name, namespace)
    oomkill_detected(name, ns) <--
        pod(name, ns, _, _),
        alert(_, _, sev),
        (sev == "Critical" || sev == "High");
        // Real impl: match on termination_reason = "OOMKilled"
        // Simplified here — actual impl uses richer tuple structure

    relation high_error_rate(String);
    // high_error_rate(service_name)
    high_error_rate(svc) <--
        metric(name, value),
        (name.starts_with("error_rate")),
        (value > 0.05),
        let svc = name.trim_start_matches("error_rate:").to_string();

    relation suspect_bad_deploy(String, String);
    // suspect_bad_deploy(deploy_name, namespace)
    suspect_bad_deploy(name, ns) <--
        deploy(name, ns, _, replicas, available),
        (available < replicas);

    relation deploy_correlated_error(String, String);
    // deploy_correlated_error(deploy_name, namespace)
    deploy_correlated_error(deploy, ns) <--
        high_error_rate(_svc),
        suspect_bad_deploy(deploy, ns);

    // ─── Priority via lattice ───
    // Lower number = higher priority (Dual inverts the lattice)
    use ascent::Dual;

    lattice best_incident(String, String, Dual<u32>);
    // best_incident(incident_id, runbook_name, priority)

    best_incident(
        format!("crashloop:{}", name),
        "crashloop_runbook".to_string(),
        Dual(1)
    ) <-- crashloop_detected(name, _ns);

    best_incident(
        format!("oomkill:{}", name),
        "oomkill_runbook".to_string(),
        Dual(2)
    ) <-- oomkill_detected(name, _ns);

    best_incident(
        format!("bad_deploy:{}", name),
        "rollback_runbook".to_string(),
        Dual(3)
    ) <-- deploy_correlated_error(name, _ns);

    best_incident(
        format!("error_rate:{}", svc),
        "high_error_rate_runbook".to_string(),
        Dual(4)
    ) <-- high_error_rate(svc);
}
```

The `AscentProgram` struct generated by the macro is your fact store. Insert facts into input relations, call `prog.run()`, read derived relations. Three operations. That's the Datalog crossing point.

### tools.rs — Dual-Implemented Infrastructure Tools

Every tool implements BOTH `rig::tool::Tool` (so the LLM can call them during analysis) AND `Effectful` (so the executor knows their effect classification). This dual implementation is the bridge between the LLM world and the effect-tracked execution world.

```rust
use rig::tool::{Tool, ToolEmbedding};
use rig::completion::ToolDefinition;
use rig_effects::{Effect, Effectful, Compensable};
use serde::{Serialize, Deserialize};
use serde_json::json;

// ─── Observation tools (Effect::Observe) ───
// LLM can call these freely during analysis — no side effects to worry about.

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPodLogs {
    pub namespace: String,
    pub pod: String,
    pub lines: u32,
}

impl Effectful for GetPodLogs {
    fn effect(&self) -> Effect { Effect::Observe }
}

impl Tool for GetPodLogs {
    const NAME: &'static str = "get_pod_logs";

    type Error = anyhow::Error;
    type Args = GetPodLogsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Retrieve recent logs from a Kubernetes pod".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "namespace": { "type": "string", "description": "Kubernetes namespace" },
                    "pod": { "type": "string", "description": "Pod name" },
                    "lines": { "type": "integer", "description": "Number of log lines", "default": 100 }
                },
                "required": ["namespace", "pod"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<String, Self::Error> {
        // kubectl logs -n {namespace} {pod} --tail={lines}
        let client = kube::Client::try_default().await?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client, &args.namespace);
        let logs = pods.logs(&args.pod, &kube::api::LogParams {
            tail_lines: Some(args.lines as i64),
            ..Default::default()
        }).await?;
        Ok(logs)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPodLogsArgs {
    pub namespace: String,
    pub pod: String,
    #[serde(default = "default_lines")]
    pub lines: u32,
}
fn default_lines() -> u32 { 100 }

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPodEvents {
    pub namespace: String,
    pub pod: String,
}

impl Effectful for GetPodEvents {
    fn effect(&self) -> Effect { Effect::Observe }
}

impl Tool for GetPodEvents {
    const NAME: &'static str = "get_pod_events";
    type Error = anyhow::Error;
    type Args = GetPodEventsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Retrieve Kubernetes events for a specific pod".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "pod": { "type": "string" }
                },
                "required": ["namespace", "pod"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<String, Self::Error> {
        // kubectl get events -n {namespace} --field-selector involvedObject.name={pod}
        todo!()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPodEventsArgs {
    pub namespace: String,
    pub pod: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub query: String,
    pub window_minutes: u32,
}

impl Effectful for QueryMetrics {
    fn effect(&self) -> Effect { Effect::Observe }
}

impl Tool for QueryMetrics {
    const NAME: &'static str = "query_metrics";
    type Error = anyhow::Error;
    type Args = QueryMetricsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Query infrastructure metrics (Prometheus/Datadog)".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "PromQL or Datadog metric query" },
                    "window_minutes": { "type": "integer", "description": "Lookback window in minutes", "default": 30 }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<String, Self::Error> {
        // reqwest GET prometheus/api/v1/query
        todo!()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryMetricsArgs {
    pub query: String,
    #[serde(default = "default_window")]
    pub window_minutes: u32,
}
fn default_window() -> u32 { 30 }

#[derive(Debug, Serialize, Deserialize)]
pub struct GetDeployHistory {
    pub namespace: String,
    pub deployment: String,
}

impl Effectful for GetDeployHistory {
    fn effect(&self) -> Effect { Effect::Observe }
}

impl Tool for GetDeployHistory {
    const NAME: &'static str = "get_deploy_history";
    type Error = anyhow::Error;
    type Args = GetDeployHistoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get recent deployment rollout history".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment": { "type": "string" }
                },
                "required": ["namespace", "deployment"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<String, Self::Error> {
        // kubectl rollout history deployment/{name} -n {namespace}
        todo!()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetDeployHistoryArgs {
    pub namespace: String,
    pub deployment: String,
}

// ─── Mutation tools (Effect::Mutate) — compensable ───
// LLM CANNOT call these directly. Only the executor calls Mutate tools.
// They implement rig::Tool so the LLM knows they exist and can propose them,
// but actual invocation goes through the effect-tracked executor.

#[derive(Debug, Serialize, Deserialize)]
pub struct RollbackDeployment {
    pub namespace: String,
    pub deployment: String,
    pub to_revision: Option<String>,
}

impl Effectful for RollbackDeployment {
    fn effect(&self) -> Effect { Effect::Mutate }
}

impl Compensable for RollbackDeployment {
    type Snapshot = String;
    type Error = anyhow::Error;

    async fn snapshot(&self) -> Result<String, Self::Error> {
        // Capture current revision before rollback
        let client = kube::Client::try_default().await?;
        let deploys: kube::Api<k8s_openapi::api::apps::v1::Deployment> =
            kube::Api::namespaced(client, &self.namespace);
        let deploy = deploys.get(&self.deployment).await?;
        let revision = deploy.metadata.annotations
            .as_ref()
            .and_then(|a| a.get("deployment.kubernetes.io/revision"))
            .cloned()
            .unwrap_or_default();
        Ok(revision)
    }

    async fn compensate(&self, from_revision: String) -> Result<(), Self::Error> {
        // Roll forward to the revision we captured before rollback
        tracing::info!("Compensating rollback: re-deploying revision {}", from_revision);
        // kubectl rollout undo deployment/{name} --to-revision={from_revision}
        todo!()
    }
}

impl Tool for RollbackDeployment {
    const NAME: &'static str = "rollback_deployment";
    type Error = anyhow::Error;
    type Args = RollbackDeploymentArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Rollback a Kubernetes deployment to a previous revision. MUTATING — requires effect tracking.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment": { "type": "string" },
                    "to_revision": { "type": "string", "description": "Target revision (omit for previous)" }
                },
                "required": ["namespace", "deployment"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<String, Self::Error> {
        // kubectl rollout undo deployment/{name} -n {namespace}
        todo!()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RollbackDeploymentArgs {
    pub namespace: String,
    pub deployment: String,
    pub to_revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScaleDeployment {
    pub namespace: String,
    pub deployment: String,
    pub replicas: u32,
}

impl Effectful for ScaleDeployment {
    fn effect(&self) -> Effect { Effect::Mutate }
}

impl Compensable for ScaleDeployment {
    type Snapshot = u32;
    type Error = anyhow::Error;

    async fn snapshot(&self) -> Result<u32, Self::Error> {
        let client = kube::Client::try_default().await?;
        let deploys: kube::Api<k8s_openapi::api::apps::v1::Deployment> =
            kube::Api::namespaced(client, &self.namespace);
        let deploy = deploys.get(&self.deployment).await?;
        Ok(deploy.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1) as u32)
    }

    async fn compensate(&self, original: u32) -> Result<(), Self::Error> {
        tracing::info!("Compensating scale: restoring to {} replicas", original);
        // kubectl scale deployment/{name} --replicas={original}
        todo!()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeletePod {
    pub namespace: String,
    pub pod: String,
}
impl Effectful for DeletePod { fn effect(&self) -> Effect { Effect::Mutate } }

#[derive(Debug, Serialize, Deserialize)]
pub struct RestartDeployment {
    pub namespace: String,
    pub deployment: String,
}
impl Effectful for RestartDeployment { fn effect(&self) -> Effect { Effect::Mutate } }

// ─── Irreversible tools ───

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackNotify {
    pub channel: String,
    pub message: String,
}
impl Effectful for SlackNotify { fn effect(&self) -> Effect { Effect::Irreversible } }

#[derive(Debug, Serialize, Deserialize)]
pub struct PagerDutyEscalate {
    pub incident_id: String,
    pub severity: String,
    pub summary: String,
}
impl Effectful for PagerDutyEscalate { fn effect(&self) -> Effect { Effect::Irreversible } }
```

**Key design rule:** Observation tools (`Effect::Observe`) are safe for the LLM to call autonomously during analysis. Mutation tools (`Effect::Mutate`) are exposed to the LLM as tool *definitions* (so it can propose them in plans) but actual execution goes through the effect-tracked executor. The LLM suggests, the planner validates, the executor runs with WAL and compensation.

### runbooks.rs — Action Schemas for Planning

A runbook is a sequence of action schemas with STRIPS-like preconditions and effects. The `pathfinding` crate's successors closure consumes these.

```rust
use rig_effects::Effect;

/// A predicate spec for display/logging.
#[derive(Clone, Debug)]
pub struct PredicateSpec {
    pub name: String,
}

/// An action schema: what an agent can do, when, and what it costs.
#[derive(Clone)]
pub struct ActionSchema {
    pub name: String,
    pub effect: Effect,
    pub preconditions: Vec<PredicateSpec>,
    pub add_effects: Vec<String>,
    pub delete_effects: Vec<String>,
    pub check_preconditions: fn(&BeliefState) -> bool,
    pub estimated_cost: u32,
}

impl ActionSchema {
    pub fn weighted_cost(&self) -> u32 {
        self.estimated_cost * self.effect.cost_weight()
    }
}

/// Snapshot of belief state for planning.
/// This is the "state" node that pathfinding searches over.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct BeliefState {
    pub propositions: std::collections::BTreeSet<String>,
}

impl BeliefState {
    pub fn apply(&self, action: &ActionSchema) -> BeliefState {
        let mut next = self.clone();
        for del in &action.delete_effects {
            next.propositions.remove(del);
        }
        for add in &action.add_effects {
            next.propositions.insert(add.clone());
        }
        next
    }

    pub fn has(&self, prop: &str) -> bool {
        self.propositions.contains(prop)
    }
}

// ─── Runbook definitions ───

pub fn crashloop_runbook() -> Vec<ActionSchema> {
    vec![
        ActionSchema {
            name: "get_pod_logs".into(),
            effect: Effect::Observe,
            preconditions: vec![pred("crashloop_detected")],
            add_effects: vec!["has_pod_logs".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("crashloop_detected"),
            estimated_cost: 1,
        },
        ActionSchema {
            name: "get_pod_events".into(),
            effect: Effect::Observe,
            preconditions: vec![pred("crashloop_detected")],
            add_effects: vec!["has_pod_events".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("crashloop_detected"),
            estimated_cost: 1,
        },
        ActionSchema {
            name: "check_recent_deploys".into(),
            effect: Effect::Observe,
            preconditions: vec![],
            add_effects: vec!["has_deploy_history".into()],
            delete_effects: vec![],
            check_preconditions: |_| true,
            estimated_cost: 1,
        },
        ActionSchema {
            name: "analyze_logs_llm".into(),
            effect: Effect::Pure,
            preconditions: vec![pred("has_pod_logs"), pred("has_pod_events")],
            add_effects: vec!["has_root_cause_analysis".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("has_pod_logs") && s.has("has_pod_events"),
            estimated_cost: 5,
        },
        ActionSchema {
            name: "rollback_deployment".into(),
            effect: Effect::Mutate,
            preconditions: vec![pred("has_root_cause_analysis"), pred("has_deploy_history")],
            add_effects: vec!["rollback_applied".into()],
            delete_effects: vec!["crashloop_detected".into()],
            check_preconditions: |s| s.has("has_root_cause_analysis") && s.has("has_deploy_history"),
            estimated_cost: 3,
        },
        ActionSchema {
            name: "verify_recovery".into(),
            effect: Effect::Observe,
            preconditions: vec![pred("rollback_applied")],
            add_effects: vec!["recovery_verified".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("rollback_applied"),
            estimated_cost: 2,
        },
    ]
}

pub fn oomkill_runbook() -> Vec<ActionSchema> {
    vec![
        ActionSchema {
            name: "get_resource_limits".into(),
            effect: Effect::Observe,
            preconditions: vec![pred("oomkill_detected")],
            add_effects: vec!["has_resource_limits".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("oomkill_detected"),
            estimated_cost: 1,
        },
        ActionSchema {
            name: "get_memory_history".into(),
            effect: Effect::Observe,
            preconditions: vec![pred("oomkill_detected")],
            add_effects: vec!["has_memory_history".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("oomkill_detected"),
            estimated_cost: 1,
        },
        ActionSchema {
            name: "rollback_or_increase_limits".into(),
            effect: Effect::Mutate,
            preconditions: vec![pred("has_resource_limits"), pred("has_memory_history")],
            add_effects: vec!["remediation_applied".into()],
            delete_effects: vec!["oomkill_detected".into()],
            check_preconditions: |s| s.has("has_resource_limits") && s.has("has_memory_history"),
            estimated_cost: 5,
        },
        ActionSchema {
            name: "monitor_recovery".into(),
            effect: Effect::Observe,
            preconditions: vec![pred("remediation_applied")],
            add_effects: vec!["recovery_verified".into()],
            delete_effects: vec![],
            check_preconditions: |s| s.has("remediation_applied"),
            estimated_cost: 2,
        },
    ]
}

fn pred(name: &str) -> PredicateSpec {
    PredicateSpec { name: name.into() }
}
```

### planner.rs — Wiring Runbooks into pathfinding

No custom planner trait. The `pathfinding` crate is the planner. This module provides the closures.

```rust
use pathfinding::prelude::astar;
use crate::runbooks::{ActionSchema, BeliefState};

pub struct Plan {
    pub steps: Vec<ActionSchema>,
    pub total_cost: u32,
}

/// Plan a sequence of actions from initial state to goal.
/// Uses A* search over belief states with runbook actions as transitions.
pub fn plan(
    initial: &BeliefState,
    goal_props: &[String],
    available_actions: &[ActionSchema],
) -> Option<Plan> {
    let goal_set: std::collections::BTreeSet<String> = goal_props.iter().cloned().collect();

    let result = astar(
        initial,
        // successors: which states can we reach from here?
        |state| {
            available_actions.iter()
                .filter(|a| (a.check_preconditions)(state))
                .map(|a| {
                    let next = state.apply(a);
                    let cost = a.weighted_cost();
                    (next, cost)
                })
                .collect::<Vec<_>>()
        },
        // heuristic: how far from goal? (count unsatisfied goal propositions)
        |state| {
            goal_set.iter()
                .filter(|g| !state.propositions.contains(g.as_str()))
                .count() as u32
        },
        // success: are we at the goal?
        |state| goal_set.iter().all(|g| state.propositions.contains(g.as_str())),
    );

    result.map(|(path, cost)| {
        let steps = extract_actions_from_path(&path, available_actions);
        Plan { steps, total_cost: cost }
    })
}

fn extract_actions_from_path(
    path: &[BeliefState],
    actions: &[ActionSchema],
) -> Vec<ActionSchema> {
    path.windows(2)
        .filter_map(|pair| {
            let (from, to) = (&pair[0], &pair[1]);
            actions.iter().find(|a| {
                (a.check_preconditions)(from) && from.apply(a) == *to
            }).cloned()
        })
        .collect()
}
```

### streams.rs — Fact Streams from Infrastructure

No custom `BeliefSource` trait. Just functions that return `impl Stream<Item = Fact>`.

### llm.rs — Rig Agent Builders

Three Rig agents, each serving a distinct role in the BDI cycle. Built using `rig`'s `AgentBuilder` API.

```rust
use rig::providers::openai;
use rig::agent::Agent;
use rig::completion::Prompt;
use crate::tools::*;
use crate::runbooks::{ActionSchema, BeliefState};
use crate::facts::Fact;
use rig_effects::Effect;

/// Configuration for the LLM layer.
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub model: String,
}

pub enum LlmProvider {
    OpenAi,
    Anthropic,
}

// ─────────────────────────────────────────────────────────
// Agent 1: INTERPRETER
// Role: When no Ascent rule fires, analyze the belief state
// and determine what's going on + what goal to pursue.
// ─────────────────────────────────────────────────────────

/// Build an interpreter agent. No tools — pure reasoning over a belief summary.
pub fn build_interpreter(config: &LlmConfig) -> impl Agent {
    // Using OpenAI as example — swap provider based on config
    let client = openai::Client::from_env();

    client
        .agent(&config.model)
        .preamble(
            "You are an expert SRE analyzing infrastructure incidents. \
             You will receive a summary of the current system state (beliefs) \
             including pod statuses, alerts, deployments, and metrics. \
             \
             Your job is to: \
             1. Identify what is likely going wrong (root cause hypothesis). \
             2. Assess severity (critical/high/medium/low). \
             3. Suggest a remediation goal as a single sentence. \
             4. List 2-5 candidate remediation actions from the available action set. \
             \
             Respond in JSON format: \
             { \
               \"hypothesis\": \"...\", \
               \"severity\": \"critical|high|medium|low\", \
               \"goal\": \"...\", \
               \"suggested_actions\": [\"action_name_1\", \"action_name_2\"] \
             }"
        )
        .build()
}

/// Interpretation result parsed from LLM response.
#[derive(Debug, serde::Deserialize)]
pub struct Interpretation {
    pub hypothesis: String,
    pub severity: String,
    pub goal: String,
    pub suggested_actions: Vec<String>,
}

/// Run interpretation: summarize beliefs, ask the LLM what's happening.
pub async fn interpret(
    agent: &impl Agent,
    beliefs: &BeliefState,
    recent_facts: &[Fact],
) -> Result<Interpretation, anyhow::Error> {
    let prompt = format!(
        "Current system beliefs:\n{}\n\nRecent facts:\n{}\n\nAvailable actions: \
         get_pod_logs, get_pod_events, query_metrics, get_deploy_history, \
         rollback_deployment, scale_deployment, restart_deployment, delete_pod\n\n\
         What is going on and what should we do?",
        format_beliefs(beliefs),
        format_recent_facts(recent_facts),
    );

    let response = agent.prompt(&prompt).await?;
    let interpretation: Interpretation = serde_json::from_str(&response)?;
    Ok(interpretation)
}

// ─────────────────────────────────────────────────────────
// Agent 2: ANALYZER
// Role: Deep-dive root cause analysis within a runbook step.
// Has access to Observe tools — can pull logs, events, metrics.
// This is the "analyze_logs_llm" step in crashloop_runbook.
// ─────────────────────────────────────────────────────────

/// Build an analyzer agent WITH observation tools.
/// The LLM can autonomously call Observe tools to gather more context.
pub fn build_analyzer(config: &LlmConfig) -> impl Agent {
    let client = openai::Client::from_env();

    client
        .agent(&config.model)
        .preamble(
            "You are an expert SRE performing root cause analysis. \
             You have access to tools for reading pod logs, events, and metrics. \
             Use these tools to gather evidence, then synthesize a root cause analysis. \
             \
             Respond in JSON format: \
             { \
               \"root_cause\": \"...\", \
               \"confidence\": 0.0-1.0, \
               \"evidence\": [\"fact1\", \"fact2\"], \
               \"recommended_action\": \"action_name\", \
               \"reasoning\": \"...\" \
             }"
        )
        .tool(GetPodLogs {
            namespace: String::new(),
            pod: String::new(),
            lines: 100,
        })
        .tool(GetPodEvents {
            namespace: String::new(),
            pod: String::new(),
        })
        .tool(QueryMetrics {
            query: String::new(),
            window_minutes: 30,
        })
        .tool(GetDeployHistory {
            namespace: String::new(),
            deployment: String::new(),
        })
        .build()
}

/// Analysis result parsed from LLM response.
#[derive(Debug, serde::Deserialize)]
pub struct Analysis {
    pub root_cause: String,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub recommended_action: String,
    pub reasoning: String,
}

/// Run analysis: give the LLM context + tools, let it investigate.
pub async fn analyze(
    agent: &impl Agent,
    context: &str,
) -> Result<Analysis, anyhow::Error> {
    let response = agent.prompt(context).await?;
    let analysis: Analysis = serde_json::from_str(&response)?;
    Ok(analysis)
}

// ─────────────────────────────────────────────────────────
// Agent 3: PROPOSER
// Role: Given a novel situation (no runbook match), propose
// a sequence of actions. These get validated by pathfinding.
// ─────────────────────────────────────────────────────────

/// Build a proposer agent. No tools — proposes action sequences
/// that the planner validates against preconditions.
pub fn build_proposer(config: &LlmConfig) -> impl Agent {
    let client = openai::Client::from_env();

    client
        .agent(&config.model)
        .preamble(
            "You are an expert SRE creating remediation plans. \
             Given a system state and a goal, propose an ordered sequence of actions. \
             \
             Available actions and their effects: \
             - get_pod_logs (Observe): Read pod logs \
             - get_pod_events (Observe): Read pod events \
             - query_metrics (Observe): Query Prometheus/Datadog metrics \
             - get_deploy_history (Observe): Check deployment revision history \
             - rollback_deployment (Mutate): Roll back to previous deployment revision \
             - scale_deployment (Mutate): Change replica count \
             - restart_deployment (Mutate): Rolling restart of deployment \
             - delete_pod (Mutate): Delete a specific pod \
             - slack_notify (Irreversible): Send Slack notification \
             - pagerduty_escalate (Irreversible): Escalate via PagerDuty \
             \
             Rules: \
             1. Always gather information (Observe) before making changes (Mutate). \
             2. Minimize Mutate actions — each one has a cost. \
             3. Never propose Irreversible actions unless the situation is critical. \
             4. End with a verification step (Observe) to confirm remediation worked. \
             \
             Respond as a JSON array of action names: \
             [\"get_pod_logs\", \"get_pod_events\", \"rollback_deployment\", \"query_metrics\"]"
        )
        .build()
}

/// Propose actions for a novel situation, then validate with pathfinding.
pub async fn propose_and_validate(
    agent: &impl Agent,
    beliefs: &BeliefState,
    goal: &str,
    all_actions: &[ActionSchema],
) -> Result<Option<Vec<ActionSchema>>, anyhow::Error> {
    let prompt = format!(
        "Current state:\n{}\n\nGoal: {}\n\nPropose a remediation plan.",
        format_beliefs(beliefs),
        goal,
    );

    let response = agent.prompt(&prompt).await?;
    let proposed_names: Vec<String> = serde_json::from_str(&response)?;

    // Filter available actions to only those the LLM proposed, preserving order
    let proposed_actions: Vec<ActionSchema> = proposed_names.iter()
        .filter_map(|name| all_actions.iter().find(|a| a.name == *name).cloned())
        .collect();

    // Validate with pathfinding — does this sequence actually reach the goal?
    let plan = crate::planner::plan(beliefs, &[goal.to_string()], &proposed_actions);

    match plan {
        Some(p) => Ok(Some(p.steps)),
        None => {
            tracing::warn!("LLM-proposed plan failed validation — actions don't reach goal");
            // Fallback: give pathfinding ALL actions and let it find its own plan
            let fallback = crate::planner::plan(beliefs, &[goal.to_string()], all_actions);
            Ok(fallback.map(|p| p.steps))
        }
    }
}

// ─── Helpers ───

fn format_beliefs(state: &BeliefState) -> String {
    state.propositions.iter()
        .map(|p| format!("  - {}", p))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_recent_facts(facts: &[Fact]) -> String {
    facts.iter()
        .map(|f| match f {
            Fact::Pod(p) => format!("  [Pod] {} in {} — {:?}, {} restarts", p.name, p.namespace, p.phase, p.restart_count),
            Fact::Alert(a) => format!("  [Alert] {} — {:?} {:?}", a.title, a.severity, a.source),
            Fact::Deploy(d) => format!("  [Deploy] {}/{} — rev {} ({}/{})", d.namespace, d.name, d.revision, d.available, d.replicas),
            Fact::Metric(m) => format!("  [Metric] {} = {:.2}", m.name, m.value),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

**How the three agents compose:**

1. **Interpreter** is called in `agent.rs` when Ascent has no match. It reads the belief state and proposes a goal + candidate actions. Pure reasoning, no tool use.

2. **Analyzer** is called during plan execution, specifically for the `analyze_logs_llm` step in runbooks. It has access to `Observe` tools and can autonomously pull logs, events, and metrics to form a root cause analysis. This is where Rig's tool-calling loop is most valuable.

3. **Proposer** is called after the Interpreter identifies a goal but no runbook exists. It proposes an action sequence that `pathfinding` then validates. If the LLM's plan doesn't satisfy preconditions, pathfinding falls back to its own A* search over all available actions.

```rust
use tokio_stream::Stream;
use tokio_stream::wrappers::{ReceiverStream, IntervalStream};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use futures::StreamExt;
use crate::facts::*;

/// Kubernetes pod watcher — returns a stream of PodFact.
pub fn kube_pod_stream(
    client: kube::Client,
    namespaces: Vec<String>,
) -> impl Stream<Item = Fact> {
    // kube-rs watcher already returns a Stream
    // Map watcher::Event<Pod> → Fact::Pod(PodFact)
    todo!()
}

/// Prometheus metrics poller — polls at interval, returns MetricFact stream.
pub fn prometheus_stream(
    endpoint: String,
    queries: Vec<String>,
    poll_interval: Duration,
) -> impl Stream<Item = Fact> {
    IntervalStream::new(interval(poll_interval))
        .then(move |_| {
            let endpoint = endpoint.clone();
            let queries = queries.clone();
            async move {
                // reqwest GET /api/v1/query for each query
                // Map response → Vec<Fact::Metric(MetricFact)>
                todo!()
            }
        })
        .flat_map(futures::stream::iter)
}

/// Datadog metrics poller.
pub fn datadog_stream(
    api_key: String,
    app_key: String,
    queries: Vec<String>,
    poll_interval: Duration,
) -> impl Stream<Item = Fact> {
    IntervalStream::new(interval(poll_interval))
        .then(move |_| {
            let api_key = api_key.clone();
            let app_key = app_key.clone();
            let queries = queries.clone();
            async move {
                // reqwest POST to Datadog metrics API
                // Map response → Vec<Fact::Metric(MetricFact)>
                todo!()
            }
        })
        .flat_map(futures::stream::iter)
}

/// Webhook receiver — returns (sender, stream).
/// The sender is given to the axum webhook handler.
/// The stream is consumed by the BDI loop.
pub fn webhook_channel(buffer: usize) -> (mpsc::Sender<Fact>, impl Stream<Item = Fact>) {
    let (tx, rx) = mpsc::channel(buffer);
    (tx, ReceiverStream::new(rx))
}

/// Merge all fact streams into one.
pub fn merge_streams(
    streams: Vec<Box<dyn Stream<Item = Fact> + Send + Unpin>>,
) -> impl Stream<Item = Fact> {
    futures::stream::select_all(streams)
}
```

### event_log.rs — SQLite Event Persistence

Concrete struct, no trait. One implementation until you need a second.

```rust
use rusqlite::{Connection, params};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

pub struct EventLog {
    conn: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: Option<i64>,
    pub incident_id: String,
    pub event_type: EventType,
    pub description: String,
    pub details: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventType {
    FactAsserted,
    FactRetracted,
    PatternMatched { pattern: String },
    PlanSelected { runbook: String },
    ActionIntent { action: String, effect: String },
    ActionResult { action: String, success: bool, error: Option<String> },
    SnapshotCaptured { action: String },
    CompensationExecuted { action: String },
    BacktrackInitiated { from_step: usize, reason: String },
    Escalated { reason: String },
    Resolved,
}

impl EventLog {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                incident_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                description TEXT NOT NULL,
                details TEXT,
                timestamp TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_incident ON events(incident_id);
            CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
        ")?;
        Ok(Self { conn })
    }

    pub fn append(&self, event: &Event) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO events (incident_id, event_type, description, details, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.incident_id,
                serde_json::to_string(&event.event_type).unwrap(),
                event.description,
                event.details.as_ref().map(|d| d.to_string()),
                event.timestamp.to_rfc3339(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn events_for_incident(&self, incident_id: &str) -> rusqlite::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, incident_id, event_type, description, details, timestamp
             FROM events WHERE incident_id = ?1 ORDER BY id ASC"
        )?;
        let events = stmt.query_map(params![incident_id], |row| {
            Ok(Event {
                id: Some(row.get(0)?),
                incident_id: row.get(1)?,
                event_type: serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
                description: row.get(3)?,
                details: row.get::<_, Option<String>>(4)?
                    .map(|s| serde_json::from_str(&s).unwrap()),
                timestamp: row.get::<_, String>(5)?.parse().unwrap(),
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    pub fn active_incidents(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT incident_id FROM events
             WHERE incident_id NOT IN (
                 SELECT incident_id FROM events
                 WHERE event_type LIKE '%Resolved%' OR event_type LIKE '%Escalated%'
             )"
        )?;
        let ids = stmt.query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }
}
```

### executor.rs — WAL Execution with Backtracking

```rust
use rig_effects::Effect;
use crate::event_log::{EventLog, Event, EventType};
use crate::runbooks::ActionSchema;

pub enum ExecutionResult {
    Resolved,
    FailedAtStep { step_index: usize, error: String },
}

struct CompensationEntry {
    step_index: usize,
    action_name: String,
    snapshot: serde_json::Value,
}

/// Execute a plan with WAL semantics and compensation tracking.
pub async fn execute_plan(
    plan: &[ActionSchema],
    incident_id: &str,
    log: &EventLog,
    tool_executor: &dyn Fn(&ActionSchema) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + '_>
    >,
) -> ExecutionResult {
    let mut compensation_stack: Vec<CompensationEntry> = Vec::new();

    for (i, step) in plan.iter().enumerate() {
        // WAL: log intent BEFORE execution
        log.append(&Event {
            id: None,
            incident_id: incident_id.to_string(),
            event_type: EventType::ActionIntent {
                action: step.name.clone(),
                effect: format!("{:?}", step.effect),
            },
            description: format!("Executing step {}: {}", i, step.name),
            details: None,
            timestamp: chrono::Utc::now(),
        }).unwrap();

        match tool_executor(step).await {
            Ok(output) => {
                log.append(&Event {
                    id: None,
                    incident_id: incident_id.to_string(),
                    event_type: EventType::ActionResult {
                        action: step.name.clone(),
                        success: true,
                        error: None,
                    },
                    description: format!("Step {} succeeded: {}", i, step.name),
                    details: Some(output.clone()),
                    timestamp: chrono::Utc::now(),
                }).unwrap();

                if step.effect == Effect::Mutate {
                    compensation_stack.push(CompensationEntry {
                        step_index: i,
                        action_name: step.name.clone(),
                        snapshot: output,
                    });
                }
            }
            Err(error) => {
                log.append(&Event {
                    id: None,
                    incident_id: incident_id.to_string(),
                    event_type: EventType::ActionResult {
                        action: step.name.clone(),
                        success: false,
                        error: Some(error.clone()),
                    },
                    description: format!("Step {} failed: {}: {}", i, step.name, error),
                    details: None,
                    timestamp: chrono::Utc::now(),
                }).unwrap();

                return ExecutionResult::FailedAtStep { step_index: i, error };
            }
        }
    }

    ExecutionResult::Resolved
}

/// Compensate executed Mutate actions in reverse order.
pub async fn compensate(
    stack: &[CompensationEntry],
    incident_id: &str,
    log: &EventLog,
    compensator: &dyn Fn(&str, &serde_json::Value) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>
    >,
) {
    for entry in stack.iter().rev() {
        log.append(&Event {
            id: None,
            incident_id: incident_id.to_string(),
            event_type: EventType::BacktrackInitiated {
                from_step: entry.step_index,
                reason: "compensating after failure".into(),
            },
            description: format!("Compensating step {}: {}", entry.step_index, entry.action_name),
            details: None,
            timestamp: chrono::Utc::now(),
        }).unwrap();

        match compensator(&entry.action_name, &entry.snapshot).await {
            Ok(()) => {
                log.append(&Event {
                    id: None,
                    incident_id: incident_id.to_string(),
                    event_type: EventType::CompensationExecuted {
                        action: entry.action_name.clone(),
                    },
                    description: format!("Compensation succeeded for step {}", entry.step_index),
                    details: None,
                    timestamp: chrono::Utc::now(),
                }).unwrap();
            }
            Err(e) => {
                tracing::error!("Compensation failed for {}: {}", entry.action_name, e);
            }
        }
    }
}
```

### agent.rs — The BDI Loop

The composition layer. Stream → Ascent → plan/interpret → execute → backtrack. Rig enters at step 2b (interpret) and step 3b (propose).

```rust
use tokio_stream::StreamExt;
use tokio::sync::mpsc;
use crate::facts::Fact;
use crate::rules::AscentProgram;
use crate::runbooks::{ActionSchema, BeliefState};
use crate::planner;
use crate::llm::{self, LlmConfig, Interpretation};
use crate::executor::{self, ExecutionResult};
use crate::event_log::{EventLog, Event, EventType};

pub struct AgentConfig {
    pub max_replan_attempts: u32,
    pub runbooks: Vec<(&'static str, Vec<ActionSchema>)>,
    pub all_actions: Vec<ActionSchema>,  // flat list of all available actions for LLM proposals
    pub goal_props: Vec<String>,
    pub llm: Option<LlmConfig>,  // None = deterministic only, no LLM fallback
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct EscalationRequest {
    pub incident_id: String,
    pub reason: String,
}

/// Run the BDI agent loop.
pub async fn run_agent(
    mut facts: impl tokio_stream::Stream<Item = Fact> + Unpin,
    config: AgentConfig,
    log: EventLog,
    escalation_tx: mpsc::Sender<EscalationRequest>,
    tool_executor: impl Fn(&ActionSchema) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + '_>
    >,
) {
    let mut prog = AscentProgram::default();
    let mut recent_facts: Vec<Fact> = Vec::new();

    // Build LLM agents if configured
    let interpreter = config.llm.as_ref().map(|c| llm::build_interpreter(c));
    let proposer = config.llm.as_ref().map(|c| llm::build_proposer(c));

    while let Some(fact) = facts.next().await {
        // Track recent facts for LLM context
        recent_facts.push(fact.clone());
        if recent_facts.len() > 50 { recent_facts.remove(0); }

        // 1. OBSERVE — insert fact into Datalog
        insert_fact(&mut prog, &fact);
        prog.run();

        // 2a. MATCH — check Ascent derived relations (deterministic path)
        let best = prog.best_incident.iter().next().cloned();

        let (incident_id, plan_steps) = if let Some((id, runbook_name, _priority)) = best {
            // Known pattern — look up runbook
            let id = id.clone();
            let runbook_name = runbook_name.clone();

            prog.already_handling.push((id.clone(),));
            prog.run();

            let runbook = config.runbooks.iter()
                .find(|(name, _)| *name == runbook_name.as_str())
                .map(|(_, actions)| actions.clone());

            let Some(actions) = runbook else {
                tracing::warn!("No runbook for {}", runbook_name);
                continue;
            };

            let initial_state = belief_state_from_ascent(&prog);
            let plan = planner::plan(&initial_state, &config.goal_props, &actions);

            let Some(plan) = plan else {
                tracing::warn!("No plan found for {}", id);
                continue;
            };

            log.append(&Event {
                id: None, incident_id: id.clone(),
                event_type: EventType::PlanSelected { runbook: runbook_name.clone() },
                description: format!("Known pattern: {} ({} steps)", runbook_name, plan.steps.len()),
                details: None, timestamp: chrono::Utc::now(),
            }).unwrap();

            (id, plan.steps)

        } else if let Some(ref interpreter_agent) = interpreter {
            // 2b. INTERPRET — no Ascent match, ask the LLM (uncertain path)
            let beliefs = belief_state_from_ascent(&prog);

            // Only interpret if we have meaningful beliefs
            if beliefs.propositions.is_empty() {
                continue;
            }

            tracing::info!("No known pattern — invoking LLM interpreter");

            let interpretation = match llm::interpret(
                interpreter_agent, &beliefs, &recent_facts
            ).await {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!("LLM interpretation failed: {}", e);
                    continue;
                }
            };

            let incident_id = format!("llm:{}", chrono::Utc::now().timestamp());

            log.append(&Event {
                id: None, incident_id: incident_id.clone(),
                event_type: EventType::PatternMatched {
                    pattern: format!("LLM: {}", interpretation.hypothesis),
                },
                description: format!(
                    "LLM interpretation: {} (severity: {}, confidence via goal: {})",
                    interpretation.hypothesis, interpretation.severity, interpretation.goal
                ),
                details: Some(serde_json::to_value(&interpretation).unwrap()),
                timestamp: chrono::Utc::now(),
            }).unwrap();

            // 3b. PROPOSE — LLM suggests actions, pathfinding validates
            let plan_steps = if let Some(ref proposer_agent) = proposer {
                match llm::propose_and_validate(
                    proposer_agent,
                    &beliefs,
                    &interpretation.goal,
                    &config.all_actions,
                ).await {
                    Ok(Some(steps)) => steps,
                    Ok(None) => {
                        tracing::warn!("Neither LLM nor pathfinding could find a plan");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("LLM plan proposal failed: {}", e);
                        continue;
                    }
                }
            } else {
                // No proposer — try pathfinding with all actions directly
                let goal = vec![interpretation.goal.clone()];
                match planner::plan(&beliefs, &goal, &config.all_actions) {
                    Some(p) => p.steps,
                    None => { continue; }
                }
            };

            prog.already_handling.push((incident_id.clone(),));
            prog.run();

            log.append(&Event {
                id: None, incident_id: incident_id.clone(),
                event_type: EventType::PlanSelected { runbook: "llm_proposed".into() },
                description: format!("LLM-proposed plan: {} steps", plan_steps.len()),
                details: None, timestamp: chrono::Utc::now(),
            }).unwrap();

            (incident_id, plan_steps)
        } else {
            // No match AND no LLM configured — nothing to do
            continue;
        };

        // 4. EXECUTE with backtracking
        let mut attempts = 0;
        loop {
            match executor::execute_plan(&plan_steps, &incident_id, &log, &tool_executor).await {
                ExecutionResult::Resolved => {
                    log.append(&Event {
                        id: None, incident_id: incident_id.clone(),
                        event_type: EventType::Resolved,
                        description: "Incident resolved".into(),
                        details: None, timestamp: chrono::Utc::now(),
                    }).unwrap();
                    break;
                }
                ExecutionResult::FailedAtStep { step_index, error } => {
                    attempts += 1;
                    if attempts >= config.max_replan_attempts {
                        // 5. ESCALATE
                        let _ = escalation_tx.send(EscalationRequest {
                            incident_id: incident_id.clone(),
                            reason: format!("Exhausted {} attempts. Last: step {}: {}",
                                attempts, step_index, error),
                        }).await;
                        log.append(&Event {
                            id: None, incident_id: incident_id.clone(),
                            event_type: EventType::Escalated {
                                reason: format!("Exhausted {} attempts", attempts),
                            },
                            description: "Escalated to human".into(),
                            details: None, timestamp: chrono::Utc::now(),
                        }).unwrap();
                        break;
                    }
                    tracing::warn!("Failed step {}, attempt {}/{}", step_index, attempts, config.max_replan_attempts);
                    // Re-observe, replan on next loop iteration
                    // The failed action's error becomes a new fact context for the next attempt
                }
            }
        }
    }
}

fn insert_fact(prog: &mut AscentProgram, fact: &Fact) {
    match fact {
        Fact::Pod(p) => {
            prog.pod.push((p.name.clone(), p.namespace.clone(),
                format!("{:?}", p.phase), p.restart_count));
        }
        Fact::Alert(a) => {
            prog.alert.push((a.id.clone(), format!("{:?}", a.source),
                format!("{:?}", a.severity)));
        }
        Fact::Deploy(d) => {
            prog.deploy.push((d.name.clone(), d.namespace.clone(),
                d.revision.clone(), d.replicas, d.available));
        }
        Fact::Metric(m) => {
            tracing::debug!("Metric fact skipped in Datalog: {}", m.name);
            // f64 doesn't impl Eq/Hash — handle via threshold alerts instead
        }
    }
}

fn belief_state_from_ascent(prog: &AscentProgram) -> BeliefState {
    let mut props = std::collections::BTreeSet::new();
    for (name, _) in &prog.crashloop_detected { props.insert("crashloop_detected".into()); }
    for (name, _) in &prog.oomkill_detected { props.insert("oomkill_detected".into()); }
    for (name, _) in &prog.suspect_bad_deploy { props.insert(format!("suspect_bad_deploy:{}", name)); }
    for (name, _) in &prog.deploy_correlated_error { props.insert(format!("deploy_correlated_error:{}", name)); }
    for (svc,) in &prog.high_error_rate { props.insert(format!("high_error_rate:{}", svc)); }
    BeliefState { propositions: props }
}
```

**The two paths through the BDI loop:**

**Deterministic path** (Ascent match → runbook → pathfinding → execute):
- Fast, predictable, no LLM latency.
- Handles known incident patterns from institutional knowledge.
- This path works with `llm: None` in config.

**Uncertain path** (no Ascent match → LLM interpret → LLM propose → pathfinding validate → execute):
- Slower, uses LLM API calls.
- Handles novel incidents not covered by runbooks.
- Requires `llm: Some(config)` in agent config.
- The LLM proposes, but pathfinding validates. Bad proposals get caught.
- If the LLM fails entirely, the incident is simply not handled (waits for more facts or escalates).

---

## Crate 3: `agent-server`

Webhook listener + headless entry point.

```rust
// src/webhook.rs
use axum::{Router, Json, extract::State, http::StatusCode, routing::post};
use tokio::sync::mpsc;
use agent_core::facts::{Fact, AlertFact, AlertSource, Severity};

pub fn webhook_router(tx: mpsc::Sender<Fact>) -> Router {
    Router::new()
        .route("/webhook/datadog", post(handle_datadog))
        .route("/webhook/pagerduty", post(handle_pagerduty))
        .route("/webhook/generic", post(handle_generic))
        .with_state(tx)
}

async fn handle_generic(
    State(tx): State<mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    let alert = AlertFact {
        id: payload["id"].as_str().unwrap_or("unknown").to_string(),
        source: AlertSource::Generic,
        severity: Severity::High,
        title: payload["title"].as_str().unwrap_or("").to_string(),
        tags: vec![],
        received_at: chrono::Utc::now(),
    };
    let _ = tx.send(Fact::Alert(alert)).await;
    StatusCode::ACCEPTED
}

async fn handle_datadog(
    State(tx): State<mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    // Normalize Datadog webhook payload → AlertFact
    todo!()
}

async fn handle_pagerduty(
    State(tx): State<mpsc::Sender<Fact>>,
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    // Normalize PagerDuty webhook payload → AlertFact
    todo!()
}
```

```rust
// src/main.rs
use agent_core::{agent, event_log, streams, runbooks};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let log = event_log::EventLog::open("incidents.db").unwrap();
    let (webhook_tx, webhook_stream) = streams::webhook_channel(256);
    let (escalation_tx, mut escalation_rx) = tokio::sync::mpsc::channel(32);

    // Webhook server
    let router = agent_server::webhook::webhook_router(webhook_tx);
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
        tracing::info!("Webhook server on :8080");
        axum::serve(listener, router).await.unwrap();
    });

    // Escalation handler (stderr in headless mode)
    tokio::spawn(async move {
        while let Some(req) = escalation_rx.recv().await {
            eprintln!("⚠ ESCALATION: {} — {}", req.incident_id, req.reason);
        }
    });

    // BDI agent
    let config = agent::AgentConfig {
        max_replan_attempts: 3,
        runbooks: vec![
            ("crashloop_runbook", runbooks::crashloop_runbook()),
            ("oomkill_runbook", runbooks::oomkill_runbook()),
        ],
        goal_props: vec!["recovery_verified".into()],
    };

    agent::run_agent(
        webhook_stream,
        config,
        log,
        escalation_tx,
        |action| Box::pin(async move {
            // Mock executor — replace with real tool dispatch
            tracing::info!("Executing: {}", action.name);
            Ok(serde_json::json!({"status": "ok"}))
        }),
    ).await;
}
```

---

## Testing Strategy

### Unit Tests

**rig-effects:** Effect → Recovery derivation, cost_weight ordering, serde roundtrip, backtrackable classification.

**agent-core/rules.rs:** Insert pod facts → verify crashloop_detected derived. Insert deploy with available < replicas → verify suspect_bad_deploy. `already_handling` prevents re-fire. Lattice priority selects correct runbook.

**agent-core/planner.rs:** Given initial state and crashloop runbook → A* finds valid plan. Unreachable goal → returns None. Weighted cost prefers Observe over Mutate paths.

**agent-core/executor.rs:** Successful plan → all events logged, Resolved returned. Failure at step N → FailedAtStep, intent+result logged. Compensation stack built only for Mutate actions.

**agent-core/event_log.rs:** Append + query roundtrip. active_incidents excludes resolved. WAL mode enabled.

### Integration Tests

- Full cycle: webhook alert → fact insertion → Ascent derivation → plan → mock execute → resolve
- Backtracking: execute → fail at step 3 → compensate steps 2,1 → replan → succeed
- Escalation: 3 failures → escalation sent to channel
- Recovery: replay event log → correct state reconstruction

### Property Tests

- Any sequence of fact insertions produces deterministic Ascent derivation (run twice, compare)
- Plan from A* has monotonically satisfied preconditions
- Compensation of all Mutate steps in reverse returns to initial state propositions

---

## Build & Run

```bash
mkdir incident-agent-workspace && cd incident-agent-workspace
cargo init --name incident-agent-workspace

cargo new rig-effects --lib
cargo new rig-effects-derive --lib
cargo new agent-core --lib
cargo new agent-server

cargo test --workspace
cargo run -p agent-server

# Test
curl -X POST http://localhost:8080/webhook/generic \
  -H 'Content-Type: application/json' \
  -d '{"id": "test-1", "title": "Pod crashlooping", "severity": "high"}'
```

## Implementation Order

1. **`rig-effects`** — types only. 30 minutes.
2. **`agent-core/facts.rs`** — domain types.
3. **`agent-core/rules.rs`** — Ascent rules. Test with mock facts.
4. **`agent-core/runbooks.rs`** — action schemas.
5. **`agent-core/planner.rs`** — wire into pathfinding.
6. **`agent-core/event_log.rs`** — SQLite.
7. **`agent-core/executor.rs`** — WAL execution.
8. **`agent-core/tools.rs`** — dual rig::Tool + Effectful impls with mock backends.
9. **`agent-core/agent.rs`** — BDI loop, deterministic path only (`llm: None`). Full end-to-end test here.
10. **`agent-core/llm.rs`** — Rig agent builders. Test interpreter, analyzer, proposer individually.
11. **`agent-core/agent.rs`** — enable uncertain path (`llm: Some(config)`). Integration test with mock LLM.
12. **`agent-core/streams.rs`** — real infrastructure streams.
13. **`agent-core/tools.rs`** — replace mock backends with real kube-rs/reqwest calls.
14. **`agent-server`** — webhook + headless main.
15. **Tauri + Leptos UI** — optional, last.

Note: steps 1-9 produce a working agent that handles known patterns without any LLM dependency. Steps 10-11 add the uncertain path. Steps 12-13 connect to real infrastructure. The deterministic path is always validated before adding LLM capability.

## Key Principles

- **Compose, don't build.** Ascent for Datalog. pathfinding for search. tokio streams for observation. rusqlite for persistence. mpsc for escalation. Rig for LLM. Only rig-effects is custom.
- **Thinnest crossing points.** `Stream::poll_next`. Three closures for planning. Insert/run/read for Datalog. Send/recv for escalation. `rig::agent::Agent::prompt()` for LLM. No custom framework traits beyond Compensable.
- **Two paths through the loop.** Deterministic (Ascent → runbook → pathfinding) and uncertain (LLM interpret → propose → pathfinding validate). The deterministic path works without Rig. The uncertain path uses Rig but pathfinding always validates.
- **Tools are dual-implemented.** Every infrastructure action implements both `rig::tool::Tool` (LLM can discover and call them) and `Effectful` (executor tracks their effects). Observe tools are safe for LLM autonomy. Mutate tools require effect-tracked execution.
- **LLM proposes, planner validates.** The LLM never executes mutating actions directly. It proposes action sequences that pathfinding validates against preconditions. Bad proposals are caught before execution.
- **Analyzer agent has tool autonomy.** The one exception: during root cause analysis, the LLM can autonomously call Observe tools to gather evidence. This is safe because Observe effects are idempotent.
- **Event log is source of truth.** Ascent state is ephemeral — rebuilt from facts. Facts are logged.
- **Agent works headless first.** No UI dependency.
- **Mock everything initially.** Get the deterministic BDI loop right before adding LLM. Get the LLM path right before connecting real infrastructure.
- **One runbook end-to-end first.** Crashloop detection → plan → execute → verify. Then add more patterns. Then add LLM interpretation.
