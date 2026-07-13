//! Global Workspace — Executive Monitor for continuous selection-broadcast.
//!
//! Implements Global Workspace Theory (GWT) for Savant agents.
//! Background modules compete for workspace access. The Executive Monitor
//! selects the most salient internal/external signal and broadcasts it
//! across the agentic framework, creating an unbroken stream of internal states.

pub mod broadcast;
pub mod state;

pub use broadcast::ExecutiveMonitor;
pub use state::{SignalSource, SignalType, WorkspaceSlot};
