pub mod dns_interceptor;
pub mod smoltcp_stack;
pub mod ssrf;
pub mod tls_proxy;

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Hot-reloadable network policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkPolicy {
    /// Allowed domains (suffix match). Empty = all domains allowed.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Explicitly allowed IPs.
    #[serde(default)]
    pub allowed_ips: Vec<IpAddr>,
    /// Explicitly blocked IPs.
    #[serde(default)]
    pub blocked_ips: Vec<IpAddr>,
    /// Max bandwidth in bytes per second (0 = unlimited).
    #[serde(default)]
    pub max_bandwidth_bytes_per_sec: u64,
    /// DNS cache TTL in seconds.
    #[serde(default = "default_dns_ttl")]
    pub dns_ttl_secs: u64,
}

fn default_dns_ttl() -> u64 {
    300
}

/// Watches a YAML policy file and hot-reloads on change.
pub struct PolicyReloader {
    policy: Arc<RwLock<NetworkPolicy>>,
    config_path: PathBuf,
}

impl PolicyReloader {
    /// Creates a new policy reloader. Loads the initial policy from disk.
    pub fn new(config_path: PathBuf) -> Result<Self, NetError> {
        let policy = Self::load_policy(&config_path)?;
        Ok(Self {
            policy: Arc::new(RwLock::new(policy)),
            config_path,
        })
    }

    /// Returns a reference to the current policy.
    pub fn policy(&self) -> Arc<RwLock<NetworkPolicy>> {
        Arc::clone(&self.policy)
    }

    /// Loads a NetworkPolicy from a YAML file.
    fn load_policy(path: &std::path::Path) -> Result<NetworkPolicy, NetError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| NetError::InvalidConfig(format!("Failed to read policy file: {}", e)))?;
        serde_yaml::from_str(&contents)
            .map_err(|e| NetError::InvalidConfig(format!("Failed to parse policy YAML: {}", e)))
    }

    /// Starts a background task that watches the policy file for changes.
    /// Reloads the policy on any modify event.
    pub async fn start_watching(self: Arc<Self>) -> Result<(), NetError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let config_path = self.config_path.clone();

        // Spawn file watcher in a blocking thread
        // SAN-08: Return errors instead of panicking
        std::thread::spawn(move || {
            use notify::{Event, RecursiveMode, Watcher};
            let mut watcher = match notify::recommended_watcher(move |res: Result<Event, _>| {
                if let Ok(event) = res {
                    if event.kind.is_modify() {
                        let _ = tx.blocking_send(());
                    }
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("[policy] Failed to create file watcher: {}", e);
                    return;
                }
            };

            if let Err(e) = watcher.watch(&config_path, RecursiveMode::NonRecursive) {
                tracing::error!("[policy] Failed to watch policy file: {}", e);
                return;
            }

            // Keep thread alive
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        });

        // Handle reload events
        let policy = Arc::clone(&self.policy);
        let path = self.config_path.clone();
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                match Self::load_policy(&path) {
                    Ok(new_policy) => {
                        let mut p = policy.write().await;
                        *p = new_policy;
                        tracing::info!("[policy] Network policy reloaded from {:?}", path);
                    }
                    Err(e) => {
                        tracing::warn!("[policy] Failed to reload policy: {}", e);
                    }
                }
            }
        });

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("platform not supported: {0}")]
    UnsupportedPlatform(String),
    #[error("packet dropped: {0}")]
    PacketDropped(String),
    #[error("DNS error: {0}")]
    Dns(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("configuration invalid: {0}")]
    InvalidConfig(String),
    #[error("network access denied: {0}")]
    AccessDenied(String),
}

/// A time-bounded network access token. The agent must request a token
/// before making any network connection. The token expires after `duration_secs`.
#[derive(Debug, Clone)]
pub struct NetworkToken {
    /// The domain this token grants access to.
    pub domain: String,
    /// Token creation time (epoch seconds).
    pub created_at: u64,
    /// Token duration in seconds.
    pub duration_secs: u64,
    /// The resolved IP address for this domain.
    pub resolved_ip: std::net::IpAddr,
}

impl NetworkToken {
    pub fn new(domain: String, resolved_ip: std::net::IpAddr, duration_secs: u64) -> Self {
        Self {
            domain,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_secs,
            resolved_ip,
        }
    }

    /// Returns `true` if this token has expired.
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now > self.created_at + self.duration_secs
    }

    /// Returns `true` if this token grants access to the given IP.
    pub fn allows(&self, ip: &std::net::IpAddr) -> bool {
        !self.is_expired() && self.resolved_ip == *ip
    }
}
