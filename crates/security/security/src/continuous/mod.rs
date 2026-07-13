//! Continuous Agent Safety Framework
//!
//! Provides taint tracing, dynamic credential brokering, deterministic
//! circuit breakers, and state sandboxing for always-on agents.

pub mod circuit_breaker;
pub mod credentials;
pub mod taint;
