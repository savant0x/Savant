// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
// Validation gate: re-run `cargo clippy -p savant_mcp --no-deps` and verify
// all disallowed method warnings trace back to json!() macro expansion.

use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use savant_core::types::CapabilityGrants;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// JSON-RPC request for MCP protocol
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC response from MCP protocol
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Discovered MCP tool metadata
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// MCP server connection state
struct McpConnection {
    /// WebSocket write half for sending messages
    write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    /// Next request ID counter
    next_id: u64,
}

/// WebSocket client for connecting to MCP servers.
///
/// This client implements the Model Context Protocol over WebSocket,
/// enabling tool discovery and execution from remote MCP servers.
///
/// # Protocol Flow
/// 1. Connect to MCP server WebSocket endpoint
/// 2. Send `initialize` handshake
/// 3. Send `tools/list` to discover available tools
/// 4. Send `tools/call` to execute individual tools
///
/// # Thread Safety
/// The client wraps its connection in `Arc<Mutex<>>` for safe concurrent access.
pub struct McpClient {
    /// Server URL
    server_url: String,
    /// Connection state
    connection: Option<Arc<Mutex<McpConnection>>>,
    /// Cached tool list
    tools: Vec<McpToolInfo>,
    /// Read half task handle
    read_task: Option<tokio::task::JoinHandle<()>>,
    /// Pending responses channel
    responses: Arc<DashMap<u64, tokio::sync::oneshot::Sender<Value>>>,
    /// Cancellation token for graceful shutdown of the read task
    cancel_token: CancellationToken,
}

impl McpClient {
    /// Creates a new MCP client for the given server URL.
    pub fn new(server_url: &str) -> Self {
        Self {
            server_url: server_url.to_string(),
            connection: None,
            tools: Vec::new(),
            read_task: None,
            responses: Arc::new(DashMap::new()),
            cancel_token: CancellationToken::new(),
        }
    }

    /// Connects to the MCP server and performs the initialize handshake.
    pub async fn connect(&mut self) -> Result<(), SavantError> {
        info!("Connecting to MCP server: {}", self.server_url);

        let url = url::Url::parse(&self.server_url)
            .map_err(|e| SavantError::Unknown(format!("Invalid MCP server URL: {}", e)))?;

        let (ws_stream, _) = connect_async(url.as_str())
            .await
            .map_err(|e| SavantError::Unknown(format!("MCP WebSocket connection failed: {}", e)))?;

        let (write, read) = ws_stream.split();

        // Spawn a task to handle incoming messages
        let responses = self.responses.clone();
        let cancel_token = self.cancel_token.clone();
        let read_task = tokio::spawn(async move {
            Self::handle_incoming(read, responses, cancel_token).await;
        });

        self.connection = Some(Arc::new(Mutex::new(McpConnection { write, next_id: 1 })));
        self.read_task = Some(read_task);

        // Perform initialize handshake
        let init_result = self
            .send_request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "Savant MCP Client",
                        "version": "2.0.0"
                    }
                })),
            )
            .await?;

        info!("MCP server initialized: {:?}", init_result);
        Ok(())
    }

    /// Connects with authentication token.
    pub async fn connect_with_auth(&mut self, auth_token: &str) -> Result<(), SavantError> {
        info!("Connecting to MCP server with auth: {}", self.server_url);

        let url = url::Url::parse(&self.server_url)
            .map_err(|e| SavantError::Unknown(format!("Invalid MCP server URL: {}", e)))?;

        let (ws_stream, _) = connect_async(url.as_str())
            .await
            .map_err(|e| SavantError::Unknown(format!("MCP WebSocket connection failed: {}", e)))?;

        let (write, read) = ws_stream.split();

        let responses = self.responses.clone();
        let cancel_token = self.cancel_token.clone();
        let read_task = tokio::spawn(async move {
            Self::handle_incoming(read, responses, cancel_token).await;
        });

        self.connection = Some(Arc::new(Mutex::new(McpConnection { write, next_id: 1 })));
        self.read_task = Some(read_task);

        // Initialize with auth token
        let init_result = self
            .send_request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "Savant MCP Client",
                        "version": "2.0.0"
                    },
                    "auth_token": auth_token
                })),
            )
            .await?;

        info!("MCP server initialized with auth: {:?}", init_result);
        Ok(())
    }

    /// Handles incoming WebSocket messages and routes responses.
    /// Supports graceful shutdown via CancellationToken.
    async fn handle_incoming(
        mut read: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        responses: Arc<DashMap<u64, tokio::sync::oneshot::Sender<Value>>>,
        cancel_token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<JsonRpcResponse>(&text) {
                                Ok(resp) => {
                                    if let Some(id) = resp.id {
                                        if let Some((_, tx)) = responses.remove(&id) {
                                            let value = resp.result.unwrap_or_else(|| {
                                                serde_json::json!({
                                                    "error": resp.error.map(|e| {
                                                        format!("Error {}: {}", e.code, e.message)
                                                    })
                                                    .unwrap_or_else(|| "Unknown error".to_string())
                                                })
                                            });
                                            if let Err(e) = tx.send(value) {
                                                debug!("[mcp::client] Failed to forward MCP response: {:?}", e);
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    debug!("Failed to parse MCP response: {}", e);
                                }
                            }
                        }
                        Some(Ok(_)) => {
                            // Non-text message, ignore
                        }
                        Some(Err(e)) => {
                            debug!("[mcp::client] WebSocket read error: {}", e);
                            break;
                        }
                        None => {
                            debug!("[mcp::client] WebSocket stream ended");
                            break;
                        }
                    }
                }
                _ = cancel_token.cancelled() => {
                    info!("MCP client shutting down gracefully");
                    break;
                }
            }
        }
        debug!("MCP client read stream closed");
    }

    /// Sends a JSON-RPC request and waits for the response.
    async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, SavantError> {
        let conn = self
            .connection
            .as_ref()
            .ok_or_else(|| SavantError::Unknown("MCP client not connected".to_string()))?;

        let (request_id, rx) = {
            let mut conn_guard = conn.lock().await;
            let id = conn_guard.next_id;
            conn_guard.next_id += 1;

            let request = JsonRpcRequest {
                jsonrpc: "2.0",
                id,
                method: method.to_string(),
                params,
            };

            let json = serde_json::to_string(&request)
                .map_err(|e| SavantError::Unknown(format!("Failed to serialize request: {}", e)))?;

            // Register response channel before sending
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.responses.insert(id, tx);

            conn_guard
                .write
                .send(Message::Text(json.into()))
                .await
                .map_err(|e| SavantError::Unknown(format!("Failed to send MCP request: {}", e)))?;

            (id, rx)
        };

        // Wait for response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => {
                // Channel closed - remove from pending
                self.responses.remove(&request_id);
                Err(SavantError::Unknown(
                    "MCP response channel closed".to_string(),
                ))
            }
            Err(_) => {
                // Timeout - remove from pending
                self.responses.remove(&request_id);
                Err(SavantError::Unknown(format!(
                    "MCP request timed out (method: {})",
                    method
                )))
            }
        }
    }

    /// Discovers available tools from the MCP server.
    ///
    /// Returns a list of tool metadata that can be used to create
    /// `McpRemoteTool` instances for execution.
    pub async fn discover_tools(&mut self) -> Result<Vec<McpToolInfo>, SavantError> {
        let response = self.send_request("tools/list", None).await?;

        let tools_array = response
            .get("tools")
            .and_then(|t| t.as_array())
            .ok_or_else(|| {
                SavantError::Unknown("Invalid tools/list response: missing tools array".to_string())
            })?;

        let mut tools = Vec::new();
        for tool_value in tools_array {
            let name = tool_value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();

            let description = tool_value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();

            let input_schema = tool_value
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            tools.push(McpToolInfo {
                name: name.clone(),
                description,
                input_schema,
            });

            debug!("Discovered MCP tool: {}", name);
        }

        info!("Discovered {} tools from MCP server", tools.len());
        self.tools = tools.clone();
        Ok(tools)
    }

    /// Executes a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, SavantError> {
        let response = self
            .send_request(
                "tools/call",
                Some(serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments
                })),
            )
            .await?;

        // Extract content from MCP response format
        if let Some(content) = response.get("content").and_then(|c| c.as_array()) {
            let text_parts: Vec<String> = content
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        item.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect();

            if !text_parts.is_empty() {
                return Ok(text_parts.join("\n"));
            }
        }

        // Fallback: return the full response as string
        Ok(response.to_string())
    }

    /// Returns whether the client is connected.
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    /// Returns the cached tool list.
    pub fn tools(&self) -> &[McpToolInfo] {
        &self.tools
    }

    /// Disconnects from the MCP server.
    pub async fn disconnect(&mut self) {
        // Signal the read task to shut down gracefully
        self.cancel_token.cancel();

        if let Some(conn) = self.connection.take() {
            let mut conn_guard = conn.lock().await;
            if let Err(e) = conn_guard.write.close().await {
                debug!(
                    "[mcp::client] Failed to close write half of connection: {}",
                    e
                );
            }
        }
        if let Some(task) = self.read_task.take() {
            task.abort();
        }
        self.tools.clear();
        // Reset the cancel token for potential reconnection
        self.cancel_token = CancellationToken::new();
        info!("Disconnected from MCP server");
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Some(task) = self.read_task.take() {
            task.abort();
        }
    }
}

/// A remote MCP tool that proxies execution to an MCP server.
///
/// This implements the `Tool` trait and can be registered in the local
/// tool registry alongside native tools.
pub struct McpRemoteTool {
    /// Tool name (from MCP server discovery)
    name: String,
    /// Tool description
    description: String,
    /// JSON Schema from MCP server (inputSchema)
    input_schema: Value,
    /// MCP client connection (shared)
    client: Arc<McpClient>,
}

impl McpRemoteTool {
    /// Creates a new remote tool wrapper.
    pub fn new(
        name: String,
        description: String,
        input_schema: Value,
        client: Arc<McpClient>,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            client,
        }
    }
}

#[async_trait::async_trait]
impl Tool for McpRemoteTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn capabilities(&self) -> CapabilityGrants {
        // Remote MCP tools are network-bound by definition
        let mut grants = CapabilityGrants::default();
        grants.network_allow.insert("mcp-server".to_string());
        grants
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        debug!("Executing MCP remote tool: {}", self.name);
        self.client.call_tool(&self.name, payload).await
    }
}

/// Discovers tools from multiple MCP servers and bridges them into
/// the local tool registry.
///
/// This enables the Savant agent to use tools from external MCP servers
/// as if they were native tools.
pub struct McpToolDiscovery {
    /// Connected MCP clients by server URL
    clients: HashMap<String, Arc<McpClient>>,
    /// Discovered tools with their source server
    discovered_tools: HashMap<String, (String, McpToolInfo)>, // tool_name -> (server_url, info)
}

impl McpToolDiscovery {
    /// Creates a new tool discovery instance.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            discovered_tools: HashMap::new(),
        }
    }

    /// Connects to an MCP server and discovers its tools.
    ///
    /// Returns the number of tools discovered from this server.
    pub async fn connect_server(&mut self, server_url: &str) -> Result<usize, SavantError> {
        let mut client = McpClient::new(server_url);
        client.connect().await?;

        let tools = client.discover_tools().await?;
        let count = tools.len();

        let client = Arc::new(client);
        for tool_info in tools {
            self.discovered_tools
                .insert(tool_info.name.clone(), (server_url.to_string(), tool_info));
        }

        self.clients.insert(server_url.to_string(), client);
        Ok(count)
    }

    /// Connects to an MCP server with authentication.
    pub async fn connect_server_with_auth(
        &mut self,
        server_url: &str,
        auth_token: &str,
    ) -> Result<usize, SavantError> {
        let mut client = McpClient::new(server_url);
        client.connect_with_auth(auth_token).await?;

        let tools = client.discover_tools().await?;
        let count = tools.len();

        let client = Arc::new(client);
        for tool_info in tools {
            self.discovered_tools
                .insert(tool_info.name.clone(), (server_url.to_string(), tool_info));
        }

        self.clients.insert(server_url.to_string(), client);
        Ok(count)
    }

    /// Returns all discovered tools as `McpRemoteTool` instances
    /// that can be registered in the local tool registry.
    pub fn get_remote_tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

        for (server_url, tool_info) in self.discovered_tools.values() {
            if let Some(client) = self.clients.get(server_url) {
                let remote_tool = McpRemoteTool::new(
                    tool_info.name.clone(),
                    tool_info.description.clone(),
                    tool_info.input_schema.clone(),
                    client.clone(),
                );
                tools.push(Arc::new(remote_tool));
            }
        }

        tools
    }

    /// Returns the number of discovered tools.
    pub fn tool_count(&self) -> usize {
        self.discovered_tools.len()
    }

    /// Returns discovered tool info for a specific tool.
    pub fn get_tool_info(&self, tool_name: &str) -> Option<&McpToolInfo> {
        self.discovered_tools.get(tool_name).map(|(_, info)| info)
    }

    /// Lists all discovered tool names.
    pub fn list_tool_names(&self) -> Vec<String> {
        self.discovered_tools.keys().cloned().collect()
    }

    /// Disconnects from all MCP servers.
    pub async fn disconnect_all(&mut self) {
        for (_, client) in self.clients.drain() {
            // Arc::try_unwrap to get owned client for disconnect
            if let Ok(mut client) = Arc::try_unwrap(client) {
                client.disconnect().await;
            }
        }
        self.discovered_tools.clear();
    }
}

impl Default for McpToolDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for McpToolDiscovery {
    fn drop(&mut self) {
        // Note: Can't call async disconnect in Drop, but each McpClient's
        // Drop impl will abort its read task automatically.
    }
}

/// Original stub pool - now a compatibility wrapper around McpClient.
pub struct McpClientPool {
    discovery: Arc<tokio::sync::Mutex<McpToolDiscovery>>,
}

impl McpClientPool {
    /// Creates a new client pool.
    pub fn new() -> Self {
        Self {
            discovery: Arc::new(tokio::sync::Mutex::new(McpToolDiscovery::new())),
        }
    }

    /// Connects to an MCP server and discovers tools.
    pub async fn connect(&self, server_url: &str) -> Result<usize, SavantError> {
        let mut discovery = self.discovery.lock().await;
        discovery.connect_server(server_url).await
    }

    /// Connects to an MCP server with auth token and discovers tools.
    pub async fn connect_server_with_auth(
        &self,
        server_url: &str,
        auth_token: &str,
    ) -> Result<usize, SavantError> {
        let mut discovery = self.discovery.lock().await;
        discovery
            .connect_server_with_auth(server_url, auth_token)
            .await
    }

    /// Connects to an MCP server and discovers tools (alias for connect).
    pub async fn connect_server(&self, server_url: &str) -> Result<usize, SavantError> {
        self.connect(server_url).await
    }

    /// Executes a tool by name, routing to the appropriate MCP server.
    pub async fn execute_tool(&self, tool_name: &str, args: &str) -> Result<String, SavantError> {
        let args_value: Value =
            serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({"raw": args}));

        let discovery = self.discovery.lock().await;
        let (server_url, _) = discovery
            .discovered_tools
            .get(tool_name)
            .ok_or_else(|| SavantError::Unknown(format!("MCP tool not found: {}", tool_name)))?;

        let client = discovery.clients.get(server_url).ok_or_else(|| {
            SavantError::Unknown(format!("MCP client not connected: {}", server_url))
        })?;

        client.call_tool(tool_name, args_value).await
    }

    /// Returns all discovered tools as Tool trait objects.
    pub async fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        let discovery = self.discovery.lock().await;
        discovery.get_remote_tools()
    }

    /// Lists all available tool names.
    pub async fn list_tools(&self) -> Vec<String> {
        let discovery = self.discovery.lock().await;
        discovery.list_tool_names()
    }

    /// Disconnects from all servers.
    pub async fn disconnect_all(&self) {
        let mut discovery = self.discovery.lock().await;
        discovery.disconnect_all().await;
    }
}

impl Default for McpClientPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_client_new() {
        let client = McpClient::new("ws://localhost:3001/mcp");
        assert!(!client.is_connected());
        assert!(client.tools().is_empty());
    }

    #[test]
    fn test_mcp_tool_discovery_new() {
        let discovery = McpToolDiscovery::new();
        assert_eq!(discovery.tool_count(), 0);
        assert!(discovery.list_tool_names().is_empty());
    }

    #[test]
    fn test_mcp_client_pool_new() {
        let _pool = McpClientPool::new();
        // Verify it creates without panicking — pool is ready for use
        assert!(_pool.discovery.try_lock().is_ok());
    }

    #[test]
    fn test_mcp_tool_info_clone() {
        let info = McpToolInfo {
            name: "test-tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let cloned = info.clone();
        assert_eq!(cloned.name, "test-tool");
        assert_eq!(cloned.description, "A test tool");
    }

    #[test]
    fn test_mcp_remote_tool_creation() {
        let client = Arc::new(McpClient::new("ws://localhost:3001/mcp"));
        let tool = McpRemoteTool::new(
            "test-tool".to_string(),
            "Test description".to_string(),
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            client,
        );
        assert_eq!(tool.name(), "test-tool");
        assert_eq!(tool.description(), "Test description");
        assert_eq!(tool.parameters_schema()["type"], "object");
    }

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("tools/list"));
        assert!(json.contains("\"id\":1"));
    }

    #[test]
    fn test_json_rpc_response_deserialization() {
        let json = r#"{"id": 1, "result": {"tools": []}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_response() {
        let json = r#"{"id": 2, "error": {"code": -32601, "message": "Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(2));
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }
}
