//! Provider implementations.

pub mod gmail;
pub mod notion;

pub use gmail::{GmailConfig, GmailProvider};
pub use notion::{NotionConfig, NotionProvider};
