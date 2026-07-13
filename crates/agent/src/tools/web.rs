// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

/// Maximum HTTP response body size (10 MB).
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// WebSovereign: HTTP fetch + DOM→Markdown conversion engine.
///
/// Implements a 3-tier web content extraction pipeline:
/// 1. HTTP fetch with SSRF protection
/// 2. DOM parsing via `scraper` crate (CSS selector-based)
/// 3. Content-root detection (main → article → [role=main] → body)
///
/// Actions:
/// - navigate: Fetch URL and return Markdown content
/// - snapshot: Fetch URL and return full DOM→Markdown (all elements)
/// - scrape: Extract text from specific CSS selector
pub struct WebSovereign {
    http: reqwest::Client,
    projection: Arc<super::web_projection::ChromeProjection>,
    taint_tracker: Arc<savant_security::continuous::taint::TaintTracker>,
}

impl WebSovereign {
    pub fn new() -> Result<Self, SavantError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5))
            .user_agent("Savant/1.6")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| {
                SavantError::Unknown(format!(
                    "CRITICAL: Failed to build HTTP client with security constraints: {}",
                    e
                ))
            })?;
        Ok(Self {
            http,
            projection: Arc::new(super::web_projection::ChromeProjection::new()),
            taint_tracker: Arc::new(savant_security::continuous::taint::TaintTracker::new()),
        })
    }

    fn max_output_chars(&self) -> usize {
        50_000
    }

    /// Fetches a URL with SSRF protection and body size limit.
    async fn fetch_url(&self, url: &str) -> Result<String, SavantError> {
        // PB-01: Use shared SSRF validation
        savant_core::net::validate_url(url)?;

        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("HTTP request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(SavantError::Unknown(format!(
                "HTTP {} for {}",
                status.as_u16(),
                url
            )));
        }

        // PB-03: Chunked body reading with size limit
        let mut bytes: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                SavantError::Unknown(format!("Failed to read response chunk: {}", e))
            })?;
            if bytes.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(SavantError::Unknown(format!(
                    "Response body exceeds {}MB limit",
                    MAX_BODY_BYTES / (1024 * 1024)
                )));
            }
            bytes.extend_from_slice(&chunk);
        }

        String::from_utf8(bytes)
            .map_err(|e| SavantError::Unknown(format!("Response body is not valid UTF-8: {}", e)))
    }

    /// Converts raw HTML to structured Markdown using ChromeProjection's
    /// content-root detection and Markdown conversion pipeline.
    async fn html_to_markdown(&self, html: &str, url: &str) -> String {
        self.projection.project_html(html, url).await
    }

    /// Extracts text content from a scraped element as Markdown.
    /// Used by the `scrape` action to get text from CSS selector matches.
    fn node_to_markdown(&self, node: &ElementRef, _depth: usize) -> String {
        // Collect all descendant text from the element
        let text: String = node
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        text
    }
}

/// Truncates a string at a safe UTF-8 character boundary.
fn truncate_safe(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let byte_end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!(
        "{}\n\n[... truncated at {} chars]",
        &s[..byte_end],
        max_chars
    )
}

/// Ensure a URL has a scheme. LLMs often omit https:// prefix.
fn ensure_scheme(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    }
}

#[async_trait]
impl Tool for WebSovereign {
    fn name(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Web operations: navigate to URLs, take DOM snapshots, scrape content. Supports HTTP fetch with SSRF protection and HTML→Markdown conversion."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["navigate", "snapshot", "scrape"]
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to or snapshot"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for scrape action (optional)"
                }
            },
            "required": ["action"]
        })
    }

    fn max_output_chars(&self) -> usize {
        50_000
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let action = payload["action"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'action' field".to_string()))?;

        match action {
            "navigate" => {
                let url = payload["url"]
                    .as_str()
                    .ok_or_else(|| SavantError::Unknown("Missing 'url' for navigate".into()))?;

                // Ensure URL has a scheme — LLMs often omit https://
                let url = ensure_scheme(url);
                let html = self.fetch_url(&url).await?;
                let markdown = self.html_to_markdown(&html, &url).await;

                // Tag fetched data as external web content (low trust)
                let data_id = format!("web:{}", url);
                self.taint_tracker.tag(
                    &data_id,
                    savant_security::continuous::taint::TaintTag::external_web(),
                );

                // PB-02: Char-boundary-safe truncation
                let truncated = truncate_safe(&markdown, self.max_output_chars());

                info!(
                    "[WEB] Navigate to {} — {} chars (taint: external_web, trust: 0.2)",
                    url,
                    truncated.len()
                );
                Ok(format!("URL: {}\n\n{}", url, truncated))
            }
            "snapshot" => {
                let url = payload["url"]
                    .as_str()
                    .ok_or_else(|| SavantError::Unknown("Missing 'url' for snapshot".into()))?;

                let url = ensure_scheme(url);
                let html = self.fetch_url(&url).await?;

                // Tag fetched data as external web content (low trust)
                let data_id = format!("web:{}", url);
                self.taint_tracker.tag(
                    &data_id,
                    savant_security::continuous::taint::TaintTag::external_web(),
                );

                // Use ChromeProjection for snapshot — adds SHA256 boundary markers
                // for content injection prevention (enterprise security)
                let projected = self.projection.project_html(&html, &url).await;

                // PB-12: Truncate snapshot output
                let truncated = truncate_safe(&projected, self.max_output_chars());

                info!(
                    "[WEB] Snapshot of {} — {} chars (taint: external_web, trust: 0.2)",
                    url,
                    truncated.len()
                );
                Ok(truncated)
            }
            "scrape" => {
                let url = payload["url"]
                    .as_str()
                    .ok_or_else(|| SavantError::Unknown("Missing 'url' for scrape".into()))?;

                let url = ensure_scheme(url);
                let selector_str = payload["selector"].as_str().unwrap_or("body");

                let html = self.fetch_url(&url).await?;

                // Tag fetched data as external web content (low trust)
                let data_id = format!("web:{}", url);
                self.taint_tracker.tag(
                    &data_id,
                    savant_security::continuous::taint::TaintTag::external_web(),
                );

                let document = Html::parse_document(&html);

                let selector = Selector::parse(selector_str).map_err(|e| {
                    SavantError::Unknown(format!(
                        "Invalid CSS selector '{}': {:?}",
                        selector_str, e
                    ))
                })?;

                let mut results = Vec::new();
                for element in document.select(&selector) {
                    let text = self.node_to_markdown(&element, 0);
                    if !text.trim().is_empty() {
                        results.push(text.trim().to_string());
                    }
                }

                if results.is_empty() {
                    Ok(format!(
                        "No elements matched selector '{}' at {}",
                        selector_str, url
                    ))
                } else {
                    // PB-12: Truncate scrape output
                    let joined = results.join("\n---\n");
                    Ok(truncate_safe(&joined, self.max_output_chars()))
                }
            }
            _ => Err(SavantError::Unknown(format!(
                "Unknown web action: '{}'. Use: navigate, snapshot, scrape",
                action
            ))),
        }
    }

    fn capabilities(&self) -> savant_core::types::CapabilityGrants {
        savant_core::types::CapabilityGrants {
            network_allow: ["http".to_string(), "https".to_string()]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }
}
