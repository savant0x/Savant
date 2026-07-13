pub mod cold_storage;
pub mod config;
pub mod error;
pub mod outbox;
pub mod watcher;
pub mod writer;

pub use cold_storage::ColdStorageManager;
pub use config::ObsidianConfig;
pub use error::VaultError;
pub use outbox::{CursorState, OutboxWorker, StateSnapshot};
pub use watcher::VaultWatcher;
pub use writer::{
    atomic_write, count_md_files, slugify, truncate_to_line, VaultStats, VaultWriter,
};
