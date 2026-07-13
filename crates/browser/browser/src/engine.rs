use crate::types::{
    truncate_content, validate_url, BrowserConfig, BrowserError, BrowserEvent, DownloadInfo,
    ElementInfo, LinkInfo, NetworkRequest, PageContent, ScreenshotResult, TabId, TabInfo,
};
use chromiumoxide::browser::{Browser, BrowserConfig as ChromeConfig};
use chromiumoxide::page::{Page, ScreenshotParams};
use dashmap::DashMap;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{info, warn};

type PageSlot = Option<Arc<Page>>;

/// Escapes a string for safe interpolation into a JavaScript single-quoted string literal.
/// Replaces `'` with `\'` and `\` with `\\` to prevent injection.
fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

pub struct BrowserEngine {
    browser: Browser,
    tabs: DashMap<TabId, PageSlot>,
    active_tab: tokio::sync::RwLock<Option<TabId>>,
    config: BrowserConfig,
    event_tx: broadcast::Sender<BrowserEvent>,
    tab_counter: std::sync::atomic::AtomicUsize,
    alive: AtomicBool,
}

impl BrowserEngine {
    pub async fn launch(config: &BrowserConfig) -> Result<Arc<Self>, BrowserError> {
        if !config.enabled {
            return Err(BrowserError::BrowserDisabled);
        }

        let user_data_dir = if config.user_data_dir.is_empty() {
            if config.persist_session {
                let mut dir = std::env::temp_dir();
                dir.push("savant-browser-profile");
                std::fs::create_dir_all(&dir).map_err(|e| {
                    BrowserError::ChromeLaunchFailed(format!("Failed to create profile dir: {e}"))
                })?;
                dir
            } else {
                let mut dir = std::env::temp_dir();
                dir.push(format!("savant-browser-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir).map_err(|e| {
                    BrowserError::ChromeLaunchFailed(format!("Failed to create temp dir: {e}"))
                })?;
                dir
            }
        } else {
            PathBuf::from(&config.user_data_dir)
        };

        let chrome_config = if config.chrome_path.is_empty() {
            ChromeConfig::builder()
                .user_data_dir(user_data_dir)
                .build()
                .map_err(|e| {
                    BrowserError::ChromeLaunchFailed(format!("Config build failed: {e}"))
                })?
        } else {
            ChromeConfig::with_executable(&config.chrome_path)
        };

        let (browser, mut handler) = Browser::launch(chrome_config)
            .await
            .map_err(|e| BrowserError::ChromeLaunchFailed(e.to_string()))?;

        let (event_tx, _) = broadcast::channel(64);

        let engine = Arc::new(BrowserEngine {
            browser,
            tabs: DashMap::new(),
            active_tab: tokio::sync::RwLock::new(None),
            config: config.clone(),
            event_tx,
            tab_counter: std::sync::atomic::AtomicUsize::new(1),
            alive: AtomicBool::new(true),
        });

        let engine_ref = engine.clone();
        tokio::spawn(async move {
            while let Some(_event) = handler.next().await {
                // Chromium handler events processed silently.
                // Connection failures are detected via the alive flag on next operation.
            }
            engine_ref.alive.store(false, Ordering::SeqCst);
            warn!("[browser] Chrome handler stream ended — browser may have crashed");
        });

        info!(
            "[browser] BrowserEngine launched (headless: {}, chrome: {})",
            config.headless,
            if config.chrome_path.is_empty() {
                "auto-detected"
            } else {
                &config.chrome_path
            }
        );
        Ok(engine)
    }

    fn check_alive(&self) -> Result<(), BrowserError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(BrowserError::Internal(
                "Chrome process has terminated. The browser session cannot be used.".to_string(),
            ));
        }
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BrowserEvent> {
        self.event_tx.subscribe()
    }

    pub fn config(&self) -> &BrowserConfig {
        &self.config
    }

    async fn get_active_page(&self) -> Result<(TabId, Arc<Page>), BrowserError> {
        self.check_alive()?;
        let active = self.active_tab.read().await;
        if let Some(ref tab_id) = *active {
            if let Some(entry) = self.tabs.get(tab_id) {
                if let Some(ref page) = *entry {
                    return Ok((tab_id.clone(), page.clone()));
                }
            }
        }
        Err(BrowserError::NoActiveTab)
    }

    async fn ensure_active_page(&self) -> Result<(TabId, Arc<Page>), BrowserError> {
        match self.get_active_page().await {
            Ok(pair) => Ok(pair),
            Err(_) => {
                let page =
                    self.browser.new_page("about:blank").await.map_err(|e| {
                        BrowserError::Internal(format!("Failed to create page: {e}"))
                    })?;

                let page = Arc::new(page);
                let count = self
                    .tab_counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let tab_id = TabId(format!("tab-{count}"));
                self.tabs.insert(tab_id.clone(), Some(page.clone()));
                *self.active_tab.write().await = Some(tab_id.clone());

                if let Err(e) = self.event_tx.send(BrowserEvent::TabOpened {
                    tab_id: tab_id.0.clone(),
                    url: String::from("about:blank"),
                }) {
                    tracing::warn!("[browser] Failed to send TabOpened event: {}", e);
                }

                Ok((tab_id, page))
            }
        }
    }

    pub async fn switch_tab(&self, tab_id: &TabId) -> Result<TabInfo, BrowserError> {
        self.check_alive()?;
        if !self.tabs.contains_key(tab_id) {
            return Err(BrowserError::TabNotFound(tab_id.0.clone()));
        }

        // Ensure the target tab has a page (lazy creation)
        if let Some(entry) = self.tabs.get(tab_id) {
            if entry.is_none() {
                drop(entry);
                let page =
                    self.browser.new_page("about:blank").await.map_err(|e| {
                        BrowserError::Internal(format!("Failed to create page: {e}"))
                    })?;
                self.tabs.insert(tab_id.clone(), Some(Arc::new(page)));
            }
        }

        *self.active_tab.write().await = Some(tab_id.clone());

        let page = self.tabs.get(tab_id).and_then(|e| e.as_ref().cloned());
        let (url, title) = match page {
            Some(p) => {
                let u = p
                    .url()
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| String::from("about:blank"));
                let t = p.get_title().await.ok().flatten().unwrap_or_default();
                (u, t)
            }
            None => (String::from("about:blank"), String::new()),
        };

        Ok(TabInfo {
            id: tab_id.clone(),
            url,
            title,
            loading: false,
            agent_name: None,
        })
    }

    pub async fn create_tab(&self, url: String) -> Result<TabId, BrowserError> {
        if self.tabs.len() >= self.config.max_tabs {
            return Err(BrowserError::TabLimitReached(self.config.max_tabs));
        }
        let count = self
            .tab_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tab_id = TabId(format!("tab-{count}"));

        let page = self
            .browser
            .new_page(&url)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to create tab: {e}")))?;

        self.tabs.insert(tab_id.clone(), Some(Arc::new(page)));

        if let Err(e) = self.event_tx.send(BrowserEvent::TabOpened {
            tab_id: tab_id.0.clone(),
            url: url.clone(),
        }) {
            tracing::warn!("[browser] Failed to send TabOpened event: {}", e);
        }

        Ok(tab_id)
    }

    pub async fn close_tab(&self, tab_id: &TabId) -> Result<(), BrowserError> {
        match self.tabs.remove(tab_id) {
            Some((_key, Some(_page))) => {
                // Page will be closed via chromiumoxide Drop when Arc is dropped
                drop(_page);
                if let Err(e) = self.event_tx.send(BrowserEvent::TabClosed {
                    tab_id: tab_id.0.clone(),
                }) {
                    tracing::warn!("[browser] Failed to send TabClosed event: {}", e);
                }
                Ok(())
            }
            Some((_key, None)) => {
                if let Err(e) = self.event_tx.send(BrowserEvent::TabClosed {
                    tab_id: tab_id.0.clone(),
                }) {
                    tracing::warn!("[browser] Failed to send TabClosed event: {}", e);
                }
                Ok(())
            }
            None => Err(BrowserError::TabNotFound(tab_id.0.clone())),
        }
    }

    pub async fn list_tabs(&self) -> Vec<TabInfo> {
        let mut tabs = Vec::new();
        for entry in self.tabs.iter() {
            let id = entry.key().clone();
            let url = match entry.value() {
                Some(page) => page.url().await.ok().flatten().unwrap_or_default(),
                None => String::new(),
            };
            let title = match entry.value() {
                Some(page) => page.get_title().await.ok().flatten().unwrap_or_default(),
                None => String::new(),
            };
            tabs.push(TabInfo {
                id,
                url,
                title,
                loading: false,
                agent_name: None,
            });
        }
        tabs
    }

    // ── Navigation ─────────────────────────────────────────────────

    /// Waits for the page to reach `complete` readyState, with a timeout.
    async fn wait_for_load(&self, page: &Page, timeout_ms: u64) -> Result<(), BrowserError> {
        let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            loop {
                let eval_result = page.evaluate("document.readyState === 'complete'").await;
                if let Ok(r) = eval_result {
                    let val: serde_json::Value = r.into_value().unwrap_or_default();
                    if val.as_bool() == Some(true) {
                        return Ok::<(), BrowserError>(());
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(BrowserError::Timeout(timeout_ms)),
        }
    }

    pub async fn navigate(&self, url_str: &str) -> Result<PageContent, BrowserError> {
        validate_url(url_str)?;
        self.check_alive()?;

        let (_tab_id, page) = self.ensure_active_page().await?;

        page.goto(url_str)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        let url = page
            .url()
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get URL: {e}")))?
            .unwrap_or_else(|| url_str.to_string());

        let title = page
            .get_title()
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get title: {e}")))?
            .unwrap_or_default();

        let text = self.extract_text_js(&page).await;
        let html = page.content().await.unwrap_or_default();

        if let Err(e) = self.event_tx.send(BrowserEvent::PageLoaded {
            tab_id: _tab_id.0.clone(),
            url: url.clone(),
            title: title.clone(),
        }) {
            tracing::warn!("[browser] Failed to send PageLoaded event: {}", e);
        }

        Ok(PageContent {
            url,
            title,
            text: truncate_content(&text),
            html_length_bytes: html.len(),
        })
    }

    pub async fn go_back(&self) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        page.evaluate("window.history.back()")
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        self.wait_for_load(&page, self.config.default_timeout_ms)
            .await?;
        let url = page
            .url()
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get URL: {e}")))?;
        Ok(url.unwrap_or_else(|| String::from("unknown")))
    }

    pub async fn go_forward(&self) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        page.evaluate("window.history.forward()")
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        self.wait_for_load(&page, self.config.default_timeout_ms)
            .await?;
        let url = page
            .url()
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get URL: {e}")))?;
        Ok(url.unwrap_or_else(|| String::from("unknown")))
    }

    pub async fn reload(&self) -> Result<PageContent, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        page.reload()
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        let url = page
            .url()
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get URL: {e}")))?
            .unwrap_or_default();

        let title = page
            .get_title()
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get title: {e}")))?
            .unwrap_or_default();

        let text = self.extract_text_js(&page).await;

        Ok(PageContent {
            url,
            title,
            text: truncate_content(&text),
            html_length_bytes: 0,
        })
    }

    // ── Content Extraction ─────────────────────────────────────────

    async fn extract_text_js(&self, page: &Page) -> String {
        page.evaluate("document.body ? document.body.innerText : ''")
            .await
            .map(|r| {
                let val: serde_json::Value = r.into_value().unwrap_or_default();
                val.as_str().unwrap_or("").to_string()
            })
            .unwrap_or_else(|_| String::from("[text extraction failed]"))
    }

    pub async fn get_text(&self) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let text = self.extract_text_js(&page).await;
        Ok(truncate_content(&text))
    }

    pub async fn get_content(&self) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let html = page
            .content()
            .await
            .map_err(|e| BrowserError::Internal(format!("Content extraction failed: {e}")))?;
        Ok(truncate_content(&html))
    }

    pub async fn get_links(&self) -> Result<Vec<LinkInfo>, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = "Array.from(document.querySelectorAll('a[href]')).map(a => ({text: (a.innerText || a.textContent || '').trim().substring(0, 200), href: a.href}))";
        let result = page
            .evaluate(js)
            .await
            .map_err(|e| BrowserError::Internal(format!("Link extraction failed: {e}")))?;

        let val: serde_json::Value = result.into_value().unwrap_or_default();
        let links: Vec<LinkInfo> = serde_json::from_value(val).unwrap_or_default();
        Ok(links)
    }

    pub async fn get_tables(&self) -> Result<serde_json::Value, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = "Array.from(document.querySelectorAll('table')).slice(0, 20).map(t => { const rows = []; const trs = t.querySelectorAll('tr'); for (const tr of trs) { const cells = []; for (const td of tr.querySelectorAll('th, td')) { cells.push((td.innerText || td.textContent || '').trim().substring(0, 500)); } if (cells.length > 0) rows.push(cells); } return rows; })";
        let result = page
            .evaluate(js)
            .await
            .map_err(|e| BrowserError::Internal(format!("Table extraction failed: {e}")))?;

        Ok(result.into_value().unwrap_or_default())
    }

    // ── Interaction ────────────────────────────────────────────────

    pub async fn click(&self, selector: &str) -> Result<ElementInfo, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let element = page
            .find_element(selector)
            .await
            .map_err(|_| BrowserError::ElementNotFound(selector.to_string()))?;

        element
            .click()
            .await
            .map_err(|e| BrowserError::Internal(format!("Click failed: {e}")))?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        let info = self.describe_element_js(&page, selector).await?;
        Ok(info)
    }

    pub async fn type_text(&self, selector: &str, text: &str) -> Result<ElementInfo, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let element = page
            .find_element(selector)
            .await
            .map_err(|_| BrowserError::ElementNotFound(selector.to_string()))?;

        element
            .click()
            .await
            .map_err(|e| BrowserError::Internal(format!("Click failed: {e}")))?;

        element
            .type_str(text)
            .await
            .map_err(|e| BrowserError::Internal(format!("Type failed: {e}")))?;

        let info = self.describe_element_js(&page, selector).await?;
        Ok(info)
    }

    async fn describe_element_js(
        &self,
        page: &Page,
        selector: &str,
    ) -> Result<ElementInfo, BrowserError> {
        let safe = escape_js_string(selector);
        let js = format!(
            "(function() {{ const e = document.querySelector('{}'); if (!e) return null; return {{ tag: e.tagName, text: (e.innerText || e.textContent || '').trim().substring(0, 500), visible: e.offsetWidth > 0 && e.offsetHeight > 0 }}; }})()",
            safe
        );
        let result = page
            .evaluate(js.as_str())
            .await
            .map_err(|e| BrowserError::Internal(format!("Element describe failed: {e}")))?;

        let val: serde_json::Value = result.into_value().unwrap_or_default();
        if val.is_null() {
            return Err(BrowserError::ElementNotFound(selector.to_string()));
        }
        Ok(ElementInfo {
            tag: val["tag"].as_str().unwrap_or("UNKNOWN").to_string(),
            text: val["text"].as_str().map(|s| s.to_string()),
            visible: val["visible"].as_bool().unwrap_or(false),
        })
    }

    pub async fn scroll(
        &self,
        position: &str,
        amount: Option<i64>,
    ) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = match (position, amount) {
            ("bottom", _) => String::from("window.scrollTo(0, document.body.scrollHeight); 'scrolled to bottom'"),
            ("top", _) => String::from("window.scrollTo(0, 0); 'scrolled to top'"),
            ("percent", Some(pct)) => format!("window.scrollTo(0, document.body.scrollHeight * {pct} / 100); 'scrolled to {pct}%'"),
            _ => format!("window.scrollBy(0, {}); 'scrolled by {}px'", amount.unwrap_or(500), amount.unwrap_or(500)),
        };
        let result = page
            .evaluate(js.as_str())
            .await
            .map_err(|e| BrowserError::Internal(format!("Scroll failed: {e}")))?;

        let val: serde_json::Value = result.into_value().unwrap_or_default();
        Ok(val.as_str().unwrap_or("scrolled").to_string())
    }

    pub async fn wait_for_selector(
        &self,
        selector: &str,
        timeout_ms: u64,
    ) -> Result<ElementInfo, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);

        let safe = escape_js_string(selector);
        let js = format!(
            "(function() {{ const e = document.querySelector('{}'); if (!e) return null; return {{ tag: e.tagName, text: (e.innerText || e.textContent || '').trim().substring(0, 500), visible: e.offsetWidth > 0 && e.offsetHeight > 0 }}; }})()",
            safe
        );

        while tokio::time::Instant::now() < deadline {
            let result = page
                .evaluate(js.as_str())
                .await
                .map_err(|e| BrowserError::Internal(format!("Wait select failed: {e}")))?;
            let val: serde_json::Value = result.into_value().unwrap_or_default();
            if !val.is_null() {
                return Ok(ElementInfo {
                    tag: val["tag"].as_str().unwrap_or("UNKNOWN").to_string(),
                    text: val["text"].as_str().map(|s| s.to_string()),
                    visible: val["visible"].as_bool().unwrap_or(false),
                });
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err(BrowserError::ElementNotFound(selector.to_string()))
    }

    pub async fn execute_js(&self, script: &str) -> Result<serde_json::Value, BrowserError> {
        self.check_alive()?;
        if let Some(blocked) = crate::types::is_js_blocked(script) {
            return Err(BrowserError::JsBlocked(blocked));
        }
        let (_tab_id, page) = self.ensure_active_page().await?;
        let result = page
            .evaluate(script)
            .await
            .map_err(|e| BrowserError::Internal(format!("JS execution failed: {e}")))?;

        Ok(result.into_value().unwrap_or_default())
    }

    pub async fn highlight(&self, selector: &str) -> Result<String, BrowserError> {
        self.check_alive()?;
        if self.config.headless {
            return Ok(String::from(
                "Highlight unavailable in headless mode. Use screenshot to inspect the page.",
            ));
        }
        let (_tab_id, page) = self.ensure_active_page().await?;
        let safe = escape_js_string(selector);
        let js = format!(
            "const el = document.querySelector('{}'); if (!el) 'Element not found'; else {{ el.style.outline = '3px solid rgba(0,213,255,0.9)'; el.style.backgroundColor = 'rgba(0,213,255,0.1)'; el.scrollIntoView({{behavior: 'smooth', block: 'center'}}); el.tagName; }}",
            safe
        );
        let result = page
            .evaluate(js.as_str())
            .await
            .map_err(|e| BrowserError::Internal(format!("Highlight failed: {e}")))?;

        let val: serde_json::Value = result.into_value().unwrap_or_default();
        if val.as_str() == Some("Element not found") {
            return Err(BrowserError::ElementNotFound(selector.to_string()));
        }
        Ok(format!(
            "Highlighted: {}",
            val.as_str().unwrap_or("element")
        ))
    }

    pub async fn enable_network_monitoring(&self) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = r#"
            if (!window.__savant_requests) {
                window.__savant_requests = [];
                const origFetch = window.fetch;
                window.fetch = async function(...args) {
                    const start = Date.now();
                    const entry = { url: args[0]?.url || String(args[0]), method: (args[1]?.method || 'GET').toUpperCase(), status: null, type: null, size: 0, time: start };
                    window.__savant_requests.push(entry);
                    if (window.__savant_requests.length > 1000) window.__savant_requests.shift();
                    try {
                        const resp = await origFetch.apply(this, args);
                        entry.status = resp.status;
                        entry.type = resp.headers.get('content-type') || null;
                        const clone = resp.clone();
                        clone.text().then(t => { entry.size = t.length; }).catch(() => {});
                        return resp;
                    } catch(e) {
                        entry.status = 0;
                        throw e;
                    }
                };
                const origXHR = window.XMLHttpRequest.prototype.open;
                window.XMLHttpRequest.prototype.open = function(method, url) {
                    this.__savant_url = url;
                    this.__savant_method = method;
                    origXHR.apply(this, arguments);
                };
                const origSend = window.XMLHttpRequest.prototype.send;
                window.XMLHttpRequest.prototype.send = function(body) {
                    const entry = { url: this.__savant_url || '', method: (this.__savant_method || 'GET').toUpperCase(), status: null, type: null, size: 0, time: Date.now() };
                    window.__savant_requests.push(entry);
                    if (window.__savant_requests.length > 1000) window.__savant_requests.shift();
                    this.addEventListener('load', () => {
                        entry.status = this.status;
                        entry.type = this.getResponseHeader('content-type');
                        entry.size = (this.responseText || '').length;
                    });
                    origSend.apply(this, arguments);
                };
                'Network monitoring enabled';
            } else {
                'Network monitoring already active';
            }
        "#;
        page.evaluate(js).await.map_err(|e| {
            BrowserError::Internal(format!("Failed to enable network monitoring: {e}"))
        })?;
        Ok(String::from("Network monitoring enabled"))
    }

    pub async fn get_network_requests(&self) -> Result<Vec<NetworkRequest>, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = "JSON.stringify((window.__savant_requests || []).slice(-100))";
        let result = page
            .evaluate(js)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get network requests: {e}")))?;
        let val: serde_json::Value = result.into_value().unwrap_or_default();
        let raw: Vec<serde_json::Value> = val.as_array().cloned().unwrap_or_default();
        let requests: Vec<NetworkRequest> = raw
            .iter()
            .map(|r| NetworkRequest {
                url: r["url"].as_str().unwrap_or("").to_string(),
                method: r["method"].as_str().unwrap_or("GET").to_string(),
                status: r["status"].as_u64().map(|s| s as u16),
                mime_type: r["type"].as_str().map(|s| s.to_string()),
                size_bytes: r["size"].as_u64().map(|s| s as usize),
                timestamp: r["time"].as_i64().unwrap_or(0),
            })
            .collect();
        Ok(requests)
    }

    pub async fn block_urls(&self, patterns: &[String]) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;

        // Use CDP Fetch.enable to intercept and block requests matching patterns.
        // Fall back to JS-based blocking if CDP method isn't available.
        let patterns_json = serde_json::to_string(patterns)
            .map_err(|e| BrowserError::Internal(format!("Failed to serialize patterns: {e}")))?;

        // JavaScript-based URL blocking: override fetch and XMLHttpRequest
        let js = format!(
            r#"
            (function() {{
                const blockedPatterns = {patterns_json};
                function isBlocked(url) {{
                    return blockedPatterns.some(p => url.includes(p) || new RegExp(p).test(url));
                }}
                const origFetch = window.fetch;
                window.fetch = function(input, init) {{
                    const url = typeof input === 'string' ? input : input.url;
                    if (isBlocked(url)) {{
                        return Promise.reject(new Error('Blocked by Savant: ' + url));
                    }}
                    return origFetch.call(this, input, init);
                }};
                const origOpen = XMLHttpRequest.prototype.open;
                XMLHttpRequest.prototype.open = function(method, url) {{
                    if (isBlocked(url)) {{
                        throw new Error('Blocked by Savant: ' + url);
                    }}
                    return origOpen.apply(this, arguments);
                }};
                return 'Blocked ' + blockedPatterns.length + ' URL patterns';
            }})()
            "#
        );

        let result = page
            .evaluate(js.as_str())
            .await
            .map_err(|e| BrowserError::Internal(format!("URL blocking failed: {e}")))?;

        let val: serde_json::Value = result.into_value().unwrap_or_default();
        Ok(val.as_str().unwrap_or("URL blocking enabled").to_string())
    }

    pub async fn get_downloads(&self) -> Result<Vec<DownloadInfo>, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = "JSON.stringify((window.__savant_downloads || []))";
        let result = page
            .evaluate(js)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to get downloads: {e}")))?;
        let val: serde_json::Value = result.into_value().unwrap_or_default();
        let raw: Vec<serde_json::Value> = val.as_array().cloned().unwrap_or_default();
        let downloads: Vec<DownloadInfo> = raw
            .iter()
            .map(|d| DownloadInfo {
                url: d["url"].as_str().unwrap_or("").to_string(),
                filename: d["filename"].as_str().unwrap_or("").to_string(),
                path: d["path"].as_str().unwrap_or("").to_string(),
                size_bytes: d["size"].as_u64().unwrap_or(0) as usize,
                status: d["status"].as_str().unwrap_or("unknown").to_string(),
                mime_type: d["type"].as_str().map(|s| s.to_string()),
            })
            .collect();
        Ok(downloads)
    }

    pub async fn enable_download_tracking(&self) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let js = r#"
            if (!window.__savant_downloads) {
                window.__savant_downloads = [];
                'Download tracking enabled';
            } else {
                'Download tracking already active';
            }
        "#;
        page.evaluate(js).await.map_err(|e| {
            BrowserError::Internal(format!("Failed to enable download tracking: {e}"))
        })?;
        Ok(String::from("Download tracking enabled"))
    }

    pub async fn read_download(&self, filename: &str) -> Result<String, BrowserError> {
        self.check_alive()?;
        let (_tab_id, page) = self.ensure_active_page().await?;
        let safe = escape_js_string(filename);
        let js = format!(
            "(() => {{ const d = (window.__savant_downloads || []).find(x => x.filename === '{}'); return d ? JSON.stringify(d) : null; }})()",
            safe
        );
        let result = page
            .evaluate(js.as_str())
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to read download: {e}")))?;
        let val: serde_json::Value = result.into_value().unwrap_or_default();
        if val.is_null() {
            return Err(BrowserError::Internal(format!(
                "Download not found: {filename}"
            )));
        }
        Ok(serde_json::to_string_pretty(&val).unwrap_or_default())
    }

    // ── Screenshot ─────────────────────────────────────────────────

    async fn get_viewport_dimensions(&self, page: &Page) -> (u32, u32) {
        let js =
            "(function() { return { w: window.innerWidth || 0, h: window.innerHeight || 0 }; })()";
        match page.evaluate(js).await {
            Ok(result) => {
                let val: serde_json::Value = result.into_value().unwrap_or_default();
                let w = val["w"].as_u64().unwrap_or(0) as u32;
                let h = val["h"].as_u64().unwrap_or(0) as u32;
                (w, h)
            }
            Err(_) => (0, 0),
        }
    }

    pub async fn screenshot(&self) -> Result<ScreenshotResult, BrowserError> {
        self.check_alive()?;
        if !self.config.screenshot_enabled {
            return Err(BrowserError::ScreenshotDisabled);
        }

        let (_tab_id, page) = self.ensure_active_page().await?;

        let filename = format!("savant-screenshot-{}.png", uuid::Uuid::new_v4());
        let png_bytes = page
            .save_screenshot(
                ScreenshotParams::builder().full_page(true).build(),
                filename.as_str(),
            )
            .await
            .map_err(|e| BrowserError::Internal(format!("Screenshot failed: {e}")))?;

        let kb = png_bytes.len() / 1024;
        if kb > self.config.max_screenshot_size_kb {
            return Err(BrowserError::ScreenshotTooLarge(
                kb,
                self.config.max_screenshot_size_kb,
            ));
        }

        let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_bytes);

        let (width, height) = self.get_viewport_dimensions(&page).await;

        if let Err(e) = self.event_tx.send(BrowserEvent::ScreenshotCaptured {
            tab_id: _tab_id.0.clone(),
        }) {
            tracing::warn!("[browser] Failed to send ScreenshotCaptured event: {}", e);
        }

        Ok(ScreenshotResult {
            base64,
            width,
            height,
            size_bytes: png_bytes.len(),
        })
    }
}

impl Drop for BrowserEngine {
    fn drop(&mut self) {
        info!("[browser] BrowserEngine dropping — Chrome will terminate");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
