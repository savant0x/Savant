use crate::error::SavantError;
use std::path::Path;
use tokio::fs;

/// Reads a mandatory file or returns a default string.
pub async fn read_or_default(path: &Path, default: &str) -> String {
    match fs::read_to_string(path).await {
        Ok(content) => content,
        Err(_) => default.to_string(),
    }
}

/// Reads an optional file returning None if missing.
pub async fn read_optional(path: &Path) -> Option<String> {
    fs::read_to_string(path).await.ok()
}

/// Appends a line to a .env file in the specified directory.
pub async fn append_to_env(
    workspace_path: &Path,
    key: &str,
    value: &str,
) -> Result<(), SavantError> {
    let env_path = workspace_path.join(".env");
    let line = format!("{}={}\n", key, value);

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&env_path)?;
        file.write_all(line.as_bytes())?;
    }

    #[cfg(not(unix))]
    {
        use tokio::io::AsyncWriteExt;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(env_path)
            .await?;
        file.write_all(line.as_bytes()).await?;
    }

    Ok(())
}

/// Ensures a directory exists.
pub async fn ensure_dir(path: &Path) -> Result<(), SavantError> {
    fs::create_dir_all(path).await?;
    Ok(())
}

/// Recursively copies a directory from `src` to `dst`.
/// Creates `dst` if it does not exist. Copies all files and subdirectories.
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
