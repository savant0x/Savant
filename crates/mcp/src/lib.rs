#![forbid(unsafe_code)]

//! Savant MCP Integration
//! Contains client pooling for downstream services via Model Context Protocol and
//! server execution points mapped to Axum.

pub mod circuit;
pub mod client;
pub mod server;
