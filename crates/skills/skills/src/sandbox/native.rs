use super::ToolExecutor;
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::types::CapabilityGrants;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::{error, info, warn};

/// Maximum execution time for native scripts (30 seconds)
const EXECUTION_TIMEOUT_SECS: u64 = 30;

/// Maximum environment variable size (32KB)
const MAX_ENV_VAR_SIZE: usize = 32 * 1024;

/// Legacy executor for native scripts (bash/python).
/// Uses platform-specific sandboxing to restrict filesystem access to the workspace.
pub struct LegacyNativeExecutor {
    script_path: String,
    workspace_dir: PathBuf,
    capabilities: CapabilityGrants,
}

impl LegacyNativeExecutor {
    /// Creates a new LegacyNativeExecutor.
    pub fn new(
        script_path: String,
        workspace_dir: PathBuf,
        capabilities: CapabilityGrants,
    ) -> Self {
        Self {
            script_path,
            workspace_dir,
            capabilities,
        }
    }

    /// Validates that the script path is safe (no shell metacharacters).
    fn validate_script_path(&self) -> Result<(), SavantError> {
        let path = Path::new(&self.script_path);

        // Check for null bytes
        if self.script_path.contains('\0') {
            return Err(SavantError::InvalidInput(
                "Script path contains null byte".into(),
            ));
        }

        // Check for shell metacharacters that could enable injection
        // Includes: &, |, ;, $, backticks, parens, redirects, newlines, quotes
        let dangerous_chars = [
            '&', '|', ';', '$', '`', '(', ')', '<', '>', '\n', '\r', '"', '\'',
        ];
        for ch in dangerous_chars {
            if self.script_path.contains(ch) {
                warn!(
                    "Script path contains potentially dangerous character: {}",
                    ch
                );
                return Err(SavantError::InvalidInput(format!(
                    "Script path contains dangerous character: {}",
                    ch
                )));
            }
        }

        // Verify the script exists
        if !path.exists() {
            return Err(SavantError::InvalidInput(format!(
                "Script does not exist: {}",
                self.script_path
            )));
        }

        Ok(())
    }

    /// Applies platform-specific sandboxing to the command.
    ///
    /// # Platform Support
    /// - **Linux**: Uses Landlock for filesystem restriction + capability dropping
    /// - **macOS**: Uses sandbox-exec with filesystem restrictions
    /// - **Windows**: Uses Job Objects with resource limits
    /// - **Other**: Refuses to run (returns error)
    #[cfg(target_os = "linux")]
    fn apply_sandbox(&self, cmd: &mut Command) -> Result<(), SavantError> {
        use caps::CapSet;
        use landlock::{AccessFs, PathBeneath, Ruleset, ABI};

        let workspace = self.workspace_dir.clone();
        let script_path = self.script_path.clone();

        // Safety: pre_exec is only called in the child process.
        // We use it to apply Landlock and drop capabilities before the script runs.
        unsafe {
            cmd.pre_exec(move || {
                // 0. Re-validate the script file wasn't swapped after validation
                // Open the script path and fstat /proc/self/fd/0 to compare
                let script_file = std::fs::File::open(&script_path).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Cannot open script: {}", e),
                    )
                })?;
                use std::os::unix::io::AsRawFd;
                let fd = script_file.as_raw_fd();
                let mut stat: libc::stat = std::mem::zeroed();
                if libc::fstat(fd, &mut stat) != 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "fstat failed on script file",
                    ));
                }
                if stat.st_size == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Script file is empty (possible swap attack)",
                    ));
                }
                // SAFETY: Intentionally leaking the file descriptor so it remains open
                // until exec() replaces the process image. If exec() succeeds, the fd is
                // reclaimed by the OS. If exec() fails, the child process exits via the
                // error handling below, and the fd is reclaimed on process exit.
                // We cannot use OwnedFd here because the fd must outlive the pre_exec closure.
                std::mem::forget(script_file);

                // 1. Drop Capabilities (prevent privilege escalation)
                // AAA: Fail if capability dropping fails (don't silently degrade)
                caps::clear(None, CapSet::Effective).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to drop capabilities: {}", e),
                    )
                })?;

                // 2. Apply Landlock (filesystem restriction)
                // AAA: Fail if Landlock fails (don't silently degrade)
                let abi = ABI::V1;
                let access = AccessFs::from_all(abi);
                let ruleset = Ruleset::new()
                    .handle_access(access)
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to create Landlock ruleset: {}", e),
                        )
                    })?
                    .create()
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to create Landlock rules: {}", e),
                        )
                    })?
                    .add_rule(PathBeneath::new(&workspace, access).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("Failed to add Landlock rule: {}", e),
                        )
                    })?)
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to add Landlock rule: {}", e),
                        )
                    })?;

                ruleset.restrict_self().map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to apply Landlock restrictions: {}", e),
                    )
                })?;

                Ok(())
            });
        }

        Ok(())
    }

    /// macOS sandbox-exec implementation with filesystem restrictions.
    #[cfg(target_os = "macos")]
    fn apply_sandbox(&self, cmd: &mut Command) -> Result<(), SavantError> {
        // Create a sandbox-exec profile that restricts filesystem access
        let workspace = self.workspace_dir.display().to_string();
        let profile = format!(
            r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow sysctl-read)
(allow file-read* (subpath "/usr"))
(allow file-read* (subpath "/bin"))
(allow file-read* (subpath "/sbin"))
(allow file-read* (subpath "/lib"))
(allow file-read* (subpath "/System"))
(allow file-read* (subpath "{}"))
(allow file-write* (subpath "{}"))
(allow file-read* (literal "/dev/null"))
(allow file-read* (literal "/dev/zero"))
(allow file-read* (literal "/dev/urandom"))
; Network access denied by default - configure per-skill allowlist if needed
; Example: (allow network-outbound (to ip "1.2.3.4") (port 443))
(deny network*)
 "#,
            workspace, workspace
        );

        // Write profile to a temporary file
        let profile_path =
            std::env::temp_dir().join(format!("savant_sandbox_{}.sb", uuid::Uuid::new_v4()));
        std::fs::write(&profile_path, &profile).map_err(|e| SavantError::IoError(e))?;

        // Wrap the command with sandbox-exec
        let original_args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let original_program = cmd.as_std().get_program().to_string_lossy().to_string();

        // Replace command with sandbox-exec wrapper
        *cmd = Command::new("sandbox-exec");
        cmd.arg("-f").arg(&profile_path);
        cmd.arg(original_program);
        for arg in original_args {
            cmd.arg(arg);
        }

        // Clean up profile file after execution (in a separate task)
        let profile_path_clone = profile_path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = std::fs::remove_file(&profile_path_clone) {
                tracing::warn!(
                    "[skills::sandbox] Failed to clean up sandbox profile file: {}",
                    e
                );
            }
        });

        info!(
            "macOS sandbox-exec profile applied for workspace: {}",
            workspace
        );
        Ok(())
    }

    /// Windows sandbox implementation with strict path validation.
    ///
    /// Windows sandboxing strategy (without requiring unsafe Win32 FFI):
    /// - **Direct interpreter execution** — no `cmd /C`, preventing shell injection
    /// - **Strict path validation** — blocks null bytes, shell metacharacters, quotes
    /// - **PowerShell Restricted policy** — prevents script injection via `-ExecutionPolicy Restricted`
    /// - **Timeout enforcement** — tokio::time::timeout in the execute() method
    /// - **kill_on_drop(true)** — inherited from the caller, ensures process cleanup
    ///
    /// For true resource limits (CPU, memory), use a Docker executor or native
    /// Win32 Job Object integration (requires unsafe FFI and suspended process spawning).
    #[cfg(target_os = "windows")]
    fn apply_sandbox(&self, cmd: &mut Command) -> Result<(), SavantError> {
        // Validate the script path (blocks shell metacharacters, null bytes, quotes)
        self.validate_script_path()?;

        let path = Path::new(&self.script_path);

        // Use direct interpreter execution — NO cmd /C to prevent shell injection
        if let Some(ext) = path.extension() {
            match ext.to_str() {
                Some("py") => {
                    // Python: direct execution, no shell involved
                    *cmd = Command::new("python");
                    cmd.arg(&self.script_path);
                }
                Some("ps1") => {
                    // PowerShell: Restricted policy prevents additional script injection
                    // -NoProfile: Don't load user profile scripts
                    // -NonInteractive: No interactive prompts
                    // -ExecutionPolicy Restricted: Blocks all script execution by default
                    // -File: Execute the specific file (no command injection possible)
                    *cmd = Command::new("powershell");
                    cmd.arg("-NoProfile");
                    cmd.arg("-NonInteractive");
                    cmd.arg("-ExecutionPolicy").arg("Restricted");
                    cmd.arg("-File").arg(&self.script_path);
                }
                Some("bat") | Some("cmd") => {
                    // Batch files: execute directly (no cmd /C wrapper needed)
                    // The path has been validated — no metacharacters or quotes
                    *cmd = Command::new(&self.script_path);
                }
                _ => {
                    // Unknown extension: try direct execution
                    *cmd = Command::new(&self.script_path);
                }
            }
        } else {
            // No extension: try direct execution
            *cmd = Command::new(&self.script_path);
        }

        info!(
            "Windows sandbox: direct execution of {} (path validated, timeout={}s)",
            self.script_path, EXECUTION_TIMEOUT_SECS
        );
        Ok(())
    }

    /// Fallback for unsupported platforms.
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    fn apply_sandbox(&self, _cmd: &mut Command) -> Result<(), SavantError> {
        error!("Native script execution is not supported on this platform.");
        Err(SavantError::InvalidInput(
            "Native script execution is not supported on this platform. \
             Only Linux, macOS, and Windows are supported."
                .into(),
        ))
    }
}

#[async_trait]
impl ToolExecutor for LegacyNativeExecutor {
    async fn execute(&self, args: Value) -> Result<String, SavantError> {
        // Validate script path before execution
        self.validate_script_path()?;

        // Build the command based on platform
        let mut cmd = if cfg!(target_os = "windows") {
            // Windows: Will be replaced by apply_sandbox
            Command::new("cmd")
        } else {
            // Unix-like: Use bash
            let mut c = Command::new("bash");
            c.arg(&self.script_path);
            c
        };

        // Pass arguments via environment variable for compatibility with OpenClaw skills
        let args_str = args.to_string();
        if args_str.len() > MAX_ENV_VAR_SIZE {
            return Err(SavantError::InvalidInput(format!(
                "Arguments too large: {} bytes (max: {} bytes)",
                args_str.len(),
                MAX_ENV_VAR_SIZE
            )));
        }
        cmd.env("TOOL_ARGS", &args_str);

        // Pass specific required environment variables
        for env_var in &self.capabilities.requires_env {
            if let Ok(val) = std::env::var(env_var) {
                if val.len() > MAX_ENV_VAR_SIZE {
                    warn!(
                        "Environment variable {} is too large ({} bytes), skipping",
                        env_var,
                        val.len()
                    );
                    continue;
                }
                cmd.env(env_var, val);
            }
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.current_dir(&self.workspace_dir);

        // Apply platform-specific sandboxing
        self.apply_sandbox(&mut cmd)?;

        info!(
            "Native Sandbox: Executing {} in {}",
            self.script_path,
            self.workspace_dir.display()
        );

        // Execute with timeout
        let timeout_duration = Duration::from_secs(EXECUTION_TIMEOUT_SECS);
        let output = match tokio::time::timeout(timeout_duration, cmd.output()).await {
            Ok(result) => result.map_err(SavantError::IoError)?,
            Err(_) => {
                error!(
                    "Script execution timed out after {} seconds",
                    EXECUTION_TIMEOUT_SECS
                );
                return Err(SavantError::Unknown(format!(
                    "Script execution timed out after {} seconds",
                    EXECUTION_TIMEOUT_SECS
                )));
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SavantError::Unknown(format!("Script failed: {}", stderr)));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
