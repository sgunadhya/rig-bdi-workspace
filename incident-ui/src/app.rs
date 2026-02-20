use crate::bridge;
use crate::dto::{
    EscalationResponse, FactDto, IncidentDto, PlanDto, PlanStepDto, TimelineEventDto, ToolCallDto,
};
use leptos::*;
use wasm_bindgen_futures::spawn_local;

fn badge_class(effect: &str) -> &'static str {
    let lower = effect.to_ascii_lowercase();
    if lower.contains("mutate") {
        "mutate"
    } else if lower.contains("irreversible") {
        "irreversible"
    } else if lower.contains("pure") {
        "pure"
    } else {
        "observe"
    }
}

#[component]
pub fn App() -> impl IntoView {
    let incidents = create_rw_signal(Vec::<IncidentDto>::new());
    let selected = create_rw_signal(None::<String>);
    let timeline = create_rw_signal(Vec::<TimelineEventDto>::new());
    let beliefs = create_rw_signal(Vec::<FactDto>::new());
    let plan = create_rw_signal(PlanDto {
        steps: Vec::<PlanStepDto>::new(),
        current_step: 0,
    });
    let tools = create_rw_signal(Vec::<ToolCallDto>::new());
    let error = create_rw_signal(None::<String>);

    let fact_id = create_rw_signal(String::new());
    let fact_title = create_rw_signal(String::new());
    let fact_severity = create_rw_signal("high".to_string());
    let fact_tags = create_rw_signal(String::new());
    let reject_reason = create_rw_signal(String::new());

    let load_incidents = move || {
        spawn_local(async move {
            match bridge::fetch_incidents().await {
                Ok(list) => {
                    let first = list.first().map(|i| i.id.clone());
                    incidents.set(list);
                    if selected.get_untracked().is_none() {
                        selected.set(first);
                    }
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let load_details = move |incident_id: String| {
        spawn_local(async move {
            let tl = bridge::fetch_timeline(&incident_id).await;
            let bl = bridge::fetch_beliefs(&incident_id).await;
            let pl = bridge::fetch_plan(&incident_id).await;
            let tc = bridge::fetch_tool_calls(&incident_id).await;
            let mut errs = Vec::new();

            match tl {
                Ok(v) => timeline.set(v),
                Err(e) => errs.push(format!("timeline: {e}")),
            }
            match bl {
                Ok(v) => {
                    beliefs.set(v.clone());
                    if let Some(first) = v.first() {
                        fact_id.set(first.fact_id.clone());
                        fact_title.set(first.summary.clone());
                        fact_severity.set(first.severity.clone());
                        fact_tags.set(first.tags.join(","));
                    } else {
                        fact_id.set(incident_id.clone());
                        fact_title.set(String::new());
                        fact_severity.set("high".to_string());
                        fact_tags.set(String::new());
                    }
                }
                Err(e) => errs.push(format!("beliefs: {e}")),
            }
            match pl {
                Ok(v) => plan.set(v),
                Err(e) => errs.push(format!("plan: {e}")),
            }
            match tc {
                Ok(v) => tools.set(v),
                Err(e) => errs.push(format!("tool_calls: {e}")),
            }

            if errs.is_empty() {
                error.set(None);
            } else {
                error.set(Some(format!(
                    "Failed to load incident details for {incident_id}\n{}",
                    errs.join("\n")
                )));
            }
        });
    };

    create_effect(move |_| {
        if let Some(id) = selected.get() {
            load_details(id);
        }
    });

    load_incidents();

    let save_fact = move || {
        if let Some(id) = selected.get_untracked() {
            let fid = {
                let v = fact_id.get_untracked().trim().to_string();
                if v.is_empty() { id.clone() } else { v }
            };
            let title = fact_title.get_untracked().trim().to_string();
            let severity = fact_severity.get_untracked().trim().to_string();
            let tags: Vec<String> = fact_tags
                .get_untracked()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            spawn_local(async move {
                let _ = bridge::upsert_alert_fact(&id, &fid, &title, &severity, &tags).await;
                load_incidents();
                load_details(id);
            });
        }
    };

    let retract = move || {
        if let Some(id) = selected.get_untracked() {
            let fid = fact_id.get_untracked().trim().to_string();
            if fid.is_empty() {
                return;
            }
            spawn_local(async move {
                let _ = bridge::retract_fact(&id, &fid).await;
                load_incidents();
                load_details(id);
            });
        }
    };

    let reprocess = move || {
        if let Some(id) = selected.get_untracked() {
            spawn_local(async move {
                let _ = bridge::reprocess_incident(&id).await;
                load_incidents();
                load_details(id);
            });
        }
    };

    let escalation = move |response: EscalationResponse| {
        if let Some(id) = selected.get_untracked() {
            spawn_local(async move {
                let _ = bridge::submit_escalation(&id, response).await;
                load_incidents();
                load_details(id);
            });
        }
    };

    view! {
      <div class="layout">
        <section class="panel">
          <h2>"Incidents"</h2>
          <button on:click=move |_| load_incidents()>"Refresh"</button>
          <ul>
            <For
              each=move || incidents.get()
              key=|i| i.id.clone()
              children=move |i| {
                let id = i.id.clone();
                view! {
                  <li on:click=move |_| selected.set(Some(id.clone()))>
                    <div><b>{i.id.clone()}</b> <span class="meta">{format!("({})", i.status)}</span></div>
                    <div>{i.title.clone()}</div>
                    <div class="meta">{format!("sev={} phase={}", i.severity, i.current_phase)}</div>
                  </li>
                }
              }
            />
          </ul>
        </section>

        <section class="panel">
          <h2>"Timeline"</h2>
          <ul>
            <For
              each=move || timeline.get()
              key=|e| e.id
              children=move |e| view! {
                <li>
                  <div><b>{e.event_type}</b></div>
                  <div>{e.description}</div>
                  <div class="meta">{e.timestamp}</div>
                </li>
              }
            />
          </ul>
        </section>

        <section class="panel">
          <h2>"Beliefs / Plan / Escalation"</h2>

          <h3>"Beliefs"</h3>
          <ul>
            <For
              each=move || beliefs.get()
              key=|f| f.fact_id.clone()
              children=move |f| view! {
                <li>
                  <div>{format!("{}({}): {}", f.fact_type, f.fact_id, f.summary)}</div>
                  <div class="meta">{format!("sev={} tags={} at {}", f.severity, f.tags.join(","), f.timestamp)}</div>
                </li>
              }
            />
          </ul>

          <h3>"Fact Editor"</h3>
          <div class="stack">
            <input
              prop:value=move || fact_id.get()
              on:input=move |ev| fact_id.set(event_target_value(&ev))
              placeholder="Fact ID"
            />
            <input
              prop:value=move || fact_title.get()
              on:input=move |ev| fact_title.set(event_target_value(&ev))
              placeholder="Fact title"
            />
            <input
              prop:value=move || fact_severity.get()
              on:input=move |ev| fact_severity.set(event_target_value(&ev))
              placeholder="Severity (low|medium|high|critical)"
            />
            <input
              prop:value=move || fact_tags.get()
              on:input=move |ev| fact_tags.set(event_target_value(&ev))
              placeholder="Tags (comma separated)"
            />
            <div class="row">
              <button on:click=move |_| save_fact()>"Save Fact"</button>
              <button on:click=move |_| retract()>"Retract Fact"</button>
            </div>
          </div>

          <h3>"Plan"</h3>
          <div class="row">
            <button on:click=move |_| reprocess()>"Re-run Pipeline"</button>
          </div>
          <ul>
            <For
              each=move || plan.get().steps
              key=|s| s.name.clone()
              children=move |s| {
                let class = if s.status == "done" {
                  "ok"
                } else if s.status == "failed" {
                  "warn"
                } else {
                  ""
                };
                view! {
                  <li class="step">
                    <span>{s.name.clone()}</span>
                    <span><span class=format!("badge {}", badge_class(&s.effect))>{s.effect}</span> " " <b class=class>{s.status}</b></span>
                  </li>
                }
              }
            />
          </ul>

          <h3>"Tool Usage"</h3>
          <ul>
            <For
              each=move || tools.get()
              key=|t| t.event_id
              children=move |t| {
                let class = if t.status == "done" {
                  "ok"
                } else if t.status == "failed" {
                  "warn"
                } else {
                  ""
                };
                view! {
                  <li>
                    <div><b>{t.tool_name.clone()}</b> <span class="meta">{format!("({})", t.phase)}</span></div>
                    <div><span class=format!("badge {}", badge_class(&t.effect))>{t.effect}</span> " " <b class=class>{t.status}</b></div>
                    <div class="meta">{t.summary}</div>
                  </li>
                }
              }
            />
          </ul>

          <h3>"Escalation Response"</h3>
          <div class="row">
            <button on:click=move |_| escalation(EscalationResponse::Approve)>"Approve"</button>
            <button on:click=move |_| escalation(EscalationResponse::TakeOver)>"Take Over"</button>
          </div>
          <div class="row">
            <input
              prop:value=move || reject_reason.get()
              on:input=move |ev| reject_reason.set(event_target_value(&ev))
              placeholder="Reject reason"
            />
            <button on:click=move |_| {
              let reason = {
                let s = reject_reason.get_untracked();
                if s.trim().is_empty() { "No reason provided".to_string() } else { s }
              };
              escalation(EscalationResponse::Reject { reason });
            }>"Reject"</button>
          </div>

          <Show
            when=move || error.get().is_some()
            fallback=|| ()
          >
            <pre class="error">{move || error.get().unwrap_or_default()}</pre>
          </Show>
        </section>
      </div>
    }
}
