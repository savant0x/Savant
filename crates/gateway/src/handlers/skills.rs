// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! Skill management handlers for the gateway
//!
//! Provides WebSocket control frame handlers for:
//! - Listing installed skills
//! - Installing skills from ClawHub
//! - Enabling/disabling skills
//! - Running security scans
//! - Uninstalling skills
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use savant_core::bus::NexusBridge;
use savant_core::types::ControlFrame;
use std::sync::Arc;
use tracing::{error, info, warn};

/// Validates a skill name to prevent path traversal attacks.
/// Only allows alphanumeric characters, hyphens, and underscores.
fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if name.len() > 128 {
        return Err("Skill name too long (max 128 chars)".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "Invalid skill name '{}': only alphanumeric, hyphens, and underscores allowed",
            name
        ));
    }
    if name.contains("..") {
        return Err("Skill name cannot contain '..'".to_string());
    }
    Ok(())
}

/// Validates and restricts a skill path to the allowed base directory.
/// Returns the canonicalized path if valid, or an error if it escapes.
fn validate_skill_path(
    path: &std::path::Path,
    base: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Invalid path: {}", e))?;
    let canonical_base = base
        .canonicalize()
        .map_err(|e| format!("Invalid base path: {}", e))?;

    if !canonical.starts_with(&canonical_base) {
        return Err(format!(
            "Path traversal detected: '{}' escapes base directory",
            path.display()
        ));
    }
    Ok(canonical)
}

/// Result of a skill operation
#[derive(Debug, serde::Serialize)]
pub struct SkillOperationResult {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Handle skill management control frames
pub async fn handle_skill_control(
    frame: ControlFrame,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    match frame {
        ControlFrame::SkillsList { agent_id } => {
            handle_skills_list(agent_id, session_id, nexus).await;
        }
        ControlFrame::SkillInstall { source, agent_id } => {
            handle_skill_install(source, agent_id, session_id, nexus).await;
        }
        ControlFrame::SkillUninstall {
            skill_name,
            agent_id,
        } => {
            handle_skill_uninstall(skill_name, agent_id, session_id, nexus).await;
        }
        ControlFrame::SkillEnable { skill_name } => {
            handle_skill_enable(skill_name, session_id, nexus).await;
        }
        ControlFrame::SkillDisable { skill_name } => {
            handle_skill_disable(skill_name, session_id, nexus).await;
        }
        ControlFrame::SkillScan { skill_path } => {
            handle_skill_scan(skill_path, session_id, nexus).await;
        }
        _ => {
            // Not a skill frame - ignore
        }
    }
}

/// Handle SkillsList - return all installed skills with their status
async fn handle_skills_list(
    agent_id: Option<String>,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    info!("📋 Skills list requested for agent: {:?}", agent_id);

    // Get skill directories
    let workspace_dir = std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("Failed to get current directory: {}", e);
        std::path::PathBuf::from(".")
    });
    let swarm_skills_dir = workspace_dir.join("skills");

    let mut skills = Vec::new();

    // Scan swarm-wide skills
    if swarm_skills_dir.exists() {
        scan_skills_directory(&swarm_skills_dir, "swarm", &mut skills).await;
    }

    // Scan agent-specific skills if agent_id provided
    if let Some(ref agent_id) = agent_id {
        let agent_skills_dir = workspace_dir
            .join("workspaces")
            .join(format!("workspace-{}", agent_id))
            .join("skills");

        if agent_skills_dir.exists() {
            scan_skills_directory(
                &agent_skills_dir,
                &format!("agent:{}", agent_id),
                &mut skills,
            )
            .await;
        }
    }

    let result = serde_json::json!({
        "skills": skills,
        "count": skills.len(),
    });

    if let Err(e) = send_skill_response("SKILLS_LIST", result, session_id, nexus).await {
        warn!("[gateway] Failed to send SKILLS_LIST response: {}", e);
    }
}

/// Handle SkillInstall - download and install a skill
async fn handle_skill_install(
    source: String,
    agent_id: Option<String>,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    info!(
        "⬇️ Skill install requested: {} for agent: {:?}",
        source, agent_id
    );

    // GTW-12: Validate agent_id to prevent path traversal
    if let Some(ref agent_id) = agent_id {
        if let Err(e) = validate_skill_name(agent_id) {
            let error_result = serde_json::json!({
                "success": false,
                "message": format!("Invalid agent_id: {}", e),
            });
            if let Err(send_err) =
                send_skill_response("SKILL_INSTALL_RESULT", error_result, session_id, nexus).await
            {
                warn!(
                    "[gateway] Failed to send SKILL_INSTALL_RESULT: {}",
                    send_err
                );
            }
            return;
        }
    }

    let workspace_dir = std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("Failed to get current directory: {}", e);
        std::path::PathBuf::from(".")
    });
    let target_dir = if let Some(ref agent_id) = agent_id {
        workspace_dir
            .join("workspaces")
            .join(format!("workspace-{}", agent_id))
            .join("skills")
    } else {
        workspace_dir.join("skills")
    };

    // Ensure target directory exists
    if let Err(e) = tokio::fs::create_dir_all(&target_dir).await {
        let error_result = serde_json::json!({
            "success": false,
            "message": format!("Failed to create skills directory: {}", e),
        });
        if let Err(e) =
            send_skill_response("SKILL_INSTALL_RESULT", error_result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_INSTALL_RESULT: {}", e);
        }
        return;
    }

    // Install from ClawHub
    let scanner = savant_skills::security::SecurityScanner::new();
    let client = savant_skills::clawhub::ClawHubClient::new();

    match client.install(&source, &target_dir, &scanner).await {
        Ok(result) => {
            let response_data = serde_json::json!({
                "success": result.success,
                "skill_name": result.skill_name,
                "message": result.message,
                "gate_result": result.gate_result.as_ref().map(|g| {
                    serde_json::json!({
                        "risk_level": format!("{:?}", g.scan_result().risk_level),
                        "required_clicks": g.required_clicks(),
                        "completed_clicks": g.completed_clicks(),
                    })
                }),
            });
            if let Err(e) =
                send_skill_response("SKILL_INSTALL_RESULT", response_data, session_id, nexus).await
            {
                warn!("[gateway] Failed to send SKILL_INSTALL_RESULT: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to install skill {}: {}", source, e);
            let error_result = serde_json::json!({
                "success": false,
                "message": format!("Installation failed: {}", e),
            });
            if let Err(e) =
                send_skill_response("SKILL_INSTALL_RESULT", error_result, session_id, nexus).await
            {
                warn!("[gateway] Failed to send SKILL_INSTALL_RESULT: {}", e);
            }
        }
    }
}

/// Handle SkillUninstall - remove a skill
async fn handle_skill_uninstall(
    skill_name: String,
    _agent_id: Option<String>,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    // Validate skill name to prevent path traversal
    if let Err(e) = validate_skill_name(&skill_name) {
        warn!("Invalid skill name rejected: {}", e);
        let result = serde_json::json!({
            "success": false,
            "message": format!("Invalid skill name: {}", e),
        });
        if let Err(e) =
            send_skill_response("SKILL_UNINSTALL_RESULT", result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_UNINSTALL_RESULT: {}", e);
        }
        return;
    }

    info!("🗑️ Skill uninstall requested: {}", skill_name);

    let workspace_dir = std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("Failed to get current directory: {}", e);
        std::path::PathBuf::from(".")
    });
    let skills_base = workspace_dir.join("skills");
    let skill_dir = skills_base.join(&skill_name);

    // Verify the resolved path stays within the skills directory
    if let Err(e) = validate_skill_path(&skill_dir, &skills_base) {
        warn!("Path traversal blocked: {}", e);
        let result = serde_json::json!({
            "success": false,
            "message": format!("Security violation: {}", e),
        });
        if let Err(e) =
            send_skill_response("SKILL_UNINSTALL_RESULT", result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_UNINSTALL_RESULT: {}", e);
        }
        return;
    }

    let result = if skill_dir.exists() {
        match tokio::fs::remove_dir_all(&skill_dir).await {
            Ok(()) => serde_json::json!({
                "success": true,
                "message": format!("Skill '{}' uninstalled successfully", skill_name),
            }),
            Err(e) => serde_json::json!({
                "success": false,
                "message": format!("Failed to remove skill: {}", e),
            }),
        }
    } else {
        serde_json::json!({
            "success": false,
            "message": format!("Skill '{}' not found", skill_name),
        })
    };

    if let Err(e) = send_skill_response("SKILL_UNINSTALL_RESULT", result, session_id, nexus).await {
        warn!("[gateway] Failed to send SKILL_UNINSTALL_RESULT: {}", e);
    }
}

/// Handle SkillEnable - enable a skill
async fn handle_skill_enable(
    skill_name: String,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    if let Err(e) = validate_skill_name(&skill_name) {
        warn!("Invalid skill name rejected: {}", e);
        let result = serde_json::json!({
            "success": false,
            "message": format!("Invalid skill name: {}", e),
        });
        if let Err(e) = send_skill_response("SKILL_ENABLE_RESULT", result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_ENABLE_RESULT: {}", e);
        }
        return;
    }

    info!("✅ Skill enable requested: {}", skill_name);

    let workspace_dir = std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("Failed to get current directory: {}", e);
        std::path::PathBuf::from(".")
    });
    let skills_base = workspace_dir.join("skills");
    let enabled_file = skills_base.join(&skill_name).join(".enabled");

    // Validate path stays within skills directory
    let skill_dir = skills_base.join(&skill_name);
    if let Err(e) = validate_skill_path(&skill_dir, &skills_base) {
        warn!("Path traversal blocked: {}", e);
        let result = serde_json::json!({
            "success": false,
            "message": format!("Security violation: {}", e),
        });
        if let Err(e) = send_skill_response("SKILL_ENABLE_RESULT", result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_ENABLE_RESULT: {}", e);
        }
        return;
    }

    let result = match tokio::fs::write(&enabled_file, "").await {
        Ok(()) => serde_json::json!({
            "success": true,
            "message": format!("Skill '{}' enabled", skill_name),
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "message": format!("Failed to enable skill: {}", e),
        }),
    };

    if let Err(e) = send_skill_response("SKILL_ENABLE_RESULT", result, session_id, nexus).await {
        warn!("[gateway] Failed to send SKILL_ENABLE_RESULT: {}", e);
    }
}

/// Handle SkillDisable - disable a skill
async fn handle_skill_disable(
    skill_name: String,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    if let Err(e) = validate_skill_name(&skill_name) {
        warn!("Invalid skill name rejected: {}", e);
        let result = serde_json::json!({
            "success": false,
            "message": format!("Invalid skill name: {}", e),
        });
        if let Err(e) = send_skill_response("SKILL_DISABLE_RESULT", result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_DISABLE_RESULT: {}", e);
        }
        return;
    }

    info!("🚫 Skill disable requested: {}", skill_name);

    let workspace_dir = std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("Failed to get current directory: {}", e);
        std::path::PathBuf::from(".")
    });
    let skills_base = workspace_dir.join("skills");
    let enabled_file = skills_base.join(&skill_name).join(".enabled");

    // Validate path stays within skills directory
    let skill_dir = skills_base.join(&skill_name);
    if let Err(e) = validate_skill_path(&skill_dir, &skills_base) {
        warn!("Path traversal blocked: {}", e);
        let result = serde_json::json!({
            "success": false,
            "message": format!("Security violation: {}", e),
        });
        if let Err(e) = send_skill_response("SKILL_DISABLE_RESULT", result, session_id, nexus).await
        {
            warn!("[gateway] Failed to send SKILL_DISABLE_RESULT: {}", e);
        }
        return;
    }

    let result = match tokio::fs::remove_file(&enabled_file).await {
        Ok(()) => serde_json::json!({
            "success": true,
            "message": format!("Skill '{}' disabled", skill_name),
        }),
        Err(_) => serde_json::json!({
            "success": true,
            "message": format!("Skill '{}' was already disabled", skill_name),
        }),
    };

    if let Err(e) = send_skill_response("SKILL_DISABLE_RESULT", result, session_id, nexus).await {
        warn!("[gateway] Failed to send SKILL_DISABLE_RESULT: {}", e);
    }
}

/// Handle SkillScan - run security scan on a skill
async fn handle_skill_scan(
    skill_path: String,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) {
    info!("🔍 Skill scan requested: {}", skill_path);

    let path = std::path::Path::new(&skill_path);

    // Validate path is within allowed directories
    let workspace_dir = std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("Failed to get current directory: {}", e);
        std::path::PathBuf::from(".")
    });
    let _skills_base = workspace_dir.join("skills");
    let _workspaces_base = workspace_dir.join("workspaces");

    // GTW-06: Validate path is within allowed directories (skills/ or workspaces/)
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            let result = serde_json::json!({
                "success": false,
                "message": format!("Invalid path: {}", e),
            });
            if let Err(e) =
                send_skill_response("SKILL_SCAN_RESULT", result, session_id, nexus).await
            {
                warn!("[gateway] Failed to send SKILL_SCAN_RESULT: {}", e);
            }
            return;
        }
    };

    let skills_base = workspace_dir
        .join("skills")
        .canonicalize()
        .unwrap_or_default();
    let workspaces_base = workspace_dir
        .join("workspaces")
        .canonicalize()
        .unwrap_or_default();
    if !canonical.starts_with(&skills_base) && !canonical.starts_with(&workspaces_base) {
        let result = serde_json::json!({
            "success": false,
            "message": "Path must be within skills/ or workspaces/ directory",
        });
        if let Err(e) = send_skill_response("SKILL_SCAN_RESULT", result, session_id, nexus).await {
            warn!("[gateway] Failed to send SKILL_SCAN_RESULT: {}", e);
        }
        return;
    }

    let scanner = savant_skills::security::SecurityScanner::new();

    let result = match scanner.scan_skill_mandatory(&canonical).await {
        Ok(scan_result) => serde_json::json!({
            "success": true,
            "skill_name": scan_result.skill_name,
            "risk_level": format!("{:?}", scan_result.risk_level),
            "is_blocked": scan_result.is_blocked,
            "findings": scan_result.findings,
            "content_hash": scan_result.content_hash,
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "message": format!("Scan failed: {}", e),
        }),
    };

    if let Err(e) = send_skill_response("SKILL_SCAN_RESULT", result, session_id, nexus).await {
        warn!("[gateway] Failed to send SKILL_SCAN_RESULT: {}", e);
    }
}

/// Scan a skills directory and collect skill info
async fn scan_skills_directory(
    dir: &std::path::Path,
    scope: &str,
    skills: &mut Vec<serde_json::Value>,
) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            continue;
        }

        let skill_dir = entry.path();
        let skill_md = skill_dir.join("SKILL.md");

        if !skill_md.exists() {
            continue;
        }

        // Read and parse SKILL.md
        if let Ok(content) = tokio::fs::read_to_string(&skill_md).await {
            let name = extract_name_from_skill_md(&content).unwrap_or_else(|| {
                skill_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            });

            let description = extract_description_from_skill_md(&content).unwrap_or_default();

            // Check if enabled
            let enabled = skill_dir.join(".enabled").exists();

            skills.push(serde_json::json!({
                "name": name,
                "description": description,
                "scope": scope,
                "path": skill_dir.to_string_lossy(),
                "enabled": enabled,
            }));
        }
    }
}

/// Extract skill name from SKILL.md frontmatter
fn extract_name_from_skill_md(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim() == "name" {
                return Some(value.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Extract skill description from SKILL.md frontmatter
fn extract_description_from_skill_md(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim() == "description" {
                return Some(value.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Send a skill management response to the requesting session
async fn send_skill_response(
    event_type: &str,
    data: serde_json::Value,
    session_id: &savant_core::types::SessionId,
    nexus: &Arc<NexusBridge>,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "event": event_type,
        "data": data,
    });

    // Publish to session-specific channel (prevents data leak to other sessions)
    let channel = format!("session.{}.{}", session_id.0, event_type.to_lowercase());
    nexus
        .publish(&channel, &payload.to_string())
        .await
        .map_err(|e| format!("Failed to publish skill event: {}", e))?;

    Ok(())
}
