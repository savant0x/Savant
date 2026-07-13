//! Sandboxed WASM Compilation Pipeline
//!
//! Wraps `cargo build` in a strict jail (Landlock on Linux). Prevents the AI
//! from accidentally (or maliciously) accessing host environment variables or
//! reading sensitive files during the compilation phase.

use std::path::PathBuf;
use std::process::Stdio;
use thiserror::Error;
use tokio::process::Command;
use tracing::{error, info, warn};

#[derive(Error, Debug)]
pub enum CompilerError {
    #[error("Compilation failed: {0}")]
    BuildFailed(String),
    #[error("Sandbox error: {0}")]
    SandboxError(String),
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Environment error: {0}")]
    EnvError(String),
}

/// The ECHO Compiler handles sandboxed Rust-to-WASM builds.
pub struct EchoCompiler {
    workspace_root: PathBuf,
}

impl EchoCompiler {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    /// Compiles a generated Rust project into a WASM component.
    pub async fn compile_to_wasm(&self, project_dir: &str) -> Result<Vec<u8>, CompilerError> {
        let full_project_path = self.workspace_root.join(project_dir);
        let output_wasm = full_project_path.join("target/wasm32-wasip2/release/echo_tool.wasm");

        info!(
            "ECHO initiating sandboxed compilation for {:?}",
            full_project_path
        );

        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("--target=wasm32-wasip2")
            .arg("--release")
            .current_dir(&full_project_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Preserve critical environment variables for cargo to function
        // On Windows, cargo needs PATH, USERPROFILE, TEMP, APPDATA, CARGO_HOME, etc.
        // On Linux, we still want a minimal environment for sandboxing
        #[cfg(target_os = "linux")]
        {
            cmd.env_clear()
                .env("PATH", std::env::var("PATH").unwrap_or_default());
        }

        #[cfg(not(target_os = "linux"))]
        {
            // On Windows/macOS, preserve critical environment variables for cargo to function
            // Explicitly preserve these before clearing sensitive vars
            let preserve_vars = [
                "USERPROFILE",
                "TEMP",
                "APPDATA",
                "CARGO_HOME",
                "PATH",
                "SystemRoot",
                "HOMEDRIVE",
                "HOMEPATH",
                "LOCALAPPDATA",
            ];
            for var in preserve_vars {
                if let Ok(val) = std::env::var(var) {
                    cmd.env(var, val);
                }
            }
            // Clear sensitive vars that could leak credentials to untrusted code
            let sensitive_vars = [
                "OPENAI_API_KEY",
                "ANTHROPIC_API_KEY",
                "GOOGLE_API_KEY",
                "GROQ_API_KEY",
                "MISTRAL_API_KEY",
                "TOGETHER_API_KEY",
                "DEEPSEEK_API_KEY",
                "COHERE_API_KEY",
                "AZURE_OPENAI_API_KEY",
                "XAI_API_KEY",
                "FIREWORKS_API_KEY",
                "NOVITA_API_KEY",
                "OR_MASTER_KEY",
                "OPENROUTER_API_KEY",
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "DATABASE_URL",
                "REDIS_URL",
                "JWT_SECRET",
                "SAVANT_MASTER_SECRET_KEY",
                "SAVANT_MASTER_PUBLIC_KEY",
            ];
            for var in sensitive_vars {
                cmd.env_remove(var);
            }
        }

        #[cfg(target_os = "linux")]
        {
            use landlock::{AccessFs, PathBeneath, Ruleset, ABI};
            let project_path_clone = full_project_path.clone();
            // SAFETY: This `pre_exec` closure only configures Landlock filesystem
            // sandboxing before the child process begins execution. It runs in the
            // forked child before execve(), performing only Landlock ABI setup and
            // ruleset restriction. No user-defined memory is accessed unsafely; all
            // captured variables are used by value. This is the standard pattern for
            // applying Linux security restrictions to child processes.
            unsafe {
                cmd.pre_exec(move || {
                    let abi = ABI::V1;
                    let ruleset = Ruleset::new()
                        .handle_access(AccessFs::from_all(abi))
                        .map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Landlock ruleset init failed",
                            )
                        })?
                        .create()
                        .map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Landlock creation failed",
                            )
                        })?
                        .add_rule(
                            PathBeneath::new(&project_path_clone, AccessFs::from_all(abi))
                                .map_err(|_| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::Other,
                                        "Project rule failed",
                                    )
                                })?,
                        )
                        .map_err(|_| {
                            std::io::Error::new(std::io::ErrorKind::Other, "Rule add failed")
                        })?
                        // Add common system paths required for compilation toolchains
                        .add_rule(
                            PathBeneath::new("/usr/lib", AccessFs::from_read(abi)).map_err(
                                |_| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::Other,
                                        "System lib rule failed",
                                    )
                                },
                            )?,
                        )
                        .map_err(|_| {
                            std::io::Error::new(std::io::ErrorKind::Other, "Rule add failed")
                        })?;

                    ruleset.restrict_self().map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "Landlock restriction failed",
                        )
                    })?;
                    Ok(())
                });
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            warn!(
                "ECHO sandboxing (Landlock) is not supported on this OS. Running without sandbox."
            );
        }

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("ECHO Compilation Failed:\n{}", stderr);
            return Err(CompilerError::BuildFailed(stderr.to_string()));
        }

        info!("Compilation successful. Output target generated.");

        let wasm_bytes = tokio::fs::read(&output_wasm).await?;
        Ok(wasm_bytes)
    }
}
