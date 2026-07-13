//! Hook/Lifecycle System — runtime extensibility for agent lifecycle events.
//!
//! Two tiers of hooks with panic safety:
//! - **Void** (fire-and-forget, parallel): All handlers run concurrently
//! - **Modifying** (sequential, priority-ordered): Handlers can modify or cancel
//!
//! Hooks are wrapped in `catch_unwind` — panics in hooks are caught and logged,
//! never crashing the agent loop (zeroclaw pattern).

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Hook event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    // Tool lifecycle
    BeforeToolCall,
    AfterToolCall,
    ToolError,
    // LLM lifecycle
    BeforeLlmCall,
    AfterLlmCall,
    LlmInput,
    LlmOutput,
    LlmError,
    // Session lifecycle
    SessionStart,
    SessionEnd,
    // Agent lifecycle
    CheckSignals,
    BuildIdentity,
    HeartbeatTick,
    TurnStart,
    TurnEnd,
}

/// Hook execution strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStrategy {
    /// All handlers run in parallel, errors logged
    Void,
    /// Handlers run sequentially by priority, first can modify or cancel
    Modifying,
}

/// Hook priority — higher runs first.
pub type HookPriority = i32;

/// Hook handler trait for void hooks (fire-and-forget, parallel).
#[async_trait::async_trait]
pub trait VoidHookHandler: Send + Sync {
    fn event(&self) -> HookEvent;
    fn priority(&self) -> HookPriority {
        0
    }
    async fn handle(&self, context: &HookContext);
}

/// Hook context passed to handlers.
#[derive(Debug, Clone)]
pub struct HookContext {
    pub event: HookEvent,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub tool_name: Option<String>,
    pub content: Option<String>,
    pub error: Option<String>,
    pub metadata: HashMap<String, String>,
}

/// Hook result for modifying hooks.
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Content was modified
    Modified(String),
    /// Content unchanged, continue
    Unchanged,
    /// Cancel the operation with a reason
    Cancel(String),
}

/// Modifying hook handler trait (sequential, can modify or cancel).
#[async_trait::async_trait]
pub trait ModifyingHookHandler: Send + Sync {
    fn event(&self) -> HookEvent;
    fn priority(&self) -> HookPriority {
        0
    }
    async fn handle(&self, context: &mut HookContext) -> HookResult;
}

/// Hook registration entry for void hooks.
struct VoidHookRegistration {
    handler: Arc<dyn VoidHookHandler>,
    priority: HookPriority,
}

/// Hook registration entry for modifying hooks.
struct ModifyingHookRegistration {
    handler: Arc<dyn ModifyingHookHandler>,
    priority: HookPriority,
}

/// Hook registry — manages lifecycle hooks.
/// Uses two tiers: void hooks (parallel) and modifying hooks (sequential with cancel).
pub struct HookRegistry {
    void_handlers: RwLock<HashMap<HookEvent, Vec<VoidHookRegistration>>>,
    modifying_handlers: RwLock<HashMap<HookEvent, Vec<ModifyingHookRegistration>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            void_handlers: RwLock::new(HashMap::new()),
            modifying_handlers: RwLock::new(HashMap::new()),
        }
    }

    /// Registers a void hook handler (fire-and-forget, parallel).
    pub async fn register_void(&self, handler: impl VoidHookHandler + 'static) {
        let event = handler.event();
        let priority = handler.priority();
        let mut handlers = self.void_handlers.write().await;
        let entry = handlers.entry(event).or_default();
        entry.push(VoidHookRegistration {
            handler: Arc::new(handler),
            priority,
        });
        entry.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Registers a modifying hook handler (sequential, can cancel).
    pub async fn register_modifying(&self, handler: impl ModifyingHookHandler + 'static) {
        let event = handler.event();
        let priority = handler.priority();
        let mut handlers = self.modifying_handlers.write().await;
        let entry = handlers.entry(event).or_default();
        entry.push(ModifyingHookRegistration {
            handler: Arc::new(handler),
            priority,
        });
        entry.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Runs void hooks for an event (parallel via spawn_blocking, panic-safe).
    /// COR-10: Uses spawn_blocking instead of block_on inside spawned task.
    pub async fn run_void(&self, context: &HookContext) {
        let handlers = self.void_handlers.read().await;
        if let Some(event_handlers) = handlers.get(&context.event) {
            let mut tasks = Vec::new();
            for reg in event_handlers {
                let ctx = context.clone();
                let handler = reg.handler.clone();
                tasks.push(tokio::task::spawn_blocking(move || {
                    // Catch panics — hooks must never crash the agent
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(handler.handle(&ctx));
                    }));
                    if let Err(e) = result {
                        let msg = e
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| e.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".to_string());
                        tracing::error!("[HOOK] Void hook panicked: {}", msg);
                    }
                }));
            }
            for task in tasks {
                if let Err(e) = task.await {
                    tracing::warn!(
                        "[core::hooks] Void hook task panicked or was cancelled: {}",
                        e
                    );
                }
            }
        }
    }

    /// Runs modifying hooks for an event (sequential by priority, first Cancel stops chain).
    /// COR-09: Wrapped in catch_unwind for panic safety, matching run_void.
    pub async fn run_modifying(&self, context: &mut HookContext) -> HookResult {
        let handlers = self.modifying_handlers.read().await;
        if let Some(event_handlers) = handlers.get(&context.event) {
            for reg in event_handlers {
                let handler = reg.handler.clone();
                let ctx = context.clone();
                // Use spawn_blocking + catch_unwind for panic safety
                let result = tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let rt = tokio::runtime::Handle::current();
                        let mut ctx = ctx;
                        rt.block_on(handler.handle(&mut ctx))
                    }))
                    .unwrap_or_else(|e| {
                        let msg = e
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| e.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".to_string());
                        tracing::error!("[HOOK] Modifying hook panicked: {}", msg);
                        HookResult::Unchanged
                    })
                })
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("[core::hooks] Modifying hook task failed: {}", e);
                    HookResult::Unchanged
                });
                match result {
                    HookResult::Cancel(reason) => {
                        tracing::info!(
                            "[HOOK] Modifying hook cancelled {}: {}",
                            format!("{:?}", context.event),
                            reason
                        );
                        return HookResult::Cancel(reason);
                    }
                    HookResult::Modified(content) => {
                        context.content = Some(content.clone());
                        return HookResult::Modified(content);
                    }
                    HookResult::Unchanged => continue,
                }
            }
        }
        HookResult::Unchanged
    }

    /// Gets the number of registered void handlers for an event.
    pub async fn handler_count(&self, event: HookEvent) -> usize {
        let handlers = self.void_handlers.read().await;
        handlers.get(&event).map(|v| v.len()).unwrap_or(0)
    }

    /// Gets the number of registered modifying handlers for an event.
    pub async fn modifying_handler_count(&self, event: HookEvent) -> usize {
        let handlers = self.modifying_handlers.read().await;
        handlers.get(&event).map(|v| v.len()).unwrap_or(0)
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Built-in hook implementations
// ============================================================================

/// Logging hook — logs tool calls and outputs.
pub struct ToolCallLogger;

#[async_trait::async_trait]
impl VoidHookHandler for ToolCallLogger {
    fn event(&self) -> HookEvent {
        HookEvent::AfterToolCall
    }
    fn priority(&self) -> HookPriority {
        0
    }

    async fn handle(&self, context: &HookContext) {
        if let Some(ref tool_name) = context.tool_name {
            tracing::debug!("[HOOK] Tool executed: {}", tool_name);
        }
    }
}

/// LLM input logger — logs context before LLM call.
pub struct LlmInputLogger;

#[async_trait::async_trait]
impl VoidHookHandler for LlmInputLogger {
    fn event(&self) -> HookEvent {
        HookEvent::BeforeLlmCall
    }
    fn priority(&self) -> HookPriority {
        0
    }

    async fn handle(&self, context: &HookContext) {
        if let Some(ref content) = context.content {
            tracing::debug!("[HOOK] LLM input: {} chars", content.len());
        }
    }
}

/// LLM output logger — logs response after LLM call.
pub struct LlmOutputLogger;

#[async_trait::async_trait]
impl VoidHookHandler for LlmOutputLogger {
    fn event(&self) -> HookEvent {
        HookEvent::AfterLlmCall
    }
    fn priority(&self) -> HookPriority {
        0
    }

    async fn handle(&self, context: &HookContext) {
        if let Some(ref content) = context.content {
            tracing::debug!("[HOOK] LLM output: {} chars", content.len());
        }
    }
}

/// Health monitor hook — logs heartbeat ticks.
pub struct HealthMonitorHook;

#[async_trait::async_trait]
impl VoidHookHandler for HealthMonitorHook {
    fn event(&self) -> HookEvent {
        HookEvent::HeartbeatTick
    }
    fn priority(&self) -> HookPriority {
        -10
    }

    async fn handle(&self, _context: &HookContext) {
        tracing::debug!("[HOOK] Heartbeat tick received");
    }
}

/// Session lifecycle hook — logs session start/end.
pub struct SessionLifecycleHook;

#[async_trait::async_trait]
impl VoidHookHandler for SessionLifecycleHook {
    fn event(&self) -> HookEvent {
        HookEvent::SessionStart
    }
    fn priority(&self) -> HookPriority {
        0
    }

    async fn handle(&self, context: &HookContext) {
        let session_id = context.session_id.as_deref().unwrap_or("unknown");
        tracing::info!("[HOOK] Session started: {}", session_id);
    }
}

/// Session end lifecycle hook.
pub struct SessionEndHook;

#[async_trait::async_trait]
impl VoidHookHandler for SessionEndHook {
    fn event(&self) -> HookEvent {
        HookEvent::SessionEnd
    }
    fn priority(&self) -> HookPriority {
        0
    }

    async fn handle(&self, context: &HookContext) {
        let session_id = context.session_id.as_deref().unwrap_or("unknown");
        tracing::info!("[HOOK] Session ended: {}", session_id);
    }
}

/// C5: Default BeforeToolCall hook — logs tool name and args for observability.
/// This hook is void (fire-and-forget) — it logs but cannot cancel execution.
pub struct BeforeToolCallLogger;

#[async_trait::async_trait]
impl VoidHookHandler for BeforeToolCallLogger {
    fn event(&self) -> HookEvent {
        HookEvent::BeforeToolCall
    }

    fn priority(&self) -> HookPriority {
        0 // lowest priority — runs after any user-registered hooks
    }

    async fn handle(&self, context: &HookContext) {
        if let Some(ref tool_name) = context.tool_name {
            let args_preview = context.content.as_deref().unwrap_or("{}");
            let truncated = if args_preview.len() > 200 {
                format!("{}...", &args_preview[..200])
            } else {
                args_preview.to_string()
            };
            tracing::debug!(
                "[hook] BeforeToolCall: tool={} args={}",
                tool_name,
                truncated
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    struct TestVoidHandler;

    #[async_trait::async_trait]
    impl VoidHookHandler for TestVoidHandler {
        fn event(&self) -> HookEvent {
            HookEvent::BeforeToolCall
        }
        async fn handle(&self, _context: &HookContext) {}
    }

    struct TestModifyingHandler;

    #[async_trait::async_trait]
    impl ModifyingHookHandler for TestModifyingHandler {
        fn event(&self) -> HookEvent {
            HookEvent::BeforeLlmCall
        }
        async fn handle(&self, context: &mut HookContext) -> HookResult {
            if let Some(ref content) = context.content {
                if content.contains("BLOCK") {
                    return HookResult::Cancel("Blocked by handler".to_string());
                }
            }
            HookResult::Unchanged
        }
    }

    #[tokio::test]
    async fn test_hook_registry_register_and_count() {
        let registry = HookRegistry::new();
        registry.register_void(TestVoidHandler).await;
        assert_eq!(registry.handler_count(HookEvent::BeforeToolCall).await, 1);
        assert_eq!(registry.handler_count(HookEvent::AfterToolCall).await, 0);
    }

    #[tokio::test]
    async fn test_hook_registry_run_void() {
        let registry = HookRegistry::new();
        registry.register_void(ToolCallLogger).await;
        let context = HookContext {
            event: HookEvent::AfterToolCall,
            session_id: None,
            agent_id: None,
            tool_name: Some("shell".to_string()),
            content: None,
            error: None,
            metadata: HashMap::new(),
        };
        registry.run_void(&context).await;
    }

    #[tokio::test]
    async fn test_modifying_hook_cancel() {
        let registry = HookRegistry::new();
        registry.register_modifying(TestModifyingHandler).await;
        let mut context = HookContext {
            event: HookEvent::BeforeLlmCall,
            session_id: None,
            agent_id: None,
            tool_name: None,
            content: Some("BLOCK this request".to_string()),
            error: None,
            metadata: HashMap::new(),
        };
        let result = registry.run_modifying(&mut context).await;
        assert!(matches!(result, HookResult::Cancel(_)));
    }

    #[tokio::test]
    async fn test_modifying_hook_unchanged() {
        let registry = HookRegistry::new();
        registry.register_modifying(TestModifyingHandler).await;
        let mut context = HookContext {
            event: HookEvent::BeforeLlmCall,
            session_id: None,
            agent_id: None,
            tool_name: None,
            content: Some("Allow this request".to_string()),
            error: None,
            metadata: HashMap::new(),
        };
        let result = registry.run_modifying(&mut context).await;
        assert!(matches!(result, HookResult::Unchanged));
    }
}
