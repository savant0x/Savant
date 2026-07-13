//! Auto-detection of installed LSP servers by checking PATH.

use std::path::PathBuf;

/// A discovered LSP server configuration.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    /// Binary name (e.g., "rust-analyzer").
    pub command: String,
    /// Arguments to pass to the binary.
    pub args: Vec<String>,
    /// Language this server handles.
    pub language: String,
    /// File extensions this server handles.
    pub file_extensions: Vec<String>,
}

/// Known LSP servers and their configurations.
fn known_servers() -> Vec<LspServerConfig> {
    vec![
        LspServerConfig {
            command: "rust-analyzer".to_string(),
            args: vec![],
            language: "rust".to_string(),
            file_extensions: vec!["rs".to_string()],
        },
        LspServerConfig {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            language: "typescript".to_string(),
            file_extensions: vec![
                "ts".to_string(),
                "tsx".to_string(),
                "js".to_string(),
                "jsx".to_string(),
            ],
        },
        LspServerConfig {
            command: "pyright-langserver".to_string(),
            args: vec!["--stdio".to_string()],
            language: "python".to_string(),
            file_extensions: vec!["py".to_string(), "pyi".to_string()],
        },
        LspServerConfig {
            command: "gopls".to_string(),
            args: vec![],
            language: "go".to_string(),
            file_extensions: vec!["go".to_string()],
        },
        LspServerConfig {
            command: "clangd".to_string(),
            args: vec![],
            language: "c".to_string(),
            file_extensions: vec![
                "c".to_string(),
                "h".to_string(),
                "cpp".to_string(),
                "hpp".to_string(),
                "cc".to_string(),
            ],
        },
    ]
}

/// Discover installed LSP servers by checking PATH.
pub fn discover_servers() -> Vec<LspServerConfig> {
    known_servers()
        .into_iter()
        .filter(|s| which_exists(&s.command))
        .collect()
}

/// Check if a binary exists in PATH.
fn which_exists(name: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path.split(sep) {
            let full = PathBuf::from(dir).join(name);
            if full.is_file() {
                return true;
            }
            if cfg!(windows) {
                let full_exe = PathBuf::from(dir).join(format!("{}.exe", name));
                if full_exe.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// Find the LSP server config for a given language.
pub fn find_server_for_language(language: &str) -> Option<LspServerConfig> {
    discover_servers()
        .into_iter()
        .find(|s| s.language == language)
}

/// Find the LSP server config for a given file extension.
pub fn find_server_for_extension(extension: &str) -> Option<LspServerConfig> {
    discover_servers()
        .into_iter()
        .find(|s| s.file_extensions.iter().any(|e| e == extension))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_at_least_check() {
        let _servers = discover_servers();
    }

    #[test]
    fn test_find_nonexistent() {
        let result = find_server_for_language("nonexistent_lang_12345");
        assert!(result.is_none());
    }

    #[test]
    fn test_known_servers_have_required_fields() {
        let servers = known_servers();
        for server in &servers {
            assert!(!server.command.is_empty());
            assert!(!server.language.is_empty());
            assert!(!server.file_extensions.is_empty());
        }
    }
}
