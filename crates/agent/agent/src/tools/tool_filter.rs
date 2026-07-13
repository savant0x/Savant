//! Tool Filter — per-profile tool restrictions for sub-agents.
//!
//! When a sub-agent is spawned with a profile, only the tools listed in the
//! profile's `allowed_tools` are exposed to the LLM. If the list is empty,
//! all tools are available.

use savant_core::traits::Tool;
use std::collections::HashSet;
use std::sync::Arc;

/// Filters tools based on a profile's allowed tool list.
pub struct ToolFilter {
    allowed: HashSet<String>,
    all_tools: Vec<Arc<dyn Tool>>,
}

impl ToolFilter {
    /// Create a new tool filter.
    /// If `allowed` is empty, all tools pass through.
    pub fn new(allowed: Vec<String>, all_tools: Vec<Arc<dyn Tool>>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
            all_tools,
        }
    }

    /// Get the tools available to this sub-agent.
    pub fn available_tools(&self) -> Vec<Arc<dyn Tool>> {
        if self.allowed.is_empty() {
            return self.all_tools.clone();
        }
        self.all_tools
            .iter()
            .filter(|t| self.allowed.contains(t.name()))
            .cloned()
            .collect()
    }

    /// Check if a specific tool is available.
    pub fn is_tool_available(&self, tool_name: &str) -> bool {
        self.allowed.is_empty() || self.allowed.contains(tool_name)
    }

    /// Get the number of available tools.
    pub fn available_count(&self) -> usize {
        if self.allowed.is_empty() {
            self.all_tools.len()
        } else {
            self.all_tools
                .iter()
                .filter(|t| self.allowed.contains(t.name()))
                .count()
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use savant_core::traits::Tool;

    struct MockTool {
        name: String,
    }

    #[async_trait::async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "mock"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
        ) -> Result<String, savant_core::error::SavantError> {
            Ok("ok".to_string())
        }
    }

    fn mock_tools() -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(MockTool {
                name: "read".to_string(),
            }),
            Arc::new(MockTool {
                name: "write".to_string(),
            }),
            Arc::new(MockTool {
                name: "cargo".to_string(),
            }),
            Arc::new(MockTool {
                name: "npm".to_string(),
            }),
        ]
    }

    #[test]
    fn test_empty_filter_passes_all() {
        let filter = ToolFilter::new(vec![], mock_tools());
        assert_eq!(filter.available_count(), 4);
        assert!(filter.is_tool_available("read"));
        assert!(filter.is_tool_available("cargo"));
    }

    #[test]
    fn test_filter_restricts_tools() {
        let filter = ToolFilter::new(vec!["read".to_string(), "cargo".to_string()], mock_tools());
        assert_eq!(filter.available_count(), 2);
        assert!(filter.is_tool_available("read"));
        assert!(filter.is_tool_available("cargo"));
        assert!(!filter.is_tool_available("write"));
        assert!(!filter.is_tool_available("npm"));
    }

    #[test]
    fn test_filter_no_match() {
        let filter = ToolFilter::new(vec!["nonexistent".to_string()], mock_tools());
        assert_eq!(filter.available_count(), 0);
        assert!(!filter.is_tool_available("read"));
    }
}
