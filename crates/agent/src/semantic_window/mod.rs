//! Semantic Window Manager — Application-layer context management.
//!
//! Selects which context to send to external LLM providers based on
//! semantic importance. Does NOT manage KV cache (that's provider-side).
//! Instead, manages which message turns are included in the prompt.

pub mod scoring;
pub mod window;

pub use scoring::ContextScore;
pub use window::SemanticWindow;
