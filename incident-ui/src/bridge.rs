use crate::dto::{
    EscalationResponse, FactDto, IncidentDto, PlanDto, SuggestedFactDto, TimelineEventDto,
    ToolCallDto,
};
use js_sys::{Function, Promise, Reflect};
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

fn invoke_fn() -> Result<(JsValue, Function), String> {
    let window = web_sys::window().ok_or_else(|| "window not available".to_string())?;
    let tauri = Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|_| "failed to access __TAURI__".to_string())?;
    if tauri.is_undefined() || tauri.is_null() {
        return Err("Tauri bridge unavailable".into());
    }

    let direct = Reflect::get(&tauri, &JsValue::from_str("invoke")).ok();
    if let Some(v) = direct {
        if v.is_function() {
            return Ok((tauri, v.unchecked_into::<Function>()));
        }
    }

    let tauri_ns = Reflect::get(&tauri, &JsValue::from_str("tauri")).ok();
    if let Some(ns) = tauri_ns {
        let ns_invoke = Reflect::get(&ns, &JsValue::from_str("invoke")).ok();
        if let Some(v) = ns_invoke {
            if v.is_function() {
                return Ok((ns, v.unchecked_into::<Function>()));
            }
        }
    }

    let core = Reflect::get(&tauri, &JsValue::from_str("core"))
        .map_err(|_| "failed to access __TAURI__.core".to_string())?;
    let core_invoke = Reflect::get(&core, &JsValue::from_str("invoke"))
        .map_err(|_| "failed to access __TAURI__.core.invoke".to_string())?;
    if core_invoke.is_function() {
        return Ok((core, core_invoke.unchecked_into::<Function>()));
    }

    Err("no invoke function available".into())
}

pub async fn call<A, R>(cmd: &str, args: &A) -> Result<R, String>
where
    A: Serialize,
    R: DeserializeOwned,
{
    let (this_obj, invoke) = invoke_fn()?;
    let args = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| e.to_string())?;
    let js = invoke
        .call2(&this_obj, &JsValue::from_str(cmd), &args)
        .map_err(|e| format!("invoke failed: {e:?}"))?;
    let val = JsFuture::from(Promise::from(js))
        .await
        .map_err(|e| format!("invoke rejected: {e:?}"))?;
    serde_wasm_bindgen::from_value(val).map_err(|e| e.to_string())
}

pub async fn fetch_incidents() -> Result<Vec<IncidentDto>, String> {
    call("list_incidents_cmd", &()).await
}

pub async fn fetch_timeline(id: &str) -> Result<Vec<TimelineEventDto>, String> {
    call("get_timeline_cmd", &serde_json::json!({ "incidentId": id })).await
}

pub async fn fetch_beliefs(id: &str) -> Result<Vec<FactDto>, String> {
    call("get_beliefs_cmd", &serde_json::json!({ "incidentId": id })).await
}

pub async fn fetch_plan(id: &str) -> Result<PlanDto, String> {
    call("get_current_plan_cmd", &serde_json::json!({ "incidentId": id })).await
}

pub async fn fetch_tool_calls(id: &str) -> Result<Vec<ToolCallDto>, String> {
    call("get_tool_calls_cmd", &serde_json::json!({ "incidentId": id })).await
}

pub async fn submit_escalation(id: &str, response: EscalationResponse) -> Result<(), String> {
    call(
        "respond_to_escalation_cmd",
        &serde_json::json!({
            "incidentId": id,
            "response": response
        }),
    )
    .await
}

pub async fn upsert_alert_fact(
    incident_id: &str,
    fact_id: &str,
    title: &str,
    severity: &str,
    tags: &[String],
) -> Result<(), String> {
    call(
        "upsert_alert_fact_cmd",
        &serde_json::json!({
            "incidentId": incident_id,
            "factId": fact_id,
            "title": title,
            "severity": severity,
            "tags": tags
        }),
    )
    .await
}

pub async fn retract_fact(incident_id: &str, fact_id: &str) -> Result<(), String> {
    call(
        "retract_fact_cmd",
        &serde_json::json!({
            "incidentId": incident_id,
            "factId": fact_id
        }),
    )
    .await
}

pub async fn reprocess_incident(incident_id: &str) -> Result<(), String> {
    call(
        "reprocess_incident_cmd",
        &serde_json::json!({
            "incidentId": incident_id
        }),
    )
    .await
}

pub async fn fetch_suggested_facts(id: &str) -> Result<Vec<SuggestedFactDto>, String> {
    call("get_suggested_facts_cmd", &serde_json::json!({ "incidentId": id })).await
}

pub async fn generate_fact_suggestions(id: &str) -> Result<(), String> {
    call(
        "generate_fact_suggestions_cmd",
        &serde_json::json!({ "incidentId": id }),
    )
    .await
}

pub async fn decide_fact_suggestion(
    id: &str,
    suggestion_event_id: i64,
    decision: &str,
) -> Result<(), String> {
    call(
        "decide_fact_suggestion_cmd",
        &serde_json::json!({
            "incidentId": id,
            "suggestionEventId": suggestion_event_id,
            "decision": decision
        }),
    )
    .await
}
