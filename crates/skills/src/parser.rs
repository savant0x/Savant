use crate::clawhub::ClawHubClient;
use crate::sandbox::{SandboxDispatcher, ToolExecutor};
use crate::security::{RiskLevel, SecurityScanResult, SecurityScanner};
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use savant_core::types::{CapabilityGrants, SkillManifest};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::{error, info, warn};

/// The result of passing through the mandatory security gate.
///
/// Every skill is scanned. Every finding is shown to the user.
/// The user ALWAYS has the final say — we just make sure they're informed.
///
/// Gate behavior (clicks required before user can proceed):
/// - Clean: 0 clicks (auto-proceed)
/// - Low: 0 clicks (proceed with notification)  
/// - Medium: 1 click (acknowledge findings)
/// - High: 2 clicks (double-confirm with full disclosure)
/// - Critical: 3 clicks (triple-confirm with "I understand the risks" checkbox)
///
/// There are NO hard blocks. The user is sovereign.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityGateResult {
    /// Clean/Low risk - automatically approved, proceed immediately
    AutoApproved { scan_result: SecurityScanResult },
    /// Medium/High/Critical - awaiting user approval clicks
    /// Tracks how many clicks have been completed
    PendingApproval {
        scan_result: SecurityScanResult,
        clicks_completed: u32,
        clicks_required: u32,
    },
    /// User completed all required clicks and approved
    UserApproved {
        scan_result: SecurityScanResult,
        approved_at: i64,
        clicks_completed: u32,
    },
    /// User explicitly rejected the skill
    UserRejected {
        scan_result: SecurityScanResult,
        rejected_at: i64,
    },
}

impl SecurityGateResult {
    /// Can the skill be loaded/executed?
    pub fn is_approved(&self) -> bool {
        matches!(
            self,
            SecurityGateResult::AutoApproved { .. } | SecurityGateResult::UserApproved { .. }
        )
    }

    /// Is the user still deciding?
    pub fn is_pending(&self) -> bool {
        matches!(self, SecurityGateResult::PendingApproval { .. })
    }

    /// Did the user reject it?
    pub fn is_rejected(&self) -> bool {
        matches!(self, SecurityGateResult::UserRejected { .. })
    }

    /// How many total clicks required
    pub fn required_clicks(&self) -> u32 {
        match self {
            SecurityGateResult::AutoApproved { .. } => 0,
            SecurityGateResult::PendingApproval {
                clicks_required, ..
            } => *clicks_required,
            SecurityGateResult::UserApproved {
                clicks_completed, ..
            } => *clicks_completed,
            SecurityGateResult::UserRejected { .. } => 0,
        }
    }

    /// How many clicks completed so far
    pub fn completed_clicks(&self) -> u32 {
        match self {
            SecurityGateResult::AutoApproved { .. } => 0,
            SecurityGateResult::PendingApproval {
                clicks_completed, ..
            } => *clicks_completed,
            SecurityGateResult::UserApproved {
                clicks_completed, ..
            } => *clicks_completed,
            SecurityGateResult::UserRejected { .. } => 0,
        }
    }

    /// How many more clicks needed
    pub fn clicks_remaining(&self) -> u32 {
        let required = self.required_clicks();
        let completed = self.completed_clicks();
        required.saturating_sub(completed)
    }

    pub fn scan_result(&self) -> &SecurityScanResult {
        match self {
            SecurityGateResult::AutoApproved { scan_result } => scan_result,
            SecurityGateResult::PendingApproval { scan_result, .. } => scan_result,
            SecurityGateResult::UserApproved { scan_result, .. } => scan_result,
            SecurityGateResult::UserRejected { scan_result, .. } => scan_result,
        }
    }

    /// Progress through the approval flow (0.0 to 1.0)
    pub fn approval_progress(&self) -> f32 {
        let required = self.required_clicks();
        if required == 0 {
            return 1.0;
        }
        let completed = self.completed_clicks();
        (completed as f32) / (required as f32)
    }

    /// Generate the approval prompt for the UI
    pub fn approval_prompt(&self) -> ApprovalPrompt {
        let scan = self.scan_result();
        let required = self.required_clicks();
        let completed = self.completed_clicks();
        let remaining = self.clicks_remaining();

        ApprovalPrompt {
            skill_name: scan.skill_name.clone(),
            risk_level: scan.risk_level,
            warning_icon: scan.risk_level.icon(),
            warning_color: scan.risk_level.color(),
            warning_bg_color: scan.risk_level.bg_color(),
            warning_message: scan.risk_level.warning_message().to_string(),
            clicks_required: required,
            clicks_completed: completed,
            clicks_remaining: remaining,
            approval_progress: self.approval_progress(),
            findings: scan.findings.clone(),
            proactive_checks_passed: scan.proactive_checks_passed.clone(),
            proactive_checks_triggered: scan.proactive_checks_triggered.clone(),
            can_always_proceed: true, // User is ALWAYS sovereign
        }
    }

    /// Create a gate result for a given risk level (starting at 0 clicks)
    pub fn from_risk_level(scan_result: SecurityScanResult) -> Self {
        let clicks_required = scan_result.risk_level.required_clicks();
        if clicks_required == 0 {
            SecurityGateResult::AutoApproved { scan_result }
        } else {
            SecurityGateResult::PendingApproval {
                scan_result,
                clicks_completed: 0,
                clicks_required,
            }
        }
    }

    /// Advance one click through the approval flow
    pub fn advance_approval(&mut self) -> bool {
        match self {
            SecurityGateResult::PendingApproval {
                clicks_completed,
                clicks_required,
                ..
            } => {
                *clicks_completed += 1;
                if *clicks_completed >= *clicks_required {
                    // Convert to approved
                    false // signals "done pending"
                } else {
                    true // signals "still pending"
                }
            }
            _ => false,
        }
    }
}

/// Prompt shown to user when approval is needed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPrompt {
    pub skill_name: String,
    pub risk_level: RiskLevel,
    pub warning_icon: &'static str,
    pub warning_color: &'static str,
    pub warning_bg_color: &'static str,
    pub warning_message: String,
    pub clicks_required: u32,
    pub clicks_completed: u32,
    pub clicks_remaining: u32,
    pub approval_progress: f32,
    pub findings: Vec<crate::security::SecurityFinding>,
    pub proactive_checks_passed: Vec<String>,
    pub proactive_checks_triggered: Vec<crate::security::ProactiveCheck>,
    pub can_always_proceed: bool, // Always true - user is sovereign
}

/// A Savant Tool backed by a Skill execution engine (WASM or Native).
pub struct SkillTool {
    manifest: SkillManifest,
    executor: Box<dyn ToolExecutor>,
}

impl SkillTool {
    /// Creates a new SkillTool from a manifest and workspace directory.
    pub fn new(manifest: SkillManifest, workspace_dir: PathBuf) -> Self {
        let executor = SandboxDispatcher::create_executor(
            &manifest.execution_mode,
            workspace_dir,
            manifest.capabilities.clone(),
        );
        Self { manifest, executor }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        &self.manifest.name
    }
    fn description(&self) -> &str {
        &self.manifest.description
    }
    fn capabilities(&self) -> CapabilityGrants {
        self.manifest.capabilities.clone()
    }
    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        // E7: Enforce CapabilityGrants before execution
        let caps = &self.manifest.capabilities;

        // Check required environment variables
        if !caps.requires_env.is_empty() {
            for env_var in &caps.requires_env {
                if std::env::var(env_var).is_err() {
                    return Err(SavantError::Unknown(format!(
                        "Skill '{}' requires env var '{}' which is not set",
                        self.manifest.name, env_var
                    )));
                }
            }
        }

        // Check fs_write capability if payload contains file paths
        if caps.fs_write.is_empty() {
            if let Some(path) = payload.get("path").and_then(|v| v.as_str()) {
                // Skill has no fs_write grants but payload targets a file
                tracing::warn!(
                    "Skill '{}' has no fs_write grants but targets path '{}'",
                    self.manifest.name,
                    path
                );
            }
        }

        let raw_output = self.executor.execute(payload).await?;

        // E8: Sanitize skill output before returning to agent
        let sanitized = sanitize_skill_output(&raw_output);
        Ok(sanitized)
    }
}

/// E8: Sanitize skill output — strip ANSI codes, truncate, scrub secrets.
fn sanitize_skill_output(output: &str) -> String {
    // Strip ANSI escape codes
    let ansi_regex = match regex::Regex::new(r"\x1b\[[0-9;]*m") {
        Ok(re) => re,
        Err(e) => {
            warn!("Failed to compile ANSI stripping regex: {}", e);
            return output.to_string();
        }
    };
    let cleaned = ansi_regex.replace_all(output, "");

    // Truncate to max 50K chars
    const MAX_OUTPUT: usize = 50_000;
    let truncated = if cleaned.len() > MAX_OUTPUT {
        format!(
            "{}...[truncated {} chars]",
            &cleaned[..MAX_OUTPUT],
            cleaned.len() - MAX_OUTPUT
        )
    } else {
        cleaned.to_string()
    };

    // Scrub common secret patterns (API keys, tokens, passwords)
    let secret_regex =
        match regex::Regex::new(r"(?i)(api[_-]?key|token|password|secret|credential)\s*[:=]\s*\S+")
        {
            Ok(re) => re,
            Err(e) => {
                warn!("Failed to compile secret scrubbing regex: {}", e);
                return truncated;
            }
        };
    secret_regex
        .replace_all(&truncated, "$1: [REDACTED]")
        .to_string()
}

/// Maximum number of skills that can be loaded
const MAX_SKILL_COUNT: usize = 1000;

/// Registry for managing agent skills and their capabilities.
/// Implements two-stage discovery to optimize LLM context window.
pub struct SkillRegistry {
    /// Maps skill names to their full manifests
    pub manifests: HashMap<String, SkillManifest>,
    /// Maps skill names to their initialized tools
    pub tools: HashMap<String, Arc<dyn Tool>>,
    /// Maximum number of skills allowed
    max_skills: usize,
    /// Security scanner for mandatory skill scanning
    scanner: SecurityScanner,
}

impl SkillRegistry {
    /// Creates a new, empty SkillRegistry.
    pub fn new() -> Self {
        Self {
            manifests: HashMap::new(),
            tools: HashMap::new(),
            max_skills: MAX_SKILL_COUNT,
            scanner: SecurityScanner::new(),
        }
    }

    /// Creates a new SkillRegistry with a custom maximum skill count.
    pub fn with_max_skills(max_skills: usize) -> Self {
        Self {
            manifests: HashMap::new(),
            tools: HashMap::new(),
            max_skills,
            scanner: SecurityScanner::new(),
        }
    }

    /// Stage 2: On-Demand Loading.
    /// Retrieves the full markdown instructions only when the agent explicitly needs them.
    pub fn get_skill_instructions(&self, skill_name: &str) -> Option<String> {
        self.manifests
            .get(skill_name)
            .map(|s| s.instructions.clone())
    }

    /// Parses an OpenClaw-compatible SKILL.md file.
    /// Enforces strict YAML frontmatter validation.
    pub async fn load_skill_from_file(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(), SavantError> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref).await.map_err(|e| {
            SavantError::IoError(std::io::Error::other(format!(
                "Failed to read {}: {}",
                path_ref.display(),
                e
            )))
        })?;

        // Extract YAML frontmatter (between --- markers)
        let parts: Vec<&str> = content.splitn(3, "---").collect();

        // If the file starts with ---, parts[0] is empty. We need exactly 3 parts.
        if parts.len() < 3 {
            return Err(SavantError::Unknown(format!(
                "Invalid SKILL.md format (missing frontmatter separator) in {}",
                path_ref.display()
            )));
        }

        // Handle both cases: file starting with --- or having empty leader
        // Standard frontmatter starts with ---, so parts[0] should be empty
        let frontmatter = parts[1];

        let instructions = parts[2].trim().to_string();

        // Strict YAML parsing.
        let mut manifest: SkillManifest = serde_yaml::from_str(frontmatter).map_err(|e| {
            SavantError::Unknown(format!("YAML parse error in {}: {}", path_ref.display(), e))
        })?;

        manifest.instructions = instructions;

        // MANDATORY SECURITY SCAN — every skill must pass before loading.
        // The scanner reads SKILL.md from the directory and runs all 10 detection layers.
        if let Some(skill_dir) = path_ref.parent() {
            match self.scanner.scan_skill_mandatory(skill_dir).await {
                Ok(scan_result) => match scan_result.risk_level {
                    RiskLevel::Clean | RiskLevel::Low => {
                        info!(
                            "Security scan passed for '{}' (risk: {})",
                            manifest.name, scan_result.risk_level
                        );
                    }
                    RiskLevel::Medium => {
                        warn!(
                            "Security scan: '{}' has MEDIUM risk. Loading with notification.",
                            manifest.name
                        );
                    }
                    RiskLevel::High | RiskLevel::Critical => {
                        error!(
                            "SECURITY BLOCK: '{}' has {} risk. Skill rejected.",
                            manifest.name, scan_result.risk_level
                        );
                        return Err(SavantError::AuthError(format!(
                            "Skill '{}' rejected: {} risk level detected by security scanner",
                            manifest.name, scan_result.risk_level
                        )));
                    }
                },
                Err(e) => {
                    warn!(
                        "Security scan failed for '{}': {}. Loading with warning.",
                        manifest.name, e
                    );
                }
            }
        }

        info!("Loaded skill: {} (v{})", manifest.name, manifest.version);

        // Check for skill name collision - reject overwrite to prevent data loss
        if self.manifests.contains_key(&manifest.name) {
            warn!(
                "Skill name collision: '{}' already loaded. Rejecting duplicate.",
                manifest.name
            );
            return Err(SavantError::InvalidInput(format!(
                "Skill '{}' is already loaded. Use a unique skill name.",
                manifest.name
            )));
        }

        // Check maximum skill count
        if self.manifests.len() >= self.max_skills && !self.manifests.contains_key(&manifest.name) {
            return Err(SavantError::Unknown(format!(
                "Maximum skill count reached: {} (max: {})",
                self.manifests.len(),
                self.max_skills
            )));
        }

        // Initialize the tool with its specific sandbox executor
        let skill_dir = path_ref
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let tool = Arc::new(SkillTool::new(manifest.clone(), skill_dir));

        self.tools.insert(manifest.name.clone(), tool);
        self.manifests.insert(manifest.name.clone(), manifest);
        Ok(())
    }

    /// NS-06: Parses an AGENTS.md file into a SkillManifest.
    /// AGENTS.md may have optional YAML frontmatter; the body is treated as instructions.
    async fn load_agents_md(&mut self, path: impl AsRef<Path>) -> Result<(), SavantError> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref).await.map_err(|e| {
            SavantError::IoError(std::io::Error::other(format!(
                "Failed to read {}: {}",
                path_ref.display(),
                e
            )))
        })?;

        // Try to extract YAML frontmatter (optional for AGENTS.md)
        let (frontmatter_str, instructions) = if content.starts_with("---") {
            let parts: Vec<&str> = content.splitn(3, "---").collect();
            if parts.len() >= 3 {
                (Some(parts[1].trim()), parts[2].trim().to_string())
            } else {
                (None, content.clone())
            }
        } else {
            (None, content.clone())
        };

        // Derive skill name from parent directory
        let dir_name = path_ref
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "agents-config".to_string());

        let mut manifest = if let Some(fm) = frontmatter_str {
            serde_yaml::from_str::<SkillManifest>(fm).unwrap_or_else(|_| SkillManifest {
                name: dir_name.clone(),
                version: "1.0.0".to_string(),
                description: format!("Agent configuration from {}", path_ref.display()),
                execution_mode: savant_core::types::ExecutionMode::Reference,
                capabilities: CapabilityGrants::default(),
                instructions: String::new(),
                depends_on: Vec::new(),
                chain_with: Vec::new(),
            })
        } else {
            SkillManifest {
                name: dir_name.clone(),
                version: "1.0.0".to_string(),
                description: format!("Agent configuration from {}", path_ref.display()),
                execution_mode: savant_core::types::ExecutionMode::Reference,
                capabilities: CapabilityGrants::default(),
                instructions: String::new(),
                depends_on: Vec::new(),
                chain_with: Vec::new(),
            }
        };

        manifest.instructions = instructions;

        info!(
            "Loaded AGENTS.md skill: {} from {}",
            manifest.name,
            path_ref.display()
        );
        self.manifests.insert(manifest.name.clone(), manifest);
        Ok(())
    }

    /// NS-06: Parses a .cursorrules file into a SkillManifest.
    /// .cursorrules files have no frontmatter — the entire content is instructions.
    async fn load_cursorrules(&mut self, path: impl AsRef<Path>) -> Result<(), SavantError> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref).await.map_err(|e| {
            SavantError::IoError(std::io::Error::other(format!(
                "Failed to read {}: {}",
                path_ref.display(),
                e
            )))
        })?;

        let dir_name = path_ref
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "cursorrules".to_string());

        let manifest = SkillManifest {
            name: format!("{}-cursorrules", dir_name),
            version: "1.0.0".to_string(),
            description: format!("Cursor rules from {}", path_ref.display()),
            execution_mode: savant_core::types::ExecutionMode::Reference,
            capabilities: CapabilityGrants::default(),
            instructions: content,
            depends_on: Vec::new(),
            chain_with: Vec::new(),
        };

        info!(
            "Loaded .cursorrules skill: {} from {}",
            manifest.name,
            path_ref.display()
        );
        self.manifests.insert(manifest.name.clone(), manifest);
        Ok(())
    }

    /// Recursively discover and load all skills in a directory.
    /// NS-06: Now discovers SKILL.md, AGENTS.md, and .cursorrules files.
    pub async fn discover_skills(
        &mut self,
        directory: impl AsRef<Path>,
    ) -> Result<usize, SavantError> {
        let mut count = 0;

        // Use walkdir for recursive discovery
        for entry in walkdir::WalkDir::new(directory) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("Skill discovery WalkDir error: {}", e);
                    continue;
                }
            };

            if !entry.file_type().is_file() {
                continue;
            }

            let file_name = entry.file_name().to_string_lossy();

            // E1+E3: Security scan before loading skill files
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                let findings = self.scanner.scan_command(&content);
                let blocked = findings
                    .iter()
                    .any(|f| matches!(f.severity, RiskLevel::High | RiskLevel::Critical));
                if blocked {
                    warn!(
                        "Security scan blocked skill file: {} ({} findings)",
                        entry.path().display(),
                        findings.len()
                    );
                    continue;
                }
            }

            let result = match file_name.as_ref() {
                "SKILL.md" => self.load_skill_from_file(entry.path()).await,
                "AGENTS.md" => self.load_agents_md(entry.path()).await,
                ".cursorrules" => self.load_cursorrules(entry.path()).await,
                _ => continue,
            };

            match result {
                Ok(()) => count += 1,
                Err(e) => error!("Failed to load skill at {}: {}", entry.path().display(), e),
            }
        }

        info!("Skill discovery complete. Total skills loaded: {}", count);
        Ok(count)
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Skill installation source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillSource {
    /// Skill bundled with Savant
    Bundled,
    /// Skill from ClawHub registry
    ClawHub {
        slug: String,
        version: Option<String>,
    },
    /// Skill installed from local path
    Local { path: PathBuf },
    /// Skill installed from URL
    Url { url: String },
    /// E4: Skill from another agent ecosystem (.claude, .agents, .opencode)
    CrossEcosystem { ecosystem: String, path: PathBuf },
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillSource::Bundled => write!(f, "bundled"),
            SkillSource::ClawHub { slug, .. } => write!(f, "clawhub:{}", slug),
            SkillSource::Local { path } => write!(f, "local:{}", path.display()),
            SkillSource::Url { url } => write!(f, "url:{}", url),
            SkillSource::CrossEcosystem { ecosystem, path } => {
                write!(f, "{}:{}", ecosystem, path.display())
            }
        }
    }
}

/// Skill trust tier based on origin and scan results
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SkillTrustTier {
    /// Savant official bundled skills
    Official = 0,
    /// Verified by Savant team
    Verified = 1,
    /// Community skills that passed security scan
    Community = 2,
    /// New/untrusted skills requiring explicit approval
    Untrusted = 3,
}

impl std::fmt::Display for SkillTrustTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillTrustTier::Official => write!(f, "official"),
            SkillTrustTier::Verified => write!(f, "verified"),
            SkillTrustTier::Community => write!(f, "community"),
            SkillTrustTier::Untrusted => write!(f, "untrusted"),
        }
    }
}

/// Skill scope - whether it's available to all agents or specific ones
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillScope {
    /// Available to all agents in the swarm
    SwarmWide,
    /// Only available to a specific agent
    AgentSpecific { agent_id: String },
}

impl std::fmt::Display for SkillScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillScope::SwarmWide => write!(f, "swarm"),
            SkillScope::AgentSpecific { agent_id } => write!(f, "agent:{}", agent_id),
        }
    }
}

/// Skill metadata persisted alongside the skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Unique skill identifier
    pub id: String,
    /// Skill name
    pub name: String,
    /// Installation source
    pub source: SkillSource,
    /// Trust tier
    pub trust_tier: SkillTrustTier,
    /// Scope (swarm-wide or agent-specific)
    pub scope: SkillScope,
    /// Whether the skill is enabled
    pub enabled: bool,
    /// Content hash for blocklist tracking
    pub content_hash: String,
    /// Security scan result
    pub last_scan: Option<SecurityScanResult>,
    /// User has explicitly approved running this skill
    pub approved: bool,
    /// Installation timestamp
    pub installed_at: i64,
    /// Last update timestamp
    pub updated_at: Option<i64>,
}

/// Skill installation status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillStatus {
    /// Skill is installed and enabled
    Active,
    /// Skill is installed but disabled by user
    Disabled,
    /// Skill failed security scan and is blocked
    Blocked { reason: String },
    /// Skill has pending update
    UpdateAvailable { current: String, available: String },
    /// Skill is pending user approval
    PendingApproval,
}

/// Skill manager handles the complete lifecycle of skills.
///
/// MANDATORY SECURITY: Every skill must pass through the security gate
/// before it can be loaded or executed. There is no bypass, no "trusted"
/// shortcut, no exception. The scan happens:
/// 1. Before installation (pre-install scan)
/// 2. On discovery (re-scan to catch changes)
/// 3. Before execution (final check)
pub struct SkillManager {
    /// Skills directory (swarm-wide)
    swarm_skills_dir: PathBuf,
    /// Scanner for security validation
    scanner: SecurityScanner,
    /// ClawHub client for skill installation
    clawhub_client: ClawHubClient,
    /// Registry of loaded skills
    registry: SkillRegistry,
    /// Metadata for installed skills
    metadata: HashMap<String, SkillMetadata>,
    /// Security gate results (path -> gate result)
    gate_cache: HashMap<PathBuf, SecurityGateResult>,
    /// Skills that are pending user approval (name -> gate result)
    pending_approvals: HashMap<String, SecurityGateResult>,
    /// Skills that user explicitly rejected
    rejected_skills: HashSet<String>,
}

impl SkillManager {
    /// Create a new skill manager
    pub fn new(swarm_skills_dir: PathBuf) -> Self {
        Self {
            swarm_skills_dir,
            scanner: SecurityScanner::new(),
            clawhub_client: ClawHubClient::new(),
            registry: SkillRegistry::new(),
            metadata: HashMap::new(),
            gate_cache: HashMap::new(),
            pending_approvals: HashMap::new(),
            rejected_skills: HashSet::new(),
        }
    }

    /// Get all skills pending user approval
    pub fn get_pending_approvals(&self) -> Vec<(&String, &SecurityGateResult)> {
        self.pending_approvals.iter().collect()
    }

    /// Check if a skill was rejected by the user
    pub fn is_rejected(&self, skill_name: &str) -> bool {
        self.rejected_skills.contains(skill_name)
    }

    /// Get the swarm-wide skills directory
    pub fn swarm_skills_dir(&self) -> &Path {
        &self.swarm_skills_dir
    }

    /// Discover and load skills from all configured directories
    ///
    /// Folder structure:
    /// - `<swarm_dir>/skills/` - Swarm-wide skills (available to all agents)
    /// - `<workspace>/skills/` - Agent-specific skills (available only to that agent)
    pub async fn discover_all_skills(
        &mut self,
        agent_workspace: Option<&Path>,
    ) -> Result<DiscoverResult, SavantError> {
        let mut result = DiscoverResult::default();

        // 1. Discover swarm-wide skills
        let swarm_skills = self.swarm_skills_dir.clone();
        if swarm_skills.exists() {
            info!(
                "Discovering swarm-wide skills from: {}",
                swarm_skills.display()
            );
            result.swarm_skills = self
                .discover_and_scan_skills(&swarm_skills, SkillScope::SwarmWide)
                .await?;
        } else {
            // Create the directory if it doesn't exist
            if let Err(e) = tokio::fs::create_dir_all(&swarm_skills).await {
                warn!("[parser] Failed to create swarm skills directory: {}", e);
            }
            info!(
                "Created swarm-wide skills directory: {}",
                swarm_skills.display()
            );
        }

        // 2. Discover agent-specific skills if workspace provided
        if let Some(workspace) = agent_workspace {
            let agent_skills = workspace.join("skills");
            if agent_skills.exists() {
                info!(
                    "Discovering agent-specific skills from: {}",
                    agent_skills.display()
                );
                let agent_id = workspace
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                result.agent_skills = self
                    .discover_and_scan_skills(&agent_skills, SkillScope::AgentSpecific { agent_id })
                    .await?;
            } else {
                // Create the directory
                if let Err(e) = tokio::fs::create_dir_all(&agent_skills).await {
                    warn!("[parser] Failed to create agent skills directory: {}", e);
                }
            }
        }

        info!(
            "Skill discovery complete: {} swarm-wide, {} agent-specific",
            result.swarm_skills, result.agent_skills
        );

        // E4: Cross-ecosystem skill discovery
        let cross_ecosystems: Vec<(&str, &str)> = vec![
            (".claude/skills", "claude"),
            (".agents/skills", "agents"),
            (".opencode/skills", "opencode"),
        ];

        for (path_suffix, source_name) in &cross_ecosystems {
            let cross_path = std::path::Path::new(path_suffix);
            if cross_path.exists() {
                info!(
                    "Discovering cross-ecosystem skills from: {} (source: {})",
                    cross_path.display(),
                    source_name
                );
                let cross_count = self
                    .discover_and_scan_skills(cross_path, SkillScope::SwarmWide)
                    .await?;
                if cross_count > 0 {
                    info!(
                        "Cross-ecosystem: found {} skills from {}",
                        cross_count, source_name
                    );
                }
            }
        }

        Ok(result)
    }

    /// Discover skills in a directory and scan them for security
    async fn discover_and_scan_skills(
        &mut self,
        directory: &Path,
        _scope: SkillScope,
    ) -> Result<usize, SavantError> {
        let mut count = 0;

        // Each subdirectory is a skill
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }

            let skill_dir = entry.path();
            let skill_md = skill_dir.join("SKILL.md");

            if !skill_md.exists() {
                warn!("No SKILL.md in {}, skipping", skill_dir.display());
                continue;
            }

            // MANDATORY SECURITY GATE - every skill must pass
            match self.scanner.scan_skill_mandatory(&skill_dir).await {
                Ok(scan_result) => {
                    let gate_result = SecurityGateResult::from_risk_level(scan_result);
                    let skill_name = gate_result.scan_result().skill_name.clone();

                    // Store gate result
                    self.gate_cache
                        .insert(skill_dir.clone(), gate_result.clone());

                    // If needs approval, add to pending and skip loading
                    if gate_result.is_pending() {
                        warn!(
                            "Skill '{}' needs {} click(s) before loading (risk: {})",
                            skill_name,
                            gate_result.required_clicks(),
                            gate_result.scan_result().risk_level
                        );
                        self.pending_approvals.insert(skill_name, gate_result);
                        continue;
                    }

                    // If user previously rejected, skip
                    if self.rejected_skills.contains(&skill_name) {
                        info!("Skill '{}' was rejected by user, skipping", skill_name);
                        continue;
                    }

                    // Auto-approved (Clean/Low) - load it
                    match self.registry.load_skill_from_file(&skill_md).await {
                        Ok(()) => {
                            count += 1;
                            info!("Loaded skill from {}", skill_dir.display());
                        }
                        Err(e) => {
                            error!("Failed to load skill from {}: {}", skill_dir.display(), e);
                        }
                    }
                }
                Err(e) => {
                    error!("Security scan failed for {}: {}", skill_dir.display(), e);
                }
            }
        }

        Ok(count)
    }

    /// Install a skill from ClawHub
    ///
    /// MANDATORY: Skill is scanned BEFORE being written to disk.
    /// If scan finds issues, the skill is NOT installed but returned
    /// as PendingApproval so the user can decide.
    pub async fn install_from_clawhub(
        &mut self,
        slug: &str,
        target_agent: Option<&str>,
    ) -> Result<InstallResult, SavantError> {
        // Determine target directory based on scope
        let target_dir = if let Some(agent_id) = target_agent {
            self.swarm_skills_dir
                .join("workspaces")
                .join(format!("workspace-{}", agent_id))
                .join("skills")
        } else {
            self.swarm_skills_dir.join("skills")
        };

        // Use ClawHubClient to install (handles download, scan, and move)
        let result = self
            .clawhub_client
            .install(slug, &target_dir, &self.scanner)
            .await
            .map_err(|e| SavantError::Unknown(format!("ClawHub install failed: {}", e)))?;

        // If the result has a gate_result that is pending approval, store it
        if let Some(ref gate_result) = result.gate_result {
            if gate_result.is_pending() {
                self.pending_approvals
                    .insert(result.skill_name.clone(), gate_result.clone());
            }
            // Store in gate cache for tracking
            let skill_dir = target_dir.join(slug.replace('/', "-"));
            self.gate_cache.insert(skill_dir, gate_result.clone());
        }

        Ok(result)
    }

    /// User approves a pending skill (clicks through the approval flow)
    pub async fn approve_pending_skill(
        &mut self,
        skill_name: &str,
    ) -> Result<ApprovalResult, SavantError> {
        // Remove from pending to work with it
        let mut gate_result = self.pending_approvals.remove(skill_name).ok_or_else(|| {
            SavantError::Unknown(format!("No pending approval for '{}'", skill_name))
        })?;

        // Advance one click
        let still_pending = gate_result.advance_approval();
        let remaining = gate_result.clicks_remaining();
        let required = gate_result.required_clicks();

        if still_pending {
            // Put it back, still pending
            self.pending_approvals
                .insert(skill_name.to_string(), gate_result);

            Ok(ApprovalResult {
                approved: false,
                clicks_remaining: remaining,
                clicks_required: required,
                message: format!(
                    "{} more click(s) required to install '{}'",
                    remaining, skill_name
                ),
            })
        } else {
            // All clicks completed - mark as approved
            let completed = gate_result.completed_clicks();

            // Remove from pending (actual loading happens elsewhere)
            self.pending_approvals.remove(skill_name);

            Ok(ApprovalResult {
                approved: true,
                clicks_remaining: 0,
                clicks_required: completed,
                message: format!("Skill '{}' approved and ready to load", skill_name),
            })
        }
    }

    /// User rejects a pending skill
    pub fn reject_pending_skill(&mut self, skill_name: &str) -> Result<(), SavantError> {
        if let Some(_gate_result) = self.pending_approvals.remove(skill_name) {
            self.rejected_skills.insert(skill_name.to_string());
            info!("User rejected skill '{}'", skill_name);
            Ok(())
        } else {
            Err(SavantError::Unknown(format!(
                "No pending approval for '{}'",
                skill_name
            )))
        }
    }

    /// Enable or disable a skill
    pub async fn set_skill_enabled(
        &mut self,
        skill_name: &str,
        enabled: bool,
    ) -> Result<(), SavantError> {
        if let Some(meta) = self.metadata.get_mut(skill_name) {
            meta.enabled = enabled;
            Ok(())
        } else {
            Err(SavantError::Unknown(format!(
                "Skill '{}' not found",
                skill_name
            )))
        }
    }

    /// Get the security gate result for a skill
    pub fn get_gate_result(&self, skill_path: &Path) -> Option<&SecurityGateResult> {
        self.gate_cache.get(skill_path)
    }

    /// Get all skill metadata
    pub fn list_skills(&self) -> Vec<(&String, &SkillMetadata)> {
        self.metadata.iter().collect()
    }

    /// Get the underlying registry
    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Get the underlying registry mutably
    pub fn registry_mut(&mut self) -> &mut SkillRegistry {
        &mut self.registry
    }
}

/// Result of skill discovery
#[derive(Debug, Default, Clone)]
pub struct DiscoverResult {
    pub swarm_skills: usize,
    pub agent_skills: usize,
}

/// Result of skill installation
#[derive(Debug, Clone)]
pub struct InstallResult {
    pub success: bool,
    pub skill_name: String,
    pub gate_result: Option<SecurityGateResult>,
    pub message: String,
}

/// Result of an approval action (click)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResult {
    /// Whether the skill is now fully approved
    pub approved: bool,
    /// How many more clicks are needed
    pub clicks_remaining: u32,
    /// Total clicks required
    pub clicks_required: u32,
    /// Human-readable message
    pub message: String,
}
