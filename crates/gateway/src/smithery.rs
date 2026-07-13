//! Smithery CLI Integration — manages MCP servers via @smithery/cli.
//!
//! Provides install/list/uninstall/info operations for MCP servers
//! from the Smithery marketplace. Auto-updates savant.toml config.

use savant_core::config::McpServerEntry;
use savant_core::error::SavantError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{info, warn};

/// Information about a Smithery server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmitheryServer {
    pub name: String,
    pub description: String,
    pub transport: String,
    pub url: Option<String>,
}

/// Detailed server info from Smithery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmitheryServerInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub tools: Vec<String>,
    pub transport: String,
}

/// Validates a server name to prevent injection attacks.
pub fn validate_server_name(name: &str) -> Result<(), SavantError> {
    if name.is_empty() {
        return Err(SavantError::Unknown("Server name cannot be empty".into()));
    }
    if name.len() > 128 {
        return Err(SavantError::Unknown(
            "Server name too long (max 128 chars)".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/' || c == '@' || c == '.')
    {
        return Err(SavantError::Unknown(format!(
            "Invalid server name '{}': only alphanumeric, hyphens, underscores, slashes, @, and dots allowed",
            name
        )));
    }
    if name.contains("..") {
        return Err(SavantError::Unknown(
            "Server name cannot contain '..'".into(),
        ));
    }
    Ok(())
}

/// Manages MCP servers via Smithery CLI.
pub struct SmitheryManager {
    /// Path to smithery binary (auto-detected or configured)
    cli_path: PathBuf,
    /// Directory for Smithery server data
    servers_dir: PathBuf,
}

impl SmitheryManager {
    /// Creates a new SmitheryManager.
    /// Uses SAVANT_HOME or falls back to ./data/mcp-servers.
    pub fn new() -> Result<Self, SavantError> {
        let home = std::env::var("SAVANT_HOME")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());

        let servers_dir = std::path::PathBuf::from(home)
            .join(".savant")
            .join("mcp-servers");

        std::fs::create_dir_all(&servers_dir)
            .map_err(|e| SavantError::Unknown(format!("Failed to create servers dir: {}", e)))?;

        Ok(Self {
            cli_path: PathBuf::from("npx"),
            servers_dir,
        })
    }

    /// Runs a smithery CLI command and returns stdout.
    async fn run_cli(&self, args: &[&str]) -> Result<String, SavantError> {
        let output = Command::new(&self.cli_path)
            .arg("@smithery/cli@latest")
            .args(args)
            .output()
            .await
            .map_err(|e| SavantError::Unknown(format!("Failed to run smithery CLI: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SavantError::Unknown(format!(
                "Smithery CLI failed (exit {}): {}",
                output.status,
                stderr.trim()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Installs an MCP server from the Smithery marketplace.
    /// Returns the server name and discovered transport URL.
    pub async fn install(&self, server_name: &str) -> Result<SmitheryServer, SavantError> {
        validate_server_name(server_name)?;
        info!("Installing Smithery server: {}", server_name);

        let output = self
            .run_cli(&[
                "install",
                server_name,
                "--output-dir",
                &self.servers_dir.to_string_lossy(),
            ])
            .await?;

        info!("Smithery install output: {}", output.trim());

        // Parse install output to get server details
        // Smithery outputs JSON with server info
        let server: SmitheryServer = serde_json::from_str(&output).unwrap_or_else(|_| {
            // Fallback: create server entry from name
            SmitheryServer {
                name: server_name.to_string(),
                description: format!("Installed from Smithery: {}", server_name),
                transport: "stdio".to_string(),
                url: None,
            }
        });

        Ok(server)
    }

    /// Lists installed Smithery servers.
    pub async fn list(&self) -> Result<Vec<SmitheryServer>, SavantError> {
        let output = self.run_cli(&["list", "--json"]).await?;

        let servers: Vec<SmitheryServer> = serde_json::from_str(&output).unwrap_or_else(|e| {
            warn!("Failed to parse smithery list output: {}", e);
            vec![]
        });

        Ok(servers)
    }

    /// Uninstalls an MCP server.
    pub async fn uninstall(&self, server_name: &str) -> Result<(), SavantError> {
        validate_server_name(server_name)?;
        info!("Uninstalling Smithery server: {}", server_name);

        self.run_cli(&["uninstall", server_name]).await?;
        Ok(())
    }

    /// Gets detailed info about a Smithery server.
    pub async fn info(&self, server_name: &str) -> Result<SmitheryServerInfo, SavantError> {
        validate_server_name(server_name)?;

        let output = self.run_cli(&["info", server_name, "--json"]).await?;

        let info: SmitheryServerInfo = serde_json::from_str(&output)
            .map_err(|e| SavantError::Unknown(format!("Failed to parse server info: {}", e)))?;

        Ok(info)
    }

    /// Converts a Smithery server into an McpServerEntry for savant.toml.
    pub fn to_mcp_entry(server: &SmitheryServer) -> McpServerEntry {
        McpServerEntry {
            name: server.name.clone(),
            url: server.url.clone().unwrap_or_else(|| {
                format!("ws://localhost:3001/mcp/{}", server.name.replace('/', "-"))
            }),
            auth_token: None,
        }
    }

    /// Returns the servers directory path.
    pub fn servers_dir(&self) -> &PathBuf {
        &self.servers_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_server_name_valid() {
        assert!(validate_server_name("@anthropic/mcp-server").is_ok());
        assert!(validate_server_name("filesystem-server").is_ok());
        assert!(validate_server_name("my_server").is_ok());
    }

    #[test]
    fn test_validate_server_name_invalid() {
        assert!(validate_server_name("").is_err());
        assert!(validate_server_name("../etc/passwd").is_err());
        assert!(validate_server_name("name; rm -rf /").is_err());
        assert!(validate_server_name(&"x".repeat(200)).is_err());
    }

    #[test]
    fn test_to_mcp_entry() {
        let server = SmitheryServer {
            name: "test-server".to_string(),
            description: "A test server".to_string(),
            transport: "stdio".to_string(),
            url: Some("ws://localhost:4000/mcp".to_string()),
        };
        let entry = SmitheryManager::to_mcp_entry(&server);
        assert_eq!(entry.name, "test-server");
        assert_eq!(entry.url, "ws://localhost:4000/mcp");
    }
}
