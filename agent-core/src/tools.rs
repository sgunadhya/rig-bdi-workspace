use crate::runbooks::ActionSchema;

#[derive(Default, Clone)]
pub struct ToolRegistry;

impl ToolRegistry {
    pub fn execute(&self, action: ActionSchema) -> Result<serde_json::Value, String> {
        if action.name.to_lowercase().contains("fail") {
            Err(format!("simulated failure for action {}", action.name))
        } else {
            Ok(serde_json::json!({
                "status": "ok",
                "action": action.name,
            }))
        }
    }
}
