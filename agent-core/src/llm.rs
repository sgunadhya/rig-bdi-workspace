use crate::facts::Fact;
use crate::runbooks::ActionSchema;
use futures::executor::block_on;
use rig::client::{completion::CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai;
use serde::{Deserialize, Serialize};
use std::future::IntoFuture;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interpretation {
    pub hypothesis: String,
    pub goal: String,
    pub candidate_actions: Vec<String>,
}

pub fn interpret(
    config: &LlmConfig,
    recent_facts: &[Fact],
) -> Result<Interpretation, String> {
    let prompt = format!(
        "Analyze the incident context and return JSON only.\n\
         Schema: {{\"hypothesis\":\"string\",\"goal\":\"string\",\"candidate_actions\":[\"string\"]}}\n\
         Facts:\n{}",
        serde_json::to_string_pretty(recent_facts).map_err(|e| e.to_string())?
    );

    let raw = run_prompt(config, "You are an incident interpreter.", &prompt)?;
    parse_interpretation(&raw)
}

pub fn propose_and_validate(
    config: &LlmConfig,
    hypothesis: &str,
    goal: &str,
    candidate_actions: &[String],
    all_actions: &[ActionSchema],
) -> Result<Vec<ActionSchema>, String> {
    let prompt = format!(
        "Return JSON only.\n\
         Schema: {{\"actions\":[\"string\"]}}\n\
         hypothesis={hypothesis}\n\
         goal={goal}\n\
         candidate_actions={}\n\
         available_actions={}",
        serde_json::to_string(candidate_actions).map_err(|e| e.to_string())?,
        serde_json::to_string(
            &all_actions
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
        )
        .map_err(|e| e.to_string())?
    );

    let raw = run_prompt(config, "You are an incident planner.", &prompt)?;
    let names = parse_action_list(&raw)?;

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

fn run_prompt(config: &LlmConfig, preamble: &str, prompt: &str) -> Result<String, String> {
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

    let agent = client
        .agent(&config.model)
        .preamble(preamble)
        .temperature(config.temperature)
        .build();

    let fut = agent.prompt(prompt).into_future();
    let out: Result<String, _> = block_on(fut);
    out.map_err(|e| format!("llm prompt failed: {e}"))
}

fn parse_interpretation(raw: &str) -> Result<Interpretation, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid llm interpretation json: {e}"))?;
    Ok(Interpretation {
        hypothesis: v
            .get("hypothesis")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        goal: v
            .get("goal")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("recovery_verified")
            .to_string(),
        candidate_actions: v
            .get("candidate_actions")
            .and_then(serde_json::Value::as_array)
            .map(|xs| {
                xs.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    })
}

fn parse_action_list(raw: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid llm action-list json: {e}"))?;
    Ok(v.get("actions")
        .and_then(serde_json::Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interpretation_json() {
        let raw = r#"{
          "hypothesis":"memory pressure",
          "goal":"recovery_verified",
          "candidate_actions":["inspect-memory-metrics","tune-memory-limits"]
        }"#;
        let parsed = parse_interpretation(raw).expect("parse");
        assert_eq!(parsed.goal, "recovery_verified");
        assert_eq!(parsed.candidate_actions.len(), 2);
    }

    #[test]
    fn parse_action_list_json() {
        let raw = r#"{"actions":["inspect-pod-logs","rollback-deployment"]}"#;
        let parsed = parse_action_list(raw).expect("parse");
        assert_eq!(parsed.len(), 2);
    }
}
