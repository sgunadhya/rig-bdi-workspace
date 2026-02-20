use agent_core::{agent, event_log, runbooks, streams};

#[tokio::main]
async fn main() {
    let log = event_log::EventLog::open("incidents.db").expect("open event log");
    let (webhook_tx, webhook_stream) = streams::webhook_channel(256);
    let (escalation_tx, escalation_rx) = std::sync::mpsc::channel();

    let config = agent::AgentConfig {
        max_replan_attempts: 3,
        runbooks: vec![
            ("crashloop_runbook", runbooks::crashloop_runbook()),
            ("oomkill_runbook", runbooks::oomkill_runbook()),
        ],
        all_actions: vec![
            runbooks::crashloop_runbook(),
            runbooks::oomkill_runbook(),
        ]
        .into_iter()
        .flatten()
        .collect(),
        goal_props: vec!["recovery_verified".into()],
        llm: build_llm_config_from_env(),
    };

    let log_for_agent = log.clone();
    std::thread::spawn(move || {
        agent::run_agent(webhook_stream, config, log_for_agent, escalation_tx, |_action| {
            Ok(serde_json::json!({"status": "ok"}))
        });
    });

    std::thread::spawn(move || {
        while let Ok(req) = escalation_rx.recv() {
            eprintln!("ESCALATION {}: {}", req.incident_id, req.reason);
        }
    });

    let app = agent_server::webhook::webhook_router(webhook_tx);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("bind :8080");

    println!("agent-server listening on :8080");
    axum::serve(listener, app).await.expect("serve");
}

fn build_llm_config_from_env() -> Option<agent_core::llm::LlmConfig> {
    let api_key_env = std::env::var("LLM_API_KEY_ENV").unwrap_or_else(|_| "OPENAI_API_KEY".into());
    if std::env::var(&api_key_env).is_err() {
        return None;
    }

    Some(agent_core::llm::LlmConfig {
        provider: std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".into()),
        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
        api_key_env,
        temperature: std::env::var("LLM_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.2),
    })
}
