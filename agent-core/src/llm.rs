use crate::facts::Fact;
use crate::runbooks::ActionSchema;
use futures::executor::block_on;
use rig::client::ProviderClient;
use rig::extractor::ExtractorBuilder;
use rig::providers::openai;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key_env: String,
    pub temperature: f64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            temperature: 0.2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Interpretation {
    pub hypothesis: String,
    pub goal: String,
    pub candidate_actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SuggestedFact {
    pub fact_id: String,
    pub title: String,
    pub severity: String,
    pub tags: Vec<String>,
    pub rationale: String,
}

pub fn interpret(
    config: &LlmConfig,
    recent_facts: &[Fact],
) -> Result<Interpretation, String> {
    let facts_json = serde_json::to_string_pretty(recent_facts).map_err(|e| e.to_string())?;
    let extracted: Interpretation = run_extract(
        config,
        "You are an incident interpreter. Infer likely incident hypothesis, goal, and candidate actions from facts.",
        "Analyze the incident facts and return a structured interpretation.",
        &[("facts_json", facts_json)],
    )?;

    Ok(Interpretation {
        hypothesis: if extracted.hypothesis.trim().is_empty() {
            "unknown".into()
        } else {
            extracted.hypothesis
        },
        goal: if extracted.goal.trim().is_empty() {
            "recovery_verified".into()
        } else {
            extracted.goal
        },
        candidate_actions: extracted.candidate_actions,
    })
}

pub fn propose_and_validate(
    config: &LlmConfig,
    hypothesis: &str,
    goal: &str,
    candidate_actions: &[String],
    all_actions: &[ActionSchema],
) -> Result<Vec<ActionSchema>, String> {
    let names = run_extract::<ProposedActions>(
        config,
        "You are an incident planner. Select executable action names from available actions.",
        "Return structured action names ordered by execution priority.",
        &[
            ("hypothesis", hypothesis.to_string()),
            ("goal", goal.to_string()),
            (
                "candidate_actions",
                serde_json::to_string(candidate_actions).map_err(|e| e.to_string())?,
            ),
            (
                "available_actions",
                serde_json::to_string(
                    &all_actions
                        .iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>(),
                )
                .map_err(|e| e.to_string())?,
            ),
        ],
    )?
    .actions;

    // LLM proposes, executor validates by intersection with known actions.
    let mut selected = Vec::new();
    for name in names {
        if let Some(action) = all_actions.iter().find(|a| a.name == name) {
            selected.push(action.clone());
        }
    }
    if selected.is_empty() {
        for action in all_actions {
            if candidate_actions.contains(&action.name) {
                selected.push(action.clone());
            }
        }
    }

    Ok(selected)
}

pub fn suggest_facts(config: &LlmConfig, recent_facts: &[Fact]) -> Result<Vec<SuggestedFact>, String> {
    let facts_json = serde_json::to_string_pretty(recent_facts).map_err(|e| e.to_string())?;
    let out: SuggestedFactsEnvelope = run_extract(
        config,
        "You are an SRE incident assistant. Propose additional facts that increase confidence.",
        "Suggest up to three additional alert-like facts that are useful next observations.",
        &[("facts_json", facts_json)],
    )?;
    Ok(out.suggestions)
}

fn run_extract<T>(
    config: &LlmConfig,
    preamble: &str,
    prompt: &str,
    context: &[(&str, String)],
) -> Result<T, String>
where
    T: JsonSchema + for<'a> Deserialize<'a> + Serialize + Send + Sync + 'static,
{
    if config.provider.to_lowercase() != "openai" {
        return Err(format!("unsupported llm provider '{}'", config.provider));
    }

    let client = if config.api_key_env == "OPENAI_API_KEY" {
        openai::Client::from_env()
    } else {
        let api_key = std::env::var(&config.api_key_env)
            .map_err(|_| format!("missing env var {}", config.api_key_env))?;
        openai::Client::new(&api_key).map_err(|e| format!("openai client error: {e}"))?
    };

    let mut builder: ExtractorBuilder<_, T> = client
        .extractor::<T>(&config.model)
        .preamble(preamble)
        .additional_params(serde_json::json!({ "temperature": config.temperature }));

    for (name, value) in context {
        builder = builder.context(&format!("{name}: {value}"));
    }

    let extractor = builder.build();
    let out = block_on(extractor.extract(prompt))
        .map_err(|e| format!("llm extractor failed: {e}"))?;
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
struct ProposedActions {
    actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
struct SuggestedFactsEnvelope {
    suggestions: Vec<SuggestedFact>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpretation_struct_roundtrip() {
        let value = Interpretation {
            hypothesis: "memory pressure".into(),
            goal: "recovery_verified".into(),
            candidate_actions: vec!["inspect-memory-metrics".into(), "tune-memory-limits".into()],
        };
        let json = serde_json::to_string(&value).expect("json");
        let back: Interpretation = serde_json::from_str(&json).expect("from json");
        assert_eq!(back.goal, "recovery_verified");
        assert_eq!(back.candidate_actions.len(), 2);
    }

    #[test]
    fn proposed_actions_struct_roundtrip() {
        let value = ProposedActions {
            actions: vec!["inspect-pod-logs".into(), "rollback-deployment".into()],
        };
        let json = serde_json::to_string(&value).expect("json");
        let back: ProposedActions = serde_json::from_str(&json).expect("from json");
        assert_eq!(back.actions.len(), 2);
    }

    #[test]
    fn suggested_facts_struct_roundtrip() {
        let value = SuggestedFactsEnvelope {
            suggestions: vec![SuggestedFact {
                fact_id: "f1".into(),
                title: "pod restarted".into(),
                severity: "high".into(),
                tags: vec!["k8s".into()],
                rationale: "recent restart spike".into(),
            }],
        };
        let json = serde_json::to_string(&value).expect("json");
        let back: SuggestedFactsEnvelope = serde_json::from_str(&json).expect("from json");
        assert_eq!(back.suggestions.len(), 1);
        assert_eq!(back.suggestions[0].fact_id, "f1");
    }
}
