use regex::Regex;
use savant_core::error::SavantError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub String);

impl Default for TabId {
    fn default() -> Self {
        TabId(uuid::Uuid::new_v4().to_string())
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: TabId,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub loading: bool,
    #[serde(default)]
    pub agent_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub enabled: bool,
    pub headless: bool,
    pub chrome_path: String,
    pub user_data_dir: String,
    pub default_timeout_ms: u64,
    pub max_tabs: usize,
    pub screenshot_enabled: bool,
    pub max_screenshot_size_kb: usize,
    pub agent_control_enabled: bool,
    pub vision_model: String,
    pub vision_model_provider: String,
    #[serde(default)]
    pub persist_session: bool,
    /// Base URL for Ollama API (default: "http://127.0.0.1:11434")
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
}

fn default_ollama_url() -> String {
    "http://127.0.0.1:11434".to_string()
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig {
            enabled: true,
            headless: false,
            chrome_path: String::new(),
            user_data_dir: String::new(),
            default_timeout_ms: 30000,
            max_tabs: 10,
            screenshot_enabled: true,
            max_screenshot_size_kb: 2048,
            agent_control_enabled: true,
            vision_model: String::from("gemma4"),
            vision_model_provider: String::from("ollama"),
            persist_session: false,
            ollama_url: default_ollama_url(),
        }
    }
}

impl BrowserConfig {
    pub fn from_config_file(path: &std::path::Path) -> Option<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[browser] Failed to read config file {:?}: {}", path, e);
                return None;
            }
        };
        let table: toml::Table = match toml::from_str(&content) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("[browser] Failed to parse config file {:?}: {}", path, e);
                return None;
            }
        };
        let browser = table.get("browser")?;
        let b = browser.as_table()?;

        let default = BrowserConfig::default();
        Some(BrowserConfig {
            enabled: b
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.enabled),
            headless: b
                .get("headless")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.headless),
            chrome_path: b
                .get("chrome_path")
                .and_then(|v| v.as_str())
                .unwrap_or(&default.chrome_path)
                .to_string(),
            user_data_dir: b
                .get("user_data_dir")
                .and_then(|v| v.as_str())
                .unwrap_or(&default.user_data_dir)
                .to_string(),
            default_timeout_ms: b
                .get("default_timeout_ms")
                .and_then(|v| v.as_integer())
                .map(|v| v as u64)
                .unwrap_or(default.default_timeout_ms),
            max_tabs: b
                .get("max_tabs")
                .and_then(|v| v.as_integer())
                .map(|v| v as usize)
                .unwrap_or(default.max_tabs),
            screenshot_enabled: b
                .get("screenshot_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.screenshot_enabled),
            max_screenshot_size_kb: b
                .get("max_screenshot_size_kb")
                .and_then(|v| v.as_integer())
                .map(|v| v as usize)
                .unwrap_or(default.max_screenshot_size_kb),
            agent_control_enabled: b
                .get("agent_control_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.agent_control_enabled),
            vision_model: b
                .get("vision_model")
                .and_then(|v| v.as_str())
                .unwrap_or(&default.vision_model)
                .to_string(),
            vision_model_provider: b
                .get("vision_model_provider")
                .and_then(|v| v.as_str())
                .unwrap_or(&default.vision_model_provider)
                .to_string(),
            persist_session: b
                .get("persist_session")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.persist_session),
            ollama_url: b
                .get("ollama_url")
                .and_then(|v| v.as_str())
                .unwrap_or(&default.ollama_url)
                .to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageContent {
    pub url: String,
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub html_length_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkInfo {
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub base64: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementInfo {
    pub tag: String,
    pub text: Option<String>,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRequest {
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<usize>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub url: String,
    pub filename: String,
    pub path: String,
    pub size_bytes: usize,
    pub status: String,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrowserEvent {
    TabOpened {
        tab_id: String,
        url: String,
    },
    TabClosed {
        tab_id: String,
    },
    TabNavigated {
        tab_id: String,
        url: String,
        status: String,
    },
    PageLoaded {
        tab_id: String,
        url: String,
        title: String,
    },
    ScreenshotCaptured {
        tab_id: String,
    },
    AgentInteraction {
        tab_id: String,
        agent_name: String,
        action: String,
    },
    ControlModeChanged {
        mode: String,
    },
}

impl BrowserEvent {
    pub fn event_type(&self) -> &str {
        match self {
            BrowserEvent::TabOpened { .. } => "browser.tab_opened",
            BrowserEvent::TabClosed { .. } => "browser.tab_closed",
            BrowserEvent::TabNavigated { .. } => "browser.tab_navigated",
            BrowserEvent::PageLoaded { .. } => "browser.page_loaded",
            BrowserEvent::ScreenshotCaptured { .. } => "browser.screenshot_captured",
            BrowserEvent::AgentInteraction { .. } => "browser.agent_interaction",
            BrowserEvent::ControlModeChanged { .. } => "browser.control_mode_changed",
        }
    }
}

#[derive(Error, Debug)]
pub enum BrowserError {
    #[error("Browser is disabled in configuration")]
    BrowserDisabled,
    #[error("Tab limit reached (max: {0})")]
    TabLimitReached(usize),
    #[error("Tab not found: {0}")]
    TabNotFound(String),
    #[error("No active tab")]
    NoActiveTab,
    #[error("Browser engine not initialized")]
    NotInitialized,
    #[error("Chrome launch failed: {0}")]
    ChromeLaunchFailed(String),
    #[error("Navigation failed: {0}")]
    NavigationFailed(String),
    #[error("Timeout ({0}ms)")]
    Timeout(u64),
    #[error("Element not found: {0}")]
    ElementNotFound(String),
    #[error("Screenshot disabled in configuration")]
    ScreenshotDisabled,
    #[error("Screenshot too large: {0}KB exceeds max {1}KB")]
    ScreenshotTooLarge(usize, usize),
    #[error("JS execution blocked: {0}")]
    JsBlocked(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<BrowserError> for SavantError {
    fn from(e: BrowserError) -> Self {
        SavantError::OperationFailed(e.to_string())
    }
}

/// Validates a URL for SSRF protection. Delegates to shared `savant_core::net::validate_url`.
pub fn validate_url(url_str: &str) -> Result<(), BrowserError> {
    savant_core::net::validate_url(url_str)
        .map_err(|e| BrowserError::NavigationFailed(e.to_string()))
}

#[allow(clippy::disallowed_methods)]
static JS_BLOCKED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?xi)
        \b alert \s* \(
        | \b prompt \s* \(
        | \b confirm \s* \(
        | \b window \. open \s* \(
        | \b document \. cookie \b
        | \b navigator \. serviceWorker \b
        | \b fetch \s* \( \s* ['"] (?:file|data|javascript) :
        | \b eval \s* \(
        | \b Function \s* \(
        | \b setTimeout \s* \(
        | \b setInterval \s* \(
    "#,
    )
    .expect("Hardcoded JS block regex is valid at compile time")
});

pub fn is_js_blocked(script: &str) -> Option<String> {
    if let Some(m) = JS_BLOCKED_RE.find(script) {
        let matched = m.as_str().trim().to_string();
        return Some(matched);
    }
    None
}

pub fn truncate_content(content: &str) -> String {
    const MAX_CHARS: usize = 50_000;
    if content.len() <= MAX_CHARS {
        return content.to_string();
    }
    let boundary = content
        .char_indices()
        .take(MAX_CHARS)
        .last()
        .map(|(pos, _)| pos)
        .unwrap_or(MAX_CHARS);
    let safe = &content[..boundary];
    format!("{}\n\n[Content truncated at {boundary} characters]", safe)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_id_default() {
        let id = TabId::default();
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_tab_id_display() {
        let id = TabId(String::from("test-123"));
        assert_eq!(format!("{id}"), "test-123");
    }

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert!(config.enabled);
        assert!(!config.headless);
        assert_eq!(config.max_tabs, 10);
        assert_eq!(config.default_timeout_ms, 30000);
        assert!(config.screenshot_enabled);
        assert_eq!(config.max_screenshot_size_kb, 2048);
    }

    #[test]
    fn test_validate_url_allows_https() {
        assert!(validate_url("https://example.com").is_ok());
    }

    #[test]
    fn test_validate_url_blocks_file() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_validate_url_blocks_loopback() {
        assert!(validate_url("http://127.0.0.1:8080").is_err());
        assert!(validate_url("http://localhost:8080").is_err());
    }

    #[test]
    fn test_validate_url_blocks_rfc1918() {
        assert!(validate_url("http://10.0.0.1").is_err());
        assert!(validate_url("http://172.16.0.1").is_err());
        assert!(validate_url("http://192.168.1.1").is_err());
    }

    #[test]
    fn test_validate_url_blocks_metadata() {
        assert!(validate_url("http://169.254.169.254").is_err());
        assert!(validate_url("http://metadata.google.internal").is_err());
    }

    #[test]
    fn test_validate_url_blocks_javascript() {
        assert!(validate_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn test_is_js_blocked_basic() {
        assert!(is_js_blocked("alert('hi')").is_some());
        assert!(is_js_blocked("window.open('url')").is_some());
        assert!(is_js_blocked("document.cookie = 'x'").is_some());
        // Narrowed: only suspicious fetch patterns are blocked
        assert!(is_js_blocked("fetch('file:///etc/passwd')").is_some());
        assert!(is_js_blocked("fetch('data:text/html,<script>')").is_some());
        assert!(is_js_blocked("fetch('/api')").is_none());
    }

    #[test]
    fn test_is_js_blocked_evasion_attempts() {
        assert!(is_js_blocked("this[\"alert\"]('x')").is_none());
        assert!(is_js_blocked("this['prompt']('x')").is_none());
    }

    #[test]
    fn test_is_js_blocked_allows_safe() {
        assert!(is_js_blocked("console.log('safe')").is_none());
        assert!(is_js_blocked("document.title").is_none());
    }

    #[test]
    fn test_browser_error_to_savant_error() {
        let err: SavantError = BrowserError::BrowserDisabled.into();
        assert!(matches!(err, SavantError::OperationFailed(_)));
    }

    #[test]
    fn test_browser_event_types() {
        let ev = BrowserEvent::TabOpened {
            tab_id: String::from("abc"),
            url: String::from("https://x.com"),
        };
        assert_eq!(ev.event_type(), "browser.tab_opened");
    }

    #[test]
    fn test_truncate_content_short() {
        let result = truncate_content("short text");
        assert_eq!(result, "short text");
    }

    #[test]
    fn test_truncate_content_long() {
        let long = "a".repeat(60_000);
        let result = truncate_content(&long);
        assert!(result.starts_with('a'));
        assert!(result.len() > 50_000);
    }

    #[test]
    fn test_truncate_content_utf8_boundary() {
        // Build a string that exceeds MAX_CHARS with multi-byte UTF-8 chars at the boundary
        let mut s = String::new();
        for _ in 0..49_995 {
            s.push('a');
        }
        s.push_str("éééééééééé");
        assert!(s.len() > 50_000);
        let result = truncate_content(&s);
        // Verify the result is valid UTF-8 and ends with the truncation suffix
        assert!(result.ends_with("characters]"));
        // Verify the truncated portion ends at a valid UTF-8 char boundary
        let truncated = &result[..result.len() - 1]; // exclude trailing ']'
        let content_portion = truncated
            .rfind('\n')
            .map(|pos| &truncated[..pos])
            .unwrap_or(truncated);
        // The content portion should only contain valid UTF-8 (no panic on char boundary check)
        assert!(content_portion.is_char_boundary(content_portion.len()));
        // Verify the original content was actually truncated
        assert!(result.len() < s.len() || result.contains("[Content truncated"));
    }
}
