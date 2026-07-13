use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::{ApprovalRequirement, Tool};
use savant_core::types::CapabilityGrants;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::info;

use crate::engine::BrowserEngine;
use crate::types::BrowserConfig;

#[allow(clippy::disallowed_methods)]
static VISION_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to build custom vision HTTP client, falling back to default: {e}"
            );
            reqwest::Client::new()
        })
});

pub struct BrowserTool {
    config: BrowserConfig,
    engine: OnceCell<Arc<BrowserEngine>>,
}

impl BrowserTool {
    pub fn new(config: BrowserConfig) -> Self {
        BrowserTool {
            config,
            engine: OnceCell::new(),
        }
    }

    async fn get_engine(&self) -> Result<&Arc<BrowserEngine>, SavantError> {
        self.engine
            .get_or_try_init(|| async {
                let engine = BrowserEngine::launch(&self.config).await.map_err(|e| {
                    SavantError::OperationFailed(format!("Browser launch failed: {e}"))
                })?;
                info!("[browser::tool] BrowserEngine lazy-initialized on first use");
                Ok(engine)
            })
            .await
    }
}

// ── Action handlers ────────────────────────────────────────────────

impl BrowserTool {
    async fn handle_navigate(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let url = payload["url"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'url'"))
        })?;
        let result = engine.navigate(url).await?;
        serde_json::to_string_pretty(&result)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_get_text(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let text = engine.get_text().await?;
        Ok(text)
    }

    async fn handle_get_content(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let content = engine.get_content().await?;
        Ok(content)
    }

    async fn handle_get_links(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let links = engine.get_links().await?;
        serde_json::to_string_pretty(&links)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_get_tables(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let tables = engine.get_tables().await?;
        serde_json::to_string_pretty(&tables)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_click(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let selector = payload["selector"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'selector'"))
        })?;
        let info = engine.click(selector).await?;
        serde_json::to_string_pretty(&info)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_type_text(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let selector = payload["selector"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'selector'"))
        })?;
        let text = payload["text"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'text'"))
        })?;
        let info = engine.type_text(selector, text).await?;
        serde_json::to_string_pretty(&info)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_scroll(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let position = payload["position"].as_str().unwrap_or("bottom");
        let amount = payload["amount"].as_i64();
        let result = engine.scroll(position, amount).await?;
        Ok(result)
    }

    async fn handle_screenshot(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let result = engine.screenshot().await?;
        serde_json::to_string_pretty(&result)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_analyze_page(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let prompt = payload["prompt"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'prompt'"))
        })?;
        let screenshot_result = engine.screenshot().await?;
        let description =
            call_vision_model(&self.config, &screenshot_result.base64, prompt).await?;
        Ok(description)
    }

    async fn handle_go_back(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let url = engine.go_back().await?;
        Ok(format!("Navigated back to: {url}"))
    }

    async fn handle_go_forward(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let url = engine.go_forward().await?;
        Ok(format!("Navigated forward to: {url}"))
    }

    async fn handle_reload(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let result = engine.reload().await?;
        Ok(format!("Page reloaded: {} - {}", result.title, result.url))
    }

    async fn handle_wait_for_selector(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let selector = payload["selector"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'selector'"))
        })?;
        let timeout = payload["timeout_ms"]
            .as_u64()
            .unwrap_or(self.config.default_timeout_ms);
        let info = engine.wait_for_selector(selector, timeout).await?;
        serde_json::to_string_pretty(&info)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_execute_js(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let script = payload["script"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'script'"))
        })?;
        let result = engine.execute_js(script).await?;
        serde_json::to_string_pretty(&result)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_highlight(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let selector = payload["selector"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'selector'"))
        })?;
        let result = engine.highlight(selector).await?;
        Ok(result)
    }

    async fn handle_new_tab(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let url = payload["url"].as_str().unwrap_or("about:blank");
        let tab_id = engine.create_tab(url.to_string()).await?;
        Ok(format!("Created tab {tab_id}"))
    }

    async fn handle_switch_tab(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let tab_id_str = payload["tab_id"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'tab_id'"))
        })?;
        let tab_id = crate::types::TabId(tab_id_str.to_string());
        let info = engine.switch_tab(&tab_id).await?;
        serde_json::to_string_pretty(&info)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_close_tab(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let tab_id_str = payload["tab_id"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'tab_id'"))
        })?;
        let tab_id = crate::types::TabId(tab_id_str.to_string());
        engine.close_tab(&tab_id).await?;
        Ok(format!("Closed tab {tab_id_str}"))
    }

    async fn handle_list_tabs(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let tabs = engine.list_tabs().await;
        serde_json::to_string_pretty(&tabs)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_network_monitor(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let result = engine.enable_network_monitoring().await?;
        Ok(result)
    }

    async fn handle_get_network_log(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let requests = engine.get_network_requests().await?;
        serde_json::to_string_pretty(&requests)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_block_urls(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let patterns: Vec<String> = payload["patterns"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if patterns.is_empty() {
            return Err(SavantError::InvalidInput(String::from(
                "Missing required field: 'patterns' (array of URL glob patterns)",
            )));
        }
        let result = engine.block_urls(&patterns).await?;
        Ok(result)
    }

    async fn handle_enable_download_tracking(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let result = engine.enable_download_tracking().await?;
        Ok(result)
    }

    async fn handle_get_downloads(
        &self,
        engine: &Arc<BrowserEngine>,
        _payload: &Value,
    ) -> Result<String, SavantError> {
        let downloads = engine.get_downloads().await?;
        serde_json::to_string_pretty(&downloads)
            .map_err(|e| SavantError::OperationFailed(format!("JSON serialization failed: {e}")))
    }

    async fn handle_read_download(
        &self,
        engine: &Arc<BrowserEngine>,
        payload: &Value,
    ) -> Result<String, SavantError> {
        let filename = payload["filename"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'filename'"))
        })?;
        let result = engine.read_download(filename).await?;
        Ok(result)
    }
}

// ── Tool trait ─────────────────────────────────────────────────────

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Full Chromium browser control via CDP. Use this tool for: interactive pages (SPAs, \
         JavaScript-rendered content), form filling, clicking through sequences, screenshots, \
         visual inspection, and multi-step navigation flows. \
         For simple HTTP GET requests or static HTML scraping without interaction, use the \
         web tool (WebSovereign) instead. \
         Actions: navigate (url), get_text, get_content, get_links, get_tables, click (selector), \
         type_text (selector, text), scroll (position, amount), screenshot, analyze_page (prompt), \
         go_back, go_forward, reload, wait_for_selector (selector, timeout_ms), execute_js (script), \
         highlight (selector), new_tab, switch_tab (tab_id), close_tab (tab_id), list_tabs. \
         First action should typically be 'navigate' to the target URL. \
         For visual analysis, use 'analyze_page' with a prompt describing what to look for. \
         The browser window is visible to the user by default — both human and agent share the same window."
    }

    #[allow(clippy::disallowed_methods)]
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The browser action to perform",
                    "enum": [
                        "navigate", "get_text", "get_content", "get_links", "get_tables",
                        "click", "type_text", "scroll", "screenshot", "analyze_page",
                        "go_back", "go_forward", "reload", "wait_for_selector",
                        "execute_js", "highlight", "new_tab", "switch_tab", "close_tab", "list_tabs",
                        "enable_network_monitor", "get_network_log", "block_urls",
                        "enable_download_tracking", "get_downloads", "read_download"
                    ]
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (required for 'navigate' action)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the target element (required for click, type_text, highlight, wait_for_selector)"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type into an input (required for 'type_text' action)"
                },
                "script": {
                    "type": "string",
                    "description": "JavaScript to execute (required for 'execute_js' action). Blocked: alert, prompt, confirm, window.open, document.cookie, fetch, serviceWorker"
                },
                "position": {
                    "type": "string",
                    "description": "Scroll position: 'top', 'bottom', or 'percent' (required for 'scroll')",
                    "enum": ["top", "bottom", "percent"]
                },
                "amount": {
                    "type": "integer",
                    "description": "Scroll amount: percentage for 'percent' position, pixels otherwise"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds for wait_for_selector (default 30000)"
                },
                "tab_id": {
                    "type": "string",
                    "description": "Tab ID to close (required for 'close_tab')"
                },
                "prompt": {
                    "type": "string",
                    "description": "Vision analysis prompt (required for 'analyze_page'). Describes what to look for in the page screenshot. Uses the configured Ollama vision model."
                },
                "patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "URL glob patterns to block (required for 'block_urls'). Example: [\"*.google-analytics.com*\"]"
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> ApprovalRequirement {
        ApprovalRequirement::Conditional
    }

    fn capabilities(&self) -> CapabilityGrants {
        let mut network = HashSet::new();
        network.insert(String::from("http"));
        network.insert(String::from("https"));
        CapabilityGrants {
            network_allow: network,
            ..Default::default()
        }
    }

    fn max_output_chars(&self) -> usize {
        50_000
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        if !self.config.enabled || !self.config.agent_control_enabled {
            return Ok(String::from(
                "Browser is disabled in configuration. Set browser.enabled = true and browser.agent_control_enabled = true in savant.toml."
            ));
        }

        let action = payload["action"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'action'"))
        })?;

        let engine = self.get_engine().await?;

        match action {
            "navigate" => self.handle_navigate(engine, &payload).await,
            "get_text" => self.handle_get_text(engine, &payload).await,
            "get_content" => self.handle_get_content(engine, &payload).await,
            "get_links" => self.handle_get_links(engine, &payload).await,
            "get_tables" => self.handle_get_tables(engine, &payload).await,
            "click" => self.handle_click(engine, &payload).await,
            "type_text" => self.handle_type_text(engine, &payload).await,
            "scroll" => self.handle_scroll(engine, &payload).await,
            "screenshot" => self.handle_screenshot(engine, &payload).await,
            "analyze_page" => self.handle_analyze_page(engine, &payload).await,
            "go_back" => self.handle_go_back(engine, &payload).await,
            "go_forward" => self.handle_go_forward(engine, &payload).await,
            "reload" => self.handle_reload(engine, &payload).await,
            "wait_for_selector" => self.handle_wait_for_selector(engine, &payload).await,
            "execute_js" => self.handle_execute_js(engine, &payload).await,
            "highlight" => self.handle_highlight(engine, &payload).await,
            "new_tab" => self.handle_new_tab(engine, &payload).await,
            "switch_tab" => self.handle_switch_tab(engine, &payload).await,
            "close_tab" => self.handle_close_tab(engine, &payload).await,
            "list_tabs" => self.handle_list_tabs(engine, &payload).await,
            "enable_network_monitor" => self.handle_network_monitor(engine, &payload).await,
            "get_network_log" => self.handle_get_network_log(engine, &payload).await,
            "block_urls" => self.handle_block_urls(engine, &payload).await,
            "enable_download_tracking" => self.handle_enable_download_tracking(engine, &payload).await,
            "get_downloads" => self.handle_get_downloads(engine, &payload).await,
            "read_download" => self.handle_read_download(engine, &payload).await,
            _ => Err(SavantError::InvalidInput(format!(
                "Unknown browser action: '{action}'. Valid actions: navigate, get_text, get_content, get_links, get_tables, click, type_text, scroll, screenshot, analyze_page, go_back, go_forward, reload, wait_for_selector, execute_js, highlight, new_tab, switch_tab, close_tab, list_tabs"
            ))),
        }
    }
}

// ── Vision model ───────────────────────────────────────────────────

#[allow(clippy::disallowed_methods)]
async fn call_vision_model(
    config: &BrowserConfig,
    image_base64: &str,
    prompt: &str,
) -> Result<String, SavantError> {
    let provider = config.vision_model_provider.as_str();
    let model = config.vision_model.as_str();

    if provider != "ollama" {
        return Ok(format!(
            "Vision analysis requires Ollama. Provider '{provider}' is not supported. \
             Configure browser.vision_model_provider = \"ollama\"."
        ));
    }

    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "images": [image_base64],
        "stream": false,
    });

    let ollama_url = format!("{}/api/generate", config.ollama_url);
    let resp = VISION_CLIENT
        .post(&ollama_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            SavantError::OperationFailed(format!(
                "Ollama request failed. Is Ollama running at {}? Error: {e}",
                config.ollama_url
            ))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(SavantError::OperationFailed(format!(
            "Ollama returned HTTP {status}. Model '{model}' may not be installed. \
             Install with: ollama pull {model}. Response: {detail}"
        )));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| {
        SavantError::OperationFailed(format!("Failed to parse Ollama response: {e}"))
    })?;

    let response = json["response"]
        .as_str()
        .unwrap_or("No response from vision model");

    Ok(response.to_string())
}
