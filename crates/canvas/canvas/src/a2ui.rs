//! A2UI WebSocket Handler
//!
//! Provides real-time canvas state broadcasting and client command handling
//! via WebSocket connections. Supports:
//! - Real-time state synchronization
//! - Client subscription management
//! - Heartbeat/ping-pong for connection health
//! - Binary frames for efficient image transfer

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};

use crate::diff::{compute_diff, DiffResult};
use crate::types::{CanvasElement, CanvasState};

/// Commands that can be sent from clients to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasCommand {
    /// Request the current full state
    GetState,
    /// Update elements on the canvas
    UpdateElements { elements: Vec<CanvasElement> },
    /// Remove elements by ID
    RemoveElements { ids: Vec<String> },
    /// Clear all elements
    Clear,
    /// Subscribe to updates for specific element types
    Subscribe { element_types: Option<Vec<String>> },
    /// Ping for connection health check
    Ping { timestamp: u64 },
}

/// Events sent from the server to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasEvent {
    /// Full state snapshot
    StateSnapshot { state: CanvasState, version: u64 },
    /// Incremental state diff
    StateDiff { diff: DiffResult },
    /// Element added
    ElementAdded {
        element: CanvasElement,
        version: u64,
    },
    /// Element updated
    ElementUpdated {
        element: CanvasElement,
        version: u64,
    },
    /// Element removed
    ElementRemoved { id: String, version: u64 },
    /// Pong response to ping
    Pong {
        timestamp: u64,
        server_timestamp: u64,
    },
    /// Error message
    Error { message: String, code: String },
}

/// Shared canvas state manager.
pub struct CanvasManager {
    /// Current canvas state
    state: RwLock<CanvasState>,
    /// Broadcast channel for state updates
    update_tx: broadcast::Sender<CanvasEvent>,
    /// Maximum number of elements allowed
    max_elements: usize,
}

impl CanvasManager {
    /// Creates a new CanvasManager.
    pub fn new(max_elements: usize) -> Self {
        let (update_tx, _) = broadcast::channel(1024);
        Self {
            state: RwLock::new(CanvasState::default()),
            update_tx,
            max_elements,
        }
    }

    /// Gets the current state.
    pub async fn get_state(&self) -> CanvasState {
        self.state.read().await.clone()
    }

    /// Updates elements on the canvas.
    pub async fn update_elements(&self, elements: Vec<CanvasElement>) -> Result<u64, String> {
        let mut state = self.state.write().await;

        // Check element count limit
        let new_count = state.elements.len() + elements.len();
        if new_count > self.max_elements {
            return Err(format!(
                "Element count would exceed limit: {} (max: {})",
                new_count, self.max_elements
            ));
        }

        // Validate element IDs
        for el in &elements {
            if el.id.is_empty() {
                return Err("Element ID cannot be empty".to_string());
            }
            if el.id.len() > 256 {
                return Err(format!("Element ID too long: {} (max: 256)", el.id.len()));
            }
            if el.id.contains('\0') {
                return Err("Element ID contains null byte".to_string());
            }
        }

        let old_version = state.version;
        let old_state = state.clone();

        // Apply updates
        for el in elements {
            let is_new = !state.elements.contains_key(&el.id);
            state.elements.insert(el.id.clone(), el.clone());

            // Broadcast individual element events
            let event = if is_new {
                CanvasEvent::ElementAdded {
                    element: el,
                    version: state.version + 1,
                }
            } else {
                CanvasEvent::ElementUpdated {
                    element: el,
                    version: state.version + 1,
                }
            };
            if let Err(e) = self.update_tx.send(event) {
                // Expected during initialization when no WebSocket subscribers exist yet.
                // State is stored in self.state — subscribers get a full snapshot on connect.
                debug!(
                    "[canvas::a2ui] No subscribers for element event (expected during init): {:?}",
                    e
                );
            }
        }

        state.version += 1;

        // Broadcast state diff
        let old_val = serde_json::to_value(&old_state)
            .map_err(|e| format!("Failed to serialize old state: {}", e))?;
        let new_val = serde_json::to_value(&*state)
            .map_err(|e| format!("Failed to serialize new state: {}", e))?;
        let diff = compute_diff(&old_val, &new_val, old_version, state.version);
        if let Err(e) = self.update_tx.send(CanvasEvent::StateDiff { diff }) {
            debug!(
                "[canvas::a2ui] No subscribers for state diff (expected during init): {:?}",
                e
            );
        }

        Ok(state.version)
    }

    /// Removes elements by ID.
    pub async fn remove_elements(&self, ids: Vec<String>) -> Result<u64, String> {
        let mut state = self.state.write().await;
        let old_version = state.version;
        let old_state = state.clone();

        for id in &ids {
            if state.elements.remove(id).is_some() {
                if let Err(e) = self.update_tx.send(CanvasEvent::ElementRemoved {
                    id: id.clone(),
                    version: state.version + 1,
                }) {
                    debug!(
                        "[canvas::a2ui] No subscribers for element removed event (expected during init): {:?}",
                        e
                    );
                }
            }
        }

        state.version += 1;

        // Broadcast state diff
        let old_val = serde_json::to_value(&old_state)
            .map_err(|e| format!("Failed to serialize old state: {}", e))?;
        let new_val = serde_json::to_value(&*state)
            .map_err(|e| format!("Failed to serialize new state: {}", e))?;
        let diff = compute_diff(&old_val, &new_val, old_version, state.version);
        if let Err(e) = self.update_tx.send(CanvasEvent::StateDiff { diff }) {
            debug!(
                "[canvas::a2ui] No subscribers for state diff (expected during init): {:?}",
                e
            );
        }

        Ok(state.version)
    }

    /// Clears all elements.
    pub async fn clear(&self) -> Result<u64, String> {
        let mut state = self.state.write().await;
        let old_version = state.version;
        let old_state = state.clone();

        state.elements.clear();
        state.version += 1;

        // Broadcast state diff
        let old_val = serde_json::to_value(&old_state)
            .map_err(|e| format!("Failed to serialize old state: {}", e))?;
        let new_val = serde_json::to_value(&*state)
            .map_err(|e| format!("Failed to serialize new state: {}", e))?;
        let diff = compute_diff(&old_val, &new_val, old_version, state.version);
        if let Err(e) = self.update_tx.send(CanvasEvent::StateDiff { diff }) {
            debug!(
                "[canvas::a2ui] No subscribers for state diff (expected during init): {:?}",
                e
            );
        }

        Ok(state.version)
    }

    /// Subscribes to canvas updates.
    pub fn subscribe(&self) -> broadcast::Receiver<CanvasEvent> {
        self.update_tx.subscribe()
    }
}

/// Shared state for the A2UI handler.
pub struct A2UIState {
    pub canvas: Arc<CanvasManager>,
}

/// Handles WebSocket upgrade requests for A2UI connections.
pub async fn a2ui_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<A2UIState>>,
) -> impl IntoResponse {
    let canvas = state.canvas.clone();
    ws.on_upgrade(move |socket| handle_a2ui_connection(socket, canvas))
}

/// Handles an individual A2UI WebSocket connection.
pub async fn handle_a2ui_connection(socket: WebSocket, canvas: Arc<CanvasManager>) {
    let (mut sender, mut receiver) = socket.split();

    // Use mpsc channel to unify sending to the WebSocket
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(128);

    // Spawn task to handle WebSocket sends
    let sender_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Send initial state snapshot
    let initial_state = canvas.get_state().await;
    let snapshot = CanvasEvent::StateSnapshot {
        state: initial_state.clone(),
        version: initial_state.version,
    };

    if let Ok(msg) = serde_json::to_string(&snapshot) {
        if let Err(e) = tx.send(Message::Text(msg)).await {
            warn!("[canvas::a2ui] Failed to send initial snapshot: {:?}", e);
        }
    }

    // Subscribe to canvas updates
    let mut update_rx = canvas.subscribe();
    let tx_for_events = tx.clone();

    // Spawn task to forward updates to client
    let events_task = tokio::spawn(async move {
        while let Ok(event) = update_rx.recv().await {
            match serde_json::to_string(&event) {
                Ok(msg) => {
                    if tx_for_events.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    error!("A2UI: Failed to serialize event: {}", e);
                    break;
                }
            }
        }
    });

    // Handle incoming commands from client
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => match serde_json::from_str::<CanvasCommand>(&text) {
                Ok(cmd) => {
                    handle_canvas_command(cmd, &canvas).await;
                }
                Err(e) => {
                    warn!("A2UI: Invalid command format: {}", e);
                    let error_event = CanvasEvent::Error {
                        message: format!("Invalid command: {}", e),
                        code: "INVALID_COMMAND".to_string(),
                    };
                    if let Ok(msg) = serde_json::to_string(&error_event) {
                        if let Err(e) = tx.send(Message::Text(msg)).await {
                            warn!("[canvas::a2ui] Failed to send error event: {:?}", e);
                        }
                    }
                }
            },
            Message::Binary(data) => {
                debug!("A2UI: Received binary frame ({} bytes)", data.len());
            }
            Message::Ping(data) => {
                if let Err(e) = tx.send(Message::Pong(data)).await {
                    warn!("[canvas::a2ui] Failed to send pong response: {:?}", e);
                }
            }
            Message::Pong(_) => {
                debug!("A2UI: Received pong");
            }
            Message::Close(_) => {
                info!("A2UI: Client requested close");
                break;
            }
        }
    }

    // Clean up
    events_task.abort();
    sender_task.abort();
    info!("A2UI: WebSocket connection closed");
}

/// Handles a canvas command from a client.
async fn handle_canvas_command(cmd: CanvasCommand, canvas: &Arc<CanvasManager>) {
    match cmd {
        CanvasCommand::GetState => {
            // State is sent automatically on connect and via diffs
            debug!("A2UI: GetState command received (state already synchronized)");
        }
        CanvasCommand::UpdateElements { elements } => {
            match canvas.update_elements(elements).await {
                Ok(version) => {
                    debug!("A2UI: Elements updated to version {}", version);
                }
                Err(e) => {
                    warn!("A2UI: Failed to update elements: {}", e);
                }
            }
        }
        CanvasCommand::RemoveElements { ids } => match canvas.remove_elements(ids).await {
            Ok(version) => {
                debug!("A2UI: Elements removed, version {}", version);
            }
            Err(e) => {
                warn!("A2UI: Failed to remove elements: {}", e);
            }
        },
        CanvasCommand::Clear => match canvas.clear().await {
            Ok(version) => {
                debug!("A2UI: Canvas cleared, version {}", version);
            }
            Err(e) => {
                warn!("A2UI: Failed to clear canvas: {}", e);
            }
        },
        CanvasCommand::Subscribe { element_types: _ } => {
            // Subscription is handled at the connection level
            debug!("A2UI: Subscribe command received");
        }
        CanvasCommand::Ping { timestamp } => {
            // Pong is handled at the WebSocket level
            debug!("A2UI: Ping received at {}", timestamp);
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::types::CanvasElement;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_canvas_manager_update() {
        let manager = CanvasManager::new(100);

        let element = CanvasElement {
            id: "test-1".to_string(),
            element_type: "rect".to_string(),
            properties: HashMap::new(),
        };

        let version = manager
            .update_elements(vec![element])
            .await
            .expect("update_elements should succeed");
        assert_eq!(version, 1);

        let state = manager.get_state().await;
        assert_eq!(state.elements.len(), 1);
        assert!(state.elements.contains_key("test-1"));
    }

    #[tokio::test]
    async fn test_canvas_manager_remove() {
        let manager = CanvasManager::new(100);

        let element = CanvasElement {
            id: "test-1".to_string(),
            element_type: "rect".to_string(),
            properties: HashMap::new(),
        };

        manager
            .update_elements(vec![element])
            .await
            .expect("update_elements should succeed");
        let version = manager
            .remove_elements(vec!["test-1".to_string()])
            .await
            .expect("remove_elements should succeed");
        assert_eq!(version, 2);

        let state = manager.get_state().await;
        assert_eq!(state.elements.len(), 0);
    }

    #[tokio::test]
    async fn test_canvas_manager_clear() {
        let manager = CanvasManager::new(100);

        let elements = vec![
            CanvasElement {
                id: "test-1".to_string(),
                element_type: "rect".to_string(),
                properties: HashMap::new(),
            },
            CanvasElement {
                id: "test-2".to_string(),
                element_type: "circle".to_string(),
                properties: HashMap::new(),
            },
        ];

        manager
            .update_elements(elements)
            .await
            .expect("update_elements should succeed");
        let version = manager.clear().await.expect("clear should succeed");
        assert_eq!(version, 2);

        let state = manager.get_state().await;
        assert_eq!(state.elements.len(), 0);
    }

    #[tokio::test]
    async fn test_canvas_manager_max_elements() {
        let manager = CanvasManager::new(2);

        let elements = vec![
            CanvasElement {
                id: "test-1".to_string(),
                element_type: "rect".to_string(),
                properties: HashMap::new(),
            },
            CanvasElement {
                id: "test-2".to_string(),
                element_type: "circle".to_string(),
                properties: HashMap::new(),
            },
            CanvasElement {
                id: "test-3".to_string(),
                element_type: "text".to_string(),
                properties: HashMap::new(),
            },
        ];

        let result = manager.update_elements(elements).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Element count would exceed limit"));
    }

    #[tokio::test]
    async fn test_canvas_manager_new_empty() {
        let manager = CanvasManager::new(100);
        let state = manager.get_state().await;
        assert!(state.elements.is_empty());
        assert_eq!(state.version, 0);
    }

    #[tokio::test]
    async fn test_canvas_manager_get_state_after_update() {
        let manager = CanvasManager::new(100);
        let element = CanvasElement {
            id: "el-1".to_string(),
            element_type: "text".to_string(),
            properties: HashMap::new(),
        };
        manager.update_elements(vec![element]).await.unwrap();
        let state = manager.get_state().await;
        assert_eq!(state.elements.len(), 1);
        assert!(state.elements.contains_key("el-1"));
    }

    #[tokio::test]
    async fn test_canvas_manager_version_increments() {
        let manager = CanvasManager::new(100);
        let e1 = CanvasElement {
            id: "el-1".to_string(),
            element_type: "rect".to_string(),
            properties: HashMap::new(),
        };
        let v1 = manager.update_elements(vec![e1]).await.unwrap();
        assert_eq!(v1, 1);

        let e2 = CanvasElement {
            id: "el-2".to_string(),
            element_type: "circle".to_string(),
            properties: HashMap::new(),
        };
        let v2 = manager.update_elements(vec![e2]).await.unwrap();
        assert_eq!(v2, 2);
    }

    #[tokio::test]
    async fn test_canvas_manager_update_existing_element() {
        let manager = CanvasManager::new(100);
        let e1 = CanvasElement {
            id: "el-1".to_string(),
            element_type: "rect".to_string(),
            properties: HashMap::new(),
        };
        manager.update_elements(vec![e1]).await.unwrap();

        let mut props = HashMap::new();
        props.insert("color".to_string(), "red".to_string());
        let e1_updated = CanvasElement {
            id: "el-1".to_string(),
            element_type: "rect".to_string(),
            properties: props,
        };
        manager.update_elements(vec![e1_updated]).await.unwrap();

        let state = manager.get_state().await;
        let el = state.elements.get("el-1").unwrap();
        assert_eq!(el.properties.get("color").unwrap(), "red");
    }

    #[tokio::test]
    async fn test_canvas_manager_subscribe() {
        let manager = CanvasManager::new(100);
        let mut rx = manager.subscribe();
        let element = CanvasElement {
            id: "el-1".to_string(),
            element_type: "rect".to_string(),
            properties: HashMap::new(),
        };
        manager.update_elements(vec![element]).await.unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(event.is_ok());
    }
}
