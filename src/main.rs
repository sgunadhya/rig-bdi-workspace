mod app;
mod bridge;
mod dto;

pub mod components {
    pub mod belief_viewer;
    pub mod dashboard;
    pub mod escalation_panel;
    pub mod incident_list;
    pub mod metrics_panel;
    pub mod plan_viewer;
    pub mod timeline;
}

fn main() {
    // TODO: mount Leptos app.
    app::app();
}
