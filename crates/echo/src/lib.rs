//! savant-echo: Autonomous Engineering & Hot-Swapping Substrate
//!
//! Provides the infrastructure for autonomous tool compilation and
//! zero-downtime atomic reconfiguration.

pub mod circuit_breaker;
pub mod compiler;
pub mod registry;
pub mod watcher;

pub use circuit_breaker::ComponentMetrics;
pub use compiler::EchoCompiler;
pub use registry::HotSwappableRegistry;
