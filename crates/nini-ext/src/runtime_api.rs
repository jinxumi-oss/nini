//! RuntimeApi — the concrete side of the `ExtensionAPI` trait. nini
//! instantiates one of these and hands `&mut` references to each
//! extension during `activate`. The runtime can then observe what
//! commands / tools were registered and replay those registrations into
//! the live command dispatcher and tool registry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    CommandHandler, CommandSpec, ExtensionAPI, Tool,
};

/// Records the side-effects extensions want to take. Held inside an
/// `Arc<Mutex<…>>` so the extension code can borrow it through the
/// `ExtensionAPI` impl, while the runtime observes the final state after
/// `activate` returns.
#[derive(Default)]
pub struct RegisteredState {
    pub commands: HashMap<String, (CommandSpec, CommandHandler)>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub queued_user_messages: Vec<String>,
    pub status_text: Option<String>,
}

/// Concrete `ExtensionAPI` implementation backed by `Arc<Mutex<…>>` so
/// the extension can be called from any thread. The runtime queries the
/// `RegisteredState` after `activate` returns to integrate registrations
/// into the live TUI.
pub struct RuntimeApi {
    state: Arc<Mutex<RegisteredState>>,
    cwd: PathBuf,
    model: String,
}

impl RuntimeApi {
    /// Construct a fresh API backed by the given cwd + active model.
    pub fn new(cwd: PathBuf, model: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(RegisteredState::default())),
            cwd,
            model,
        }
    }

    /// Hand out an `Arc<Mutex<RegisteredState>>` to the runtime so it
    /// can pull the registrations after `activate`.
    pub fn shared_state(&self) -> Arc<Mutex<RegisteredState>> {
        self.state.clone()
    }
}

impl ExtensionAPI for RuntimeApi {
    fn register_command(&mut self, name: &str, spec: CommandSpec, handler: CommandHandler) {
        let mut s = self.state.lock().expect("RuntimeApi state poisoned");
        s.commands.insert(name.to_string(), (spec, handler));
    }

    fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        let mut s = self.state.lock().expect("RuntimeApi state poisoned");
        s.tools.push(tool);
    }

    fn send_user_message(&self, text: &str) {
        let mut s = self.state.lock().expect("RuntimeApi state poisoned");
        s.queued_user_messages.push(text.to_string());
    }

    fn get_active_model(&self) -> String {
        self.model.clone()
    }

    fn get_current_cwd(&self) -> PathBuf {
        self.cwd.clone()
    }

    fn set_status(&self, text: &str) {
        let mut s = self.state.lock().expect("RuntimeApi state poisoned");
        s.status_text = Some(text.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandSpec, ExtensionAPI};
    use nini_core::tool::{Tool, ToolContext, ToolOutput};

    fn sample_spec() -> CommandSpec {
        CommandSpec {
            description: "sample command".to_string(),
            argument_hint: None,
            hidden: false,
        }
    }

    #[test]
    fn register_command_stores_in_state() {
        let mut api = RuntimeApi::new(PathBuf::from("/tmp"), "test-model".to_string());
        api.register_command(
            "foo",
            sample_spec(),
            Box::new(|_ctx| {
                // No-op handler for the test.
            }),
        );
        let s = api.shared_state();
        let guard = s.lock().unwrap();
        assert!(guard.commands.contains_key("foo"));
    }

    #[test]
    fn send_user_message_queues() {
        let api = RuntimeApi::new(PathBuf::from("/tmp"), "m".to_string());
        api.send_user_message("hello");
        api.send_user_message("world");
        let s = api.shared_state();
        let guard = s.lock().unwrap();
        assert_eq!(guard.queued_user_messages, vec!["hello", "world"]);
    }

    #[test]
    fn set_status_overwrites() {
        let api = RuntimeApi::new(PathBuf::from("/tmp"), "m".to_string());
        api.set_status("first");
        api.set_status("second");
        assert_eq!(
            api.shared_state().lock().unwrap().status_text.as_deref(),
            Some("second")
        );
    }

    #[test]
    fn get_active_model_returns_snapshot() {
        let api = RuntimeApi::new(PathBuf::from("/tmp"), "anthropic/claude".to_string());
        assert_eq!(api.get_active_model(), "anthropic/claude");
    }

    #[test]
    fn get_current_cwd_returns_snapshot() {
        let api = RuntimeApi::new(PathBuf::from("/home/nini"), "m".to_string());
        assert_eq!(api.get_current_cwd(), PathBuf::from("/home/nini"));
    }

    struct DummyTool;
    #[async_trait::async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &'static str { "dummy" }
        fn spec(&self) -> nini_core::tool::ToolSpec {
            nini_core::tool::ToolSpec {
                name: "dummy".to_string(),
                description: "dummy tool for tests".to_string(),
                input_schema: serde_json::json!({}),
            }
        }
        async fn execute(&self, _input: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, nini_core::tool::ToolError> {
            Ok(ToolOutput { content: String::new(), is_error: false, details: None })
        }
    }

    #[test]
    fn register_tool_appends_to_state() {
        let mut api = RuntimeApi::new(PathBuf::from("/tmp"), "m".to_string());
        let tool: Arc<dyn Tool> = Arc::new(DummyTool);
        api.register_tool(tool);
        let s = api.shared_state();
        let guard = s.lock().unwrap();
        assert_eq!(guard.tools.len(), 1);
    }
}
