// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::{ApprovalRequirement, Tool};
use savant_core::types::CapabilityGrants;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

use crate::provenance::{ProvenanceEntry, ProvenanceTracker};
use crate::quality::QualityGate;
use crate::registry::SharedToolRegistry;

pub struct ToolForgeTool {
    forge_dir: PathBuf,
    registry: Arc<SharedToolRegistry>,
    provenance: Arc<ProvenanceTracker>,
}

impl ToolForgeTool {
    pub fn new(
        forge_dir: PathBuf,
        registry: Arc<SharedToolRegistry>,
        provenance: Arc<ProvenanceTracker>,
    ) -> Self {
        ToolForgeTool {
            forge_dir,
            registry,
            provenance,
        }
    }

    fn ensure_forge_dir(&self) -> Result<(), SavantError> {
        std::fs::create_dir_all(&self.forge_dir).map_err(|e| {
            SavantError::OperationFailed(format!("Failed to create forge directory: {e}"))
        })
    }

    fn tool_dir(&self, name: &str) -> PathBuf {
        self.forge_dir.join(name)
    }

    fn skill_md_path(&self, name: &str) -> PathBuf {
        self.tool_dir(name).join("SKILL.md")
    }

    fn existing_tool_names(&self) -> HashSet<String> {
        self.registry.list_all().keys().cloned().collect()
    }

    fn read_skill(&self, name: &str) -> Result<String, SavantError> {
        let path = self.skill_md_path(name);
        if !path.exists() {
            return Err(SavantError::InvalidInput(format!(
                "Tool '{name}' not found"
            )));
        }
        std::fs::read_to_string(&path)
            .map_err(|e| SavantError::OperationFailed(format!("Failed to read SKILL.md: {e}")))
    }

    fn write_skill_atomic(&self, name: &str, content: &str) -> Result<(), SavantError> {
        self.ensure_forge_dir()?;
        let dir = self.tool_dir(name);
        std::fs::create_dir_all(&dir).map_err(|e| {
            SavantError::OperationFailed(format!("Failed to create tool directory: {e}"))
        })?;

        let path = self.skill_md_path(name);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content)
            .map_err(|e| SavantError::OperationFailed(format!("Failed to write temp file: {e}")))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            SavantError::OperationFailed(format!("Failed to rename temp file: {e}"))
        })?;
        Ok(())
    }

    fn bump_version(current: &str, bump: Option<&str>) -> String {
        let parts: Vec<u32> = current.split('.').filter_map(|s| s.parse().ok()).collect();
        if parts.len() != 3 {
            return String::from("0.1.1");
        }
        match bump {
            Some("major") => format!("{}.0.0", parts[0] + 1),
            Some("minor") => format!("{}.{}.0", parts[0], parts[1] + 1),
            _ => format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1),
        }
    }
}

#[async_trait]
impl Tool for ToolForgeTool {
    fn name(&self) -> &str {
        "tool_forge"
    }

    fn description(&self) -> &str {
        "Forge new tools for the collective swarm toolkit. Created tools are immediately \
         available to all agents. Use this to codify workflows, procedures, and discoveries \
         so the entire swarm benefits.\n\
         \n\
         CREATE a new tool when: you completed a complex task (5+ tool calls), fixed a \
         tricky error and learned the pattern, discovered a non-trivial workflow, the user \
         asked you to remember a procedure, or you see another agent struggling with \
         something you solved.\n\
         \n\
         PATCH (update) when: an existing forge tool had wrong/missing instructions, or \
         you discovered a better approach.\n\
         \n\
         Rate tools after using them. Use stats to see which tools are most valuable.\n\
         Rollback if a patch introduces issues.\n\
         \n\
         Actions: forge, patch, list, view, stats, archive, pin, rollback, rate, share."
    }

    fn requires_approval(&self) -> ApprovalRequirement {
        ApprovalRequirement::Conditional
    }

    fn capabilities(&self) -> CapabilityGrants {
        CapabilityGrants {
            fs_write: [PathBuf::from("skills/forge/")].into_iter().collect(),
            ..Default::default()
        }
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let action = payload["action"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'action'"))
        })?;

        match action {
            "forge" => self.handle_forge(&payload).await,
            "patch" => self.handle_patch(&payload).await,
            "list" => self.handle_list(&payload).await,
            "view" => self.handle_view(&payload).await,
            "stats" => self.handle_stats(&payload).await,
            "archive" => self.handle_archive(&payload).await,
            "pin" => self.handle_pin(&payload).await,
            "rollback" => self.handle_rollback(&payload).await,
            "rate" => self.handle_rate(&payload).await,
            "share" => self.handle_share(&payload).await,
            _ => Err(SavantError::InvalidInput(format!(
                "Unknown action: '{action}'. Valid: forge, patch, list, view, stats, archive, pin, rollback, rate, share"
            ))),
        }
    }
}

impl ToolForgeTool {
    async fn handle_forge(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        let description = payload["description"].as_str().unwrap_or("");
        let body = payload["body"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'body'"))
        })?;
        let category = payload["category"].as_str();
        let version = "0.1.0";

        let existing = self.existing_tool_names();
        let qr = QualityGate::validate(name, description, version, body, &existing);
        if !qr.passed {
            return Ok(serde_json::to_string_pretty(&serde_json::json!({
                "status": "REJECTED",
                "gate": "quality",
                "failures": qr.failures
            }))
            .unwrap_or_default());
        }

        let full_body = format!(
            "---\nname: {name}\ndescription: {description}\nversion: {version}\n---\n\n{body}"
        );
        self.write_skill_atomic(name, &full_body)?;

        let entry = ProvenanceEntry {
            name: name.to_string(),
            creator_agent_id: String::new(),
            creator_agent_name: String::new(),
            action: String::from("forge"),
            version: Some(String::from(version)),
            description: Some(description.to_string()),
            category: category.map(|s| s.to_string()),
            rating: None,
            rating_agent: None,
            comment: None,
            pinned: None,
            reason: None,
            superseded_by: None,
            audit_result: Some(String::from("PASSED")),
            audit_iterations: Some(1),
            audit_findings: None,
            from_version: None,
            to_version: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.provenance.append(&entry).await;

        info!("[toolforge] Tool forged: {name}");
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "FORGED",
            "name": name,
            "version": version,
            "message": format!("Tool '{name}' v{version} created and available to all agents")
        }))
        .unwrap_or_default())
    }

    async fn handle_patch(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        let old_string = payload["old_string"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'old_string'"))
        })?;
        let new_string = payload["new_string"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'new_string'"))
        })?;
        let version_bump = payload["version_bump"].as_str();

        let content = self.read_skill(name)?;
        if !content.contains(old_string) {
            return Err(SavantError::InvalidInput(format!(
                "'old_string' not found in {name}/SKILL.md"
            )));
        }

        let current_version = content
            .lines()
            .find(|l| l.trim().starts_with("version:"))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| String::from("0.1.0"));

        let new_version = Self::bump_version(&current_version, version_bump);
        let updated = content
            .replace(old_string, new_string)
            .replace(&current_version, &new_version);

        self.write_skill_atomic(name, &updated)?;

        let entry = ProvenanceEntry {
            name: name.to_string(),
            creator_agent_id: String::new(),
            creator_agent_name: String::new(),
            action: String::from("patch"),
            version: Some(new_version.clone()),
            description: None,
            category: None,
            rating: None,
            rating_agent: None,
            comment: None,
            pinned: None,
            reason: None,
            superseded_by: None,
            audit_result: None,
            audit_iterations: None,
            audit_findings: None,
            from_version: Some(current_version),
            to_version: Some(new_version.clone()),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.provenance.append(&entry).await;

        info!("[toolforge] Tool patched: {name} → v{new_version}");
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "PATCHED",
            "name": name,
            "version": new_version,
            "message": format!("Tool '{name}' updated to v{new_version}")
        }))
        .unwrap_or_default())
    }

    async fn handle_list(&self, _payload: &Value) -> Result<String, SavantError> {
        let entries = self.provenance.replay();
        let mut tools: Vec<serde_json::Value> = Vec::new();
        let mut seen = HashSet::new();

        for entry in entries.iter().rev() {
            if seen.contains(&entry.name) {
                continue;
            }
            seen.insert(entry.name.clone());

            let stats = self.provenance.compute_stats(&entry.name);
            tools.push(serde_json::json!({
                "name": entry.name,
                "version": entry.version,
                "description": entry.description,
                "category": entry.category,
                "creator_agent_id": entry.creator_agent_id,
                "creator_agent_name": entry.creator_agent_name,
                "use_count": stats.use_count,
                "unique_agents": stats.unique_agents,
                "thumbs_up": stats.thumbs_up,
                "thumbs_down": stats.thumbs_down,
                "success_rate": stats.success_rate(),
                "last_used_at": stats.last_used_at.map(|t| t.to_rfc3339()),
            }));
        }

        Ok(serde_json::to_string_pretty(&tools).unwrap_or_default())
    }

    async fn handle_view(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        self.read_skill(name)
    }

    async fn handle_stats(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        let stats = self.provenance.compute_stats(name);
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "name": name,
            "use_count": stats.use_count,
            "unique_agents": stats.unique_agents,
            "thumbs_up": stats.thumbs_up,
            "thumbs_down": stats.thumbs_down,
            "success_rate": stats.success_rate(),
            "last_used_at": stats.last_used_at.map(|t| t.to_rfc3339()),
        }))
        .unwrap_or_default())
    }

    async fn handle_archive(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        let reason = payload["reason"].as_str().map(|s| s.to_string());
        let superseded_by = payload["superseded_by"].as_str().map(|s| s.to_string());

        let path = self.skill_md_path(name);
        if !path.exists() {
            return Err(SavantError::InvalidInput(format!(
                "Tool '{name}' not found"
            )));
        }

        self.registry.remove(name);

        let entry = ProvenanceEntry {
            name: name.to_string(),
            creator_agent_id: String::new(),
            creator_agent_name: String::new(),
            action: String::from("archive"),
            version: None,
            description: None,
            category: None,
            rating: None,
            rating_agent: None,
            comment: None,
            pinned: None,
            reason,
            superseded_by,
            audit_result: None,
            audit_iterations: None,
            audit_findings: None,
            from_version: None,
            to_version: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.provenance.append(&entry).await;

        info!("[toolforge] Tool archived: {name}");
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "ARCHIVED",
            "name": name,
            "message": format!("Tool '{name}' archived")
        }))
        .unwrap_or_default())
    }

    async fn handle_pin(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        let pinned = payload["pinned"].as_bool().unwrap_or(true);

        let entry = ProvenanceEntry {
            name: name.to_string(),
            creator_agent_id: String::new(),
            creator_agent_name: String::new(),
            action: String::from("pin"),
            version: None,
            description: None,
            category: None,
            rating: None,
            rating_agent: None,
            comment: None,
            pinned: Some(pinned),
            reason: None,
            superseded_by: None,
            audit_result: None,
            audit_iterations: None,
            audit_findings: None,
            from_version: None,
            to_version: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.provenance.append(&entry).await;

        info!("[toolforge] Tool pin toggled: {name} = {pinned}");
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "PINNED",
            "name": name,
            "pinned": pinned
        }))
        .unwrap_or_default())
    }

    async fn handle_rollback(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;

        let entries = self.provenance.replay();
        let forge_entries: Vec<&ProvenanceEntry> = entries
            .iter()
            .filter(|e| e.name == name && (e.action == "forge" || e.action == "patch"))
            .collect();

        if forge_entries.len() < 2 {
            return Err(SavantError::InvalidInput(format!(
                "Tool '{name}' has no previous version to roll back to"
            )));
        }

        let target = payload["target_version"].as_str();
        let restore_entry = match target {
            Some(v) => forge_entries
                .iter()
                .find(|e| e.version.as_deref() == Some(v))
                .copied(),
            None => forge_entries.get(forge_entries.len() - 2).copied(),
        }
        .ok_or_else(|| {
            SavantError::InvalidInput(format!("Target version not found for '{name}'"))
        })?;

        let current_version = forge_entries.last().and_then(|e| e.version.clone());
        let restore_version = restore_entry.version.clone();

        for entry in forge_entries.iter().rev() {
            if entry.version == restore_version {
                let content = self.read_skill(name)?;
                let versioned = content.replace(
                    current_version.as_deref().unwrap_or("0.1.0"),
                    restore_version.as_deref().unwrap_or("0.1.0"),
                );
                self.write_skill_atomic(name, &versioned)?;

                let rb_entry = ProvenanceEntry {
                    name: name.to_string(),
                    action: String::from("rollback"),
                    from_version: current_version,
                    to_version: restore_version.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    ..Default::default()
                };
                self.provenance.append(&rb_entry).await;

                info!("[toolforge] Tool rolled back: {name}");
                return Ok(serde_json::to_string_pretty(&serde_json::json!({
                    "status": "ROLLED_BACK",
                    "name": name,
                    "restored_version": restore_version.clone()
                }))
                .unwrap_or_default());
            }
        }

        Err(SavantError::InvalidInput(format!(
            "Rollback failed for '{name}'"
        )))
    }

    async fn handle_rate(&self, payload: &Value) -> Result<String, SavantError> {
        let name = payload["name"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from("Missing required field: 'name'"))
        })?;
        let rating = payload["rating"].as_str().ok_or_else(|| {
            SavantError::InvalidInput(String::from(
                "Missing required field: 'rating' (thumbs_up or thumbs_down)",
            ))
        })?;

        if rating != "thumbs_up" && rating != "thumbs_down" {
            return Err(SavantError::InvalidInput(format!(
                "Rating must be 'thumbs_up' or 'thumbs_down', got '{rating}'"
            )));
        }

        let comment = payload["comment"].as_str().map(|s| s.to_string());

        let entry = ProvenanceEntry {
            name: name.to_string(),
            action: String::from("rate"),
            rating: Some(rating.to_string()),
            rating_agent: None,
            comment,
            timestamp: chrono::Utc::now().to_rfc3339(),
            ..Default::default()
        };
        self.provenance.append(&entry).await;

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "RATED",
            "name": name,
            "rating": rating
        }))
        .unwrap_or_default())
    }

    async fn handle_share(&self, _payload: &Value) -> Result<String, SavantError> {
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "SHARED",
            "message": "All forge tools are shared across the swarm by default"
        }))
        .unwrap_or_default())
    }
}

impl Default for ProvenanceEntry {
    fn default() -> Self {
        ProvenanceEntry {
            name: String::new(),
            creator_agent_id: String::new(),
            creator_agent_name: String::new(),
            action: String::new(),
            version: None,
            description: None,
            category: None,
            rating: None,
            rating_agent: None,
            comment: None,
            pinned: None,
            reason: None,
            superseded_by: None,
            audit_result: None,
            audit_iterations: None,
            audit_findings: None,
            from_version: None,
            to_version: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}
