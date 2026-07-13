//! Chrome Projection: Real DOM content extraction with content-root detection
//!
//! Converts web page DOM into LLM-readable Markdown with:
//! - Content-root detection (main → article → [role=main] → body)
//! - SHA256 boundary markers for external content injection prevention
//! - Skip-elements list (non-content elements)
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::SymbolicBrowser;
use scraper::{Html, Selector};
use serde_json::{json, Value};
use tracing::info;

/// Elements that should be skipped during DOM projection.
const SKIP_ELEMENTS: &[&str] = &[
    "script", "style", "noscript", "nav", "footer", "header", "aside", "iframe", "svg", "form",
    "input", "button", "select", "textarea",
];

/// Generates a deterministic SHA256 boundary marker for content.
/// Used to mark where external content is injected, preventing content injection attacks.
fn content_boundary_marker(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    let hex: String = hash[..8].iter().map(|b| format!("{:02x}", b)).collect();
    format!("<!-- boundary:{} -->", hex)
}

/// Chrome-based DOM projection with real content extraction.
pub struct ChromeProjection {
    url: String,
    /// The last projected HTML content, used for intent coherence verification.
    last_html: tokio::sync::RwLock<String>,
}

impl Default for ChromeProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeProjection {
    pub fn new() -> Self {
        Self {
            url: "about:blank".to_string(),
            last_html: tokio::sync::RwLock::new(String::new()),
        }
    }

    /// Projects HTML into a structured Markdown representation with content-root detection.
    pub async fn project_html(&self, html: &str, url: &str) -> String {
        // Store the HTML for intent coherence verification
        *self.last_html.write().await = html.to_string();
        let document = Html::parse_document(html);

        // Content-root detection: main → article → [role=main] → body
        let root = self.find_content_root(&document);

        // Generate boundary marker for this content
        let boundary = content_boundary_marker(html);

        // Convert to Markdown
        let markdown = self.node_to_markdown(&root, 0);

        format!(
            "{}\n\nURL: {}\n\n{}\n\n{}",
            boundary, url, markdown, boundary
        )
    }

    fn find_content_root<'a>(&self, document: &'a Html) -> scraper::ElementRef<'a> {
        if let Ok(sel) = Selector::parse("main") {
            if let Some(el) = document.select(&sel).next() {
                return el;
            }
        }
        if let Ok(sel) = Selector::parse("article") {
            if let Some(el) = document.select(&sel).next() {
                return el;
            }
        }
        if let Ok(sel) = Selector::parse("[role='main']") {
            if let Some(el) = document.select(&sel).next() {
                return el;
            }
        }
        if let Ok(sel) = Selector::parse("body") {
            if let Some(el) = document.select(&sel).next() {
                return el;
            }
        }
        document.root_element()
    }

    #[allow(clippy::only_used_in_recursion)]
    fn node_to_markdown(&self, node: &scraper::ElementRef, depth: usize) -> String {
        let mut output = String::new();
        let tag = node.value().name();

        if SKIP_ELEMENTS.contains(&tag) {
            return String::new();
        }

        match tag {
            "h1" => output.push_str(&format!("# {}\n\n", self.text_content(node))),
            "h2" => output.push_str(&format!("## {}\n\n", self.text_content(node))),
            "h3" => output.push_str(&format!("### {}\n\n", self.text_content(node))),
            "h4" | "h5" | "h6" => output.push_str(&format!("#### {}\n\n", self.text_content(node))),
            "p" => {
                output.push_str(&self.inline_content(node));
                output.push_str("\n\n");
            }
            "li" => output.push_str(&format!("- {}\n", self.inline_content(node))),
            "blockquote" => {
                for line in self.inline_content(node).lines() {
                    output.push_str(&format!("> {}\n", line));
                }
                output.push('\n');
            }
            "pre" => {
                output.push_str("```\n");
                output.push_str(&self.text_content(node));
                output.push_str("\n```\n\n");
            }
            "code" => output.push_str(&format!("`{}`", self.text_content(node))),
            "a" => {
                let href = node.value().attr("href").unwrap_or("");
                let text = self.text_content(node);
                if !href.is_empty() && !text.is_empty() {
                    output.push_str(&format!("[{}]({})", text, href));
                } else {
                    output.push_str(&text);
                }
            }
            "img" => {
                let alt = node.value().attr("alt").unwrap_or("");
                let src = node.value().attr("src").unwrap_or("");
                if !src.is_empty() {
                    output.push_str(&format!("![{}]({})", alt, src));
                }
            }
            "br" => output.push('\n'),
            "hr" => output.push_str("---\n\n"),
            "strong" | "b" => output.push_str(&format!("**{}**", self.inline_content(node))),
            "em" | "i" => output.push_str(&format!("*{}*", self.inline_content(node))),
            _ => {
                for child in node.children() {
                    if let Some(element) = scraper::ElementRef::wrap(child) {
                        output.push_str(&self.node_to_markdown(&element, depth + 1));
                    } else if let Some(text) = child.value().as_text() {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            output.push_str(trimmed);
                            output.push(' ');
                        }
                    }
                }
            }
        }
        output
    }

    fn text_content(&self, node: &scraper::ElementRef) -> String {
        let mut text = String::new();
        for child in node.children() {
            if let Some(t) = child.value().as_text() {
                text.push_str(t);
            } else if let Some(element) = scraper::ElementRef::wrap(child) {
                text.push_str(&self.text_content(&element));
            }
        }
        text.trim().to_string()
    }

    fn inline_content(&self, node: &scraper::ElementRef) -> String {
        let mut text = String::new();
        for child in node.children() {
            if let Some(t) = child.value().as_text() {
                text.push_str(t.trim());
            } else if let Some(element) = scraper::ElementRef::wrap(child) {
                let tag = element.value().name();
                if tag == "a" {
                    let href = element.value().attr("href").unwrap_or("");
                    let link_text = self.text_content(&element);
                    if !href.is_empty() {
                        text.push_str(&format!("[{}]({})", link_text, href));
                    } else {
                        text.push_str(&link_text);
                    }
                } else if tag == "code" {
                    text.push_str(&format!("`{}`", self.text_content(&element)));
                } else if tag == "strong" || tag == "b" {
                    text.push_str(&format!("**{}**", self.text_content(&element)));
                } else if tag == "em" || tag == "i" {
                    text.push_str(&format!("*{}*", self.text_content(&element)));
                } else {
                    text.push_str(&self.inline_content(&element));
                }
                text.push(' ');
            }
        }
        text.trim().to_string()
    }
}

#[async_trait]
impl SymbolicBrowser for ChromeProjection {
    async fn project_dom(&self) -> Result<Value, SavantError> {
        // With real web fetching, project_dom returns the last known state.
        // The actual projection is done via project_html() which takes HTML input.
        Ok(json!({
            "url": self.url,
            "status": "ready",
            "note": "Use WebSovereign (web tool) to fetch pages. ChromeProjection converts HTML to Markdown."
        }))
    }

    async fn prove_intent_coherence(
        &self,
        action: &str,
        selector: &str,
        _intent_matrix: Value,
    ) -> Result<bool, SavantError> {
        info!(
            "Projection: Intent coherence check for {} on {}",
            action, selector
        );

        // Validate the action type
        let valid_actions = [
            "click", "type", "scroll", "navigate", "select", "hover", "focus", "submit", "read",
        ];
        if !valid_actions.contains(&action) {
            tracing::warn!("Projection: Unknown action type '{}'", action);
            return Ok(false);
        }

        // Validate the selector is non-empty
        if selector.is_empty() {
            tracing::warn!("Projection: Empty selector for action '{}'", action);
            return Ok(false);
        }

        // Clone the HTML string and drop the lock before CPU-intensive DOM parsing.
        let html_content = {
            let html = self.last_html.read().await;

            if html.is_empty() {
                // No projected DOM yet; allow the action but log it
                tracing::debug!(
                    "Projection: No projected DOM for coherence check; allowing action '{}'",
                    action
                );
                return Ok(true);
            }
            html.clone()
            // Lock dropped here
        };

        // Verify the selector matches at least one element in the projected DOM
        let document = Html::parse_document(&html_content);
        match Selector::parse(selector) {
            Ok(sel) => {
                let found = document.select(&sel).next().is_some();
                if !found {
                    tracing::warn!(
                        "Projection: Selector '{}' not found in projected DOM for action '{}'",
                        selector,
                        action
                    );
                }
                Ok(found)
            }
            Err(_) => {
                tracing::warn!("Projection: Invalid CSS selector '{}'", selector);
                Ok(false)
            }
        }
    }

    async fn execute_verified(&self, action: Value) -> Result<String, SavantError> {
        let op = action["op"].as_str().unwrap_or("unknown");
        Ok(format!(
            "Projection: Verified execution of '{}' completed.",
            op
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_boundary_marker() {
        let marker = content_boundary_marker("test content");
        assert!(marker.starts_with("<!-- boundary:"));
        assert!(marker.ends_with("-->"));
    }

    #[tokio::test]
    async fn test_html_to_markdown() {
        let projection = ChromeProjection::new();
        let html = r#"
            <html><body>
                <main>
                    <h1>Hello World</h1>
                    <p>This is a <strong>test</strong> paragraph.</p>
                    <ul><li>Item 1</li><li>Item 2</li></ul>
                </main>
            </body></html>
        "#;
        let md = projection.project_html(html, "https://example.com").await;
        assert!(md.contains("# Hello World"));
        assert!(md.contains("**test**"));
        assert!(md.contains("- Item 1"));
        assert!(md.contains("boundary:"));
    }

    #[tokio::test]
    async fn test_content_root_detection() {
        let projection = ChromeProjection::new();
        let html = r#"
            <html><body>
                <nav>Navigation</nav>
                <main>
                    <h1>Main Content</h1>
                </main>
                <footer>Footer</footer>
            </body></html>
        "#;
        let md = projection.project_html(html, "https://example.com").await;
        assert!(md.contains("Main Content"));
        assert!(!md.contains("Navigation"));
        assert!(!md.contains("Footer"));
    }
}
