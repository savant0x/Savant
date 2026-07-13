use savant_core::db::Storage;
use savant_core::types::ChatMessage;
use std::sync::Arc;
use tracing::{debug, error};

/// 🌀 GatewayPersistence: Logic for aligning multi-channel streams with UCH anchors.
pub struct GatewayPersistence;

impl GatewayPersistence {
    /// Determines the correct partition for a ChatMessage and persists it.
    /// Partition precedence: agent_id > sender > recipient > session_id > "global"
    /// Agent responses must go to `chat.{agent_name}` so get_history can find them.
    /// session_id (UUID) is a fallback only — it's invisible to the dashboard's
    /// HistoryRequest which queries by agent name.
    pub async fn persist_chat(
        storage: &Arc<Storage>,
        msg: &ChatMessage,
    ) -> Result<(), savant_core::error::SavantError> {
        // Partition: agent_id first (matches get_history lane_id), then sender/recipient,
        // session_id as last resort (UUID-only collections are invisible to dashboard)
        let partition = if let Some(aid) = &msg.agent_id {
            aid.clone()
        } else if let Some(sender) = &msg.sender {
            sender.clone()
        } else if let Some(recipient) = &msg.recipient {
            recipient.clone()
        } else if let Some(sid) = &msg.session_id {
            sid.0.clone()
        } else {
            "global".to_string()
        };

        // Sanitize partition key for filesystem safety
        let partition = savant_core::session::sanitize_session_id(&partition)
            .unwrap_or_else(|| partition.clone());

        let partition = crate::handlers::normalize_lane_id(&partition);

        debug!(partition = %partition, "Persisting message to substrate");

        storage.append_chat(&partition, msg).map_err(|e| {
            error!(error = %e, "Substrate write failure");
            savant_core::error::SavantError::Unknown(e.to_string())
        })
    }
}
