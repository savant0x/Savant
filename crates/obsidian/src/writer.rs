use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use tracing::info;

use savant_memory::audit::AuditTrail;
use savant_memory::engine::MemoryEnclave;
use savant_memory::lessons::{Insight, Lesson};
use savant_memory::models::{AgentMessage, MemoryEntry, MessageRole};
use savant_memory::procedural::ProceduralMemory;
use savant_memory::promotion::EbbinghausScorer;

use crate::config::ObsidianConfig;
use crate::error::VaultError;

const MAX_CONTENT_PREVIEW: usize = 400;

pub struct VaultWriter {
    vault_path: PathBuf,
    enclave: Option<Arc<MemoryEnclave>>,
    config: ObsidianConfig,
    agent_name: String,
}

impl VaultWriter {
    pub fn new(
        vault_path: PathBuf,
        enclave: Option<Arc<MemoryEnclave>>,
        config: ObsidianConfig,
        agent_name: String,
    ) -> Self {
        Self {
            vault_path,
            enclave,
            config,
            agent_name,
        }
    }

    pub async fn ensure_structure(&self) -> Result<(), VaultError> {
        let vault_path = self.vault_path.clone();
        tokio::task::spawn_blocking(move || {
            let dirs = [
                vault_path.join(".obsidian"),
                vault_path.join("Episodic"),
                vault_path.join("Semantic"),
                vault_path.join("Identity").join("Evolution"),
                vault_path.join("Themes"),
                vault_path.join("Working"),
                vault_path.join("Dashboard"),
                vault_path.join(".stale"),
                vault_path.join("Delegation"),
                vault_path.join("Procedural"),
                vault_path.join("Lessons"),
                vault_path.join("Insights"),
                vault_path.join("Graphs").join("Temporal"),
                vault_path.join("Graphs").join("Causal"),
                vault_path.join("Graphs").join("Entity"),
                vault_path.join("Retention"),
                vault_path.join("Audit"),
                vault_path.join("Multimodal"),
            ];
            for dir in &dirs {
                fs::create_dir_all(dir)?;
            }
            Ok::<(), VaultError>(())
        })
        .await
        .map_err(|e| VaultError::Config(e.to_string()))??;

        let appearance = self.vault_path.join(".obsidian").join("appearance.json");
        if !appearance.exists() {
            let json = "{\"accentColor\":\"#00FFBB\",\"baseTheme\":\"obsidian\",\"interfaceFontFamily\":\"Inter\",\"textFontFamily\":\"Inter\",\"monospaceFontFamily\":\"JetBrains Mono\",\"translucency\":false,\"native\":false,\"enabledCssSnippets\":[],\"cssTheme\":\"\"}";
            atomic_write(&appearance, json).await?;
        }

        let stale_gi = self.vault_path.join(".stale").join(".gitignore");
        if !stale_gi.exists() {
            atomic_write(&stale_gi, "*\n").await?;
        }
        Ok(())
    }

    // ─── INDEX ────────────────────────────────────────────────────────────

    pub async fn write_index(&self, stats: &VaultStats) -> Result<(), VaultError> {
        let today = Utc::now().format("%Y-%m-%d");
        let t = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let content = format!(
            "# {name}'s Memory Tree\n\n\
             > *Last synced: {t}*\n\
             > *Agent stage: {stage} | Evolution score: {score} | \
             {sessions} sessions, {memories} memories, {vectors} vectors*\n\n\
             ---\n\n\
             ## Episodic\n\n\
             Recent: [[Episodic/{today}]] | All sessions stored in LSM+HNSW\n\n\
             ## Semantic\n\n\
             Concepts: [[Semantic/Concepts]] | Relations: [[Semantic/Relations]]\n\
             Triplets: [[Semantic/Triplets]] | Entities: [[Semantic/Entities]]\n\n\
             ## Identity\n\n\
             SOUL: [[Identity/SOUL]] | Personality: [[Identity/Personality]]\n\
             Evolution: [[Identity/Evolution]]\n\n\
             ## Knowledge\n\n\
             Procedures: [[Procedural/INDEX|Procedures]] | \
             Lessons: [[Lessons/INDEX|Lessons]] | \
             Insights: [[Insights/INDEX|Insights]]\n\n\
             ## Graphs\n\n\
             Temporal: [[Graphs/Temporal/INDEX|Temporal]] | \
             Causal: [[Graphs/Causal/INDEX|Causal]] | \
             Entity: [[Graphs/Entity/INDEX|Entity]]\n\n\
             ## Memory Health\n\n\
             Retention: [[Retention/INDEX|Retention Tiers]] | \
             Themes: [[Themes/INDEX|Themes]]\n\n\
             ## Dashboard\n\n\
             Recent: [[Dashboard/Recent]] | Health: [[Dashboard/Health]]\n\
             Audit: [[Dashboard/Audit]] | Notifications: [[Dashboard/Notifications]]\n\n\
             ---\n\n\
             *This vault is a bidirectional projection of Savant's LSM+HNSW memory substrate. \
             Edits to Semantic/, Identity/, Procedural/, Lessons/, and Insights/ are synced \
             back to the agent. Edits to Episodic/ are logged as correction nodes. \
             Edits to Graphs/, Retention/, Audit/, and Dashboard/ are rejected.*\n",
            name = self.agent_name,
            t = t,
            today = today,
            stage = stats.stage,
            score = stats.evolution_score,
            sessions = stats.session_count,
            memories = stats.memory_count,
            vectors = stats.vector_count,
        );
        atomic_write(&self.vault_path.join("INDEX.md"), &content).await
    }

    // ─── EPISODIC ─────────────────────────────────────────────────────────

    pub async fn write_episodic(&self, date: &NaiveDate) -> Result<(), VaultError> {
        let filename = format!("{}.md", date.format("%Y-%m-%d"));
        let path = self.vault_path.join("Episodic").join(&filename);

        let date_start = date
            .and_hms_opt(0, 0, 0)
            .map(|dt| dt.and_utc().timestamp())
            .unwrap_or(0);
        let date_end = date_start + 86400;

        let mut content = format!(
            "# Episodic — {date}\n\n> Daily transcript. Append-only. \
             Edited at {ts}.\n\n---\n\n",
            date = date.format("%Y-%m-%d"),
            ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
        );

        let mut session_map: BTreeMap<String, Vec<AgentMessage>> = BTreeMap::new();
        if let Some(enclave) = &self.enclave {
            let lsm = enclave.lsm();
            for sid in lsm.session_keys() {
                let msgs = lsm.fetch_session_tail(&sid, 200);
                let day_msgs: Vec<AgentMessage> = msgs
                    .into_iter()
                    .filter(|m| {
                        let ts: i64 = m.timestamp.into();
                        ts >= date_start && ts < date_end
                    })
                    .collect();
                if !day_msgs.is_empty() {
                    session_map.insert(sid.clone(), day_msgs);
                }
            }
        }

        if session_map.is_empty() {
            content.push_str("*No sessions recorded for this date.*\n");
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!("**{n} session(s)**\n\n", n = session_map.len()));

        for (sid, msgs) in &session_map {
            content.push_str(&format!(
                "### Session: `{sid}`\n\n| Role | Content |\n|------|--------|\n"
            ));
            for msg in msgs {
                let role = match msg.role {
                    MessageRole::User => "**You**",
                    MessageRole::Assistant => "**Agent**",
                    MessageRole::Tool => "*Tool*",
                    MessageRole::System => "System",
                };
                let truncated: String = msg.content.chars().take(MAX_CONTENT_PREVIEW).collect();
                let ellipsis = if msg.content.len() > MAX_CONTENT_PREVIEW {
                    "…"
                } else {
                    ""
                };
                let escaped = truncate_to_line(truncated.trim()).replace('|', "\\|");
                content.push_str(&format!("| {role} | {escaped}{ellipsis} |\n"));
                for tc in &msg.tool_calls {
                    content.push_str(&format!(
                        "| → | `{name}(…{args}…)` |\n",
                        name = tc.tool_name,
                        args = truncate_to_line(&tc.arguments.chars().take(80).collect::<String>()),
                    ));
                }
            }
            content.push('\n');
        }

        atomic_write(&path, &content).await
    }

    // ─── SEMANTIC ─────────────────────────────────────────────────────────

    pub async fn write_concepts(&self) -> Result<(), VaultError> {
        let path = self.vault_path.join("Semantic").join("Concepts.md");
        let mut content = String::from(
            "# Concept Graph\n\n> Concepts are distilled from agent interactions. \
             [[Semantic/Relations]] | [[Semantic/Entities]] | [[Semantic/Triplets]]\n\n---\n\n",
        );

        let mut by_category: BTreeMap<String, Vec<MemoryEntry>> = BTreeMap::new();
        if let Some(enclave) = &self.enclave {
            if let Ok(entries) = enclave.lsm().iter_metadata() {
                for entry in entries {
                    let cat = if entry.category.is_empty() {
                        "uncategorized".to_string()
                    } else {
                        entry.category.clone()
                    };
                    by_category.entry(cat).or_default().push(entry);
                }
            } else {
                tracing::warn!("[obsidian] iter_metadata failed in write_concepts");
            }
        }

        if by_category.is_empty() {
            content.push_str("*No concepts extracted yet. Concepts are generated by the DistillationPipeline during background consolidation.*\n");
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str("## Concepts by Category\n\n");
        for (cat, entries) in &by_category {
            content.push_str(&format!("### {cat}\n\n| Concept | Importance | Mentions | Source |\n|--------|-----------|----------|--------|\n"));
            for entry in entries {
                let snippet: String = entry.content.chars().take(100).collect();
                let ellipsis = if entry.content.len() > 100 { "…" } else { "" };
                let anchor = slugify(&snippet.chars().take(30).collect::<String>());
                content.push_str(&format!(
                    "| [[Semantic/Concepts#{anchor}|{snippet}{ellipsis}]] | {imp} | {hits} | [[Episodic/]] |\n",
                    anchor = anchor,
                    snippet = truncate_to_line(&snippet),
                    imp = entry.importance,
                    hits = u32::from(entry.hit_count),
                ));
            }
            content.push('\n');
        }

        atomic_write(&path, &content).await
    }

    pub async fn write_relations(&self) -> Result<(), VaultError> {
        let path = self.vault_path.join("Semantic").join("Relations.md");
        let mut content = String::from(
            "# Relation Graph\n\n> Typed edges between concepts. \
             [[Semantic/Concepts]] | [[Semantic/Entities]] | [[Semantic/Triplets]]\n\n---\n\n",
        );

        let mut relation_count = 0usize;
        if let Some(enclave) = &self.enclave {
            if let Ok(entries) = enclave.lsm().iter_metadata() {
                for entry in &entries {
                    if !entry.related_to.is_empty() {
                        relation_count += entry.related_to.len();
                    }
                }
            } else {
                tracing::warn!("[obsidian] iter_metadata failed in write_relations (count)");
            }
        }

        if relation_count == 0 {
            content.push_str("*No relations discovered yet. Relations emerge from shared session context and the DistillationPipeline.*\n");
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(
            "## Ontology\n\n| Category | Relation Types |\n|----------|---------------|\n\
                          | Hierarchical | is_a, part_of, subclass_of |\n\
                          | Social | works_for, knows, founded, advises |\n\
                          | Temporal | superseded_by, evolved_into, prior_state |\n\
                          | Epistemic | contradicts, supports, derived_from |\n\
                          | Operational | requires, generates, modifies |\n\n",
        );

        if let Some(enclave) = &self.enclave {
            if let Ok(entries) = enclave.lsm().iter_metadata() {
                content.push_str("## Active Edges\n\n| Source | Relation | Target |\n|--------|----------|--------|\n");
                for entry in &entries {
                    for rel_id in &entry.related_to {
                        let rel_u64: u64 = (*rel_id).into();
                        if let Ok(Some(target)) = enclave.lsm().get_metadata(rel_u64) {
                            let src_snippet: String = entry.content.chars().take(60).collect();
                            let tgt_snippet: String = target.content.chars().take(60).collect();
                            content.push_str(&format!(
                                "| {src}… | related_to | {tgt}… |\n",
                                src = truncate_to_line(&src_snippet),
                                tgt = truncate_to_line(&tgt_snippet),
                            ));
                        }
                    }
                }
            } else {
                tracing::warn!("[obsidian] iter_metadata failed in write_relations (edges)");
            }
        }

        atomic_write(&path, &content).await
    }

    pub async fn write_triplets(&self) -> Result<(), VaultError> {
        let path = self.vault_path.join("Semantic").join("Triplets.md");
        let mut content = String::from(
            "# Distilled Knowledge (Triplets)\n\n> Subject-predicate-object extractions. \
             [[Semantic/Concepts]] | [[Semantic/Relations]] | [[Semantic/Entities]]\n\n---\n\n",
        );

        if let Some(enclave) = &self.enclave {
            let facts = enclave.lsm().iter_facts();
            if !facts.is_empty() {
                content.push_str("| Subject | Predicate | Object | Confidence |\n");
                content.push_str("|---------|-----------|--------|------------|\n");
                for (subj, pred, obj, conf) in &facts {
                    content.push_str(&format!("| {subj} | {pred} | {obj} | {conf:.2} |\n"));
                }
                content.push('\n');
                content.push_str(&format!(
                    "**{n} triplets** extracted by the DistillationPipeline.\n",
                    n = facts.len()
                ));
            } else {
                content.push_str("*No triplets extracted yet. Triplets are generated by the DistillationPipeline.*\n");
            }
        } else {
            content.push_str("*No triplets extracted yet. Triplets are generated by the DistillationPipeline.*\n");
        }

        atomic_write(&path, &content).await
    }

    pub async fn write_entities(&self) -> Result<(), VaultError> {
        let path = self.vault_path.join("Semantic").join("Entities.md");
        let mut content = String::from(
            "# Entity Catalog\n\n> People, projects, services, and tools tracked across sessions. \
             [[Semantic/Concepts]] | [[Semantic/Relations]]\n\n---\n\n",
        );

        let mut entities: BTreeMap<String, (u32, u32, i64, i64)> = BTreeMap::new();
        if let Some(enclave) = &self.enclave {
            let lsm = enclave.lsm();
            for sid in lsm.session_keys() {
                let msgs = lsm.fetch_session_tail(&sid, 100);
                for msg in &msgs {
                    extract_entities_from_text(&msg.content, &mut entities);
                    for tc in &msg.tool_calls {
                        extract_entities_from_text(&tc.tool_name, &mut entities);
                        extract_entities_from_text(&tc.arguments, &mut entities);
                    }
                }
            }
        }

        if entities.is_empty() {
            content.push_str("*No entities extracted yet. Entity extraction uses Schema.org/FOAF patterns with LLM fallback.*\n");
        } else {
            content.push_str("| Entity | Type | Mentions | Sessions | First Seen | Last Seen |\n");
            content.push_str("|--------|------|----------|----------|------------|-----------|\n");
            for (name, (count, sessions, first, last)) in &entities {
                let first_dt = chrono::DateTime::from_timestamp(*first, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                let last_dt = chrono::DateTime::from_timestamp(*last, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                content.push_str(&format!(
                    "| {name} | entity | {count} | {sessions} | {first_dt} | {last_dt} |\n"
                ));
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── IDENTITY ─────────────────────────────────────────────────────────

    pub async fn write_soul(&self, workspace_root: &Path) -> Result<(), VaultError> {
        let target = self.vault_path.join("Identity").join("SOUL.md");
        let source = workspace_root.join("SOUL.md");
        if source.exists() {
            atomic_write(&target, &fs::read_to_string(&source)?).await?;
        } else {
            let content = format!(
                "# SOUL.md — {name}\n\n> No SOUL.md found. Use the Evolution system \
                 to generate one.\n\n## Terminal Mantra\n\nOperate with precision, \
                 security, and autonomy.\n",
                name = self.agent_name,
            );
            atomic_write(&target, &content).await?;
        }
        Ok(())
    }

    pub async fn write_personality(&self, workspace_root: &Path) -> Result<(), VaultError> {
        let path = self.vault_path.join("Identity").join("Personality.md");
        let agent_json_path = workspace_root.join("agent.json");

        let mut o = "—".to_string();
        let mut c = "—".to_string();
        let mut e = "—".to_string();
        let mut a = "—".to_string();
        let mut n = "—".to_string();

        if agent_json_path.exists() {
            if let Ok(content) = fs::read_to_string(&agent_json_path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(pt) = val.get("personality_traits") {
                        o = pt
                            .get("openness")
                            .and_then(|v| v.as_f64())
                            .map(|v| format!("{:.2}", v))
                            .unwrap_or_else(|| o);
                        c = pt
                            .get("conscientiousness")
                            .and_then(|v| v.as_f64())
                            .map(|v| format!("{:.2}", v))
                            .unwrap_or_else(|| c);
                        e = pt
                            .get("extraversion")
                            .and_then(|v| v.as_f64())
                            .map(|v| format!("{:.2}", v))
                            .unwrap_or_else(|| e);
                        a = pt
                            .get("agreeableness")
                            .and_then(|v| v.as_f64())
                            .map(|v| format!("{:.2}", v))
                            .unwrap_or_else(|| a);
                        n = pt
                            .get("neuroticism")
                            .and_then(|v| v.as_f64())
                            .map(|v| format!("{:.2}", v))
                            .unwrap_or_else(|| n);
                    }
                }
            }
        }

        let content = format!(
            "# Personality — OCEAN Traits\n\n> **{name}** — [[Identity/Evolution]] | [[Identity/SOUL]]\n\n\
             ---\n\n## Current Scores\n\n| Trait | Score | Range |\n|-------|-------|-------|\n\
             | **Openness** | {o} | 0.0–1.0 |\n\
             | **Conscientiousness** | {c} | 0.0–1.0 |\n\
             | **Extraversion** | {e} | 0.0–1.0 |\n\
             | **Agreeableness** | {a} | 0.0–1.0 |\n\
             | **Neuroticism** | {n} | 0.0–1.0 |\n\n\
             Personality is managed through the Evolution system. Mutations approved \
             via the dashboard update these traits.\n",
            name = self.agent_name, o = o, c = c, e = e, a = a, n = n,
        );
        atomic_write(&path, &content).await
    }

    pub async fn write_evolution_index(&self, workspace_root: &Path) -> Result<(), VaultError> {
        let path = self
            .vault_path
            .join("Identity")
            .join("Evolution")
            .join("INDEX.md");
        let evolution_path = workspace_root.join("EVOLUTION.jsonl");
        let agent_json_path = workspace_root.join("agent.json");

        let mutations: Vec<serde_json::Value> = if evolution_path.exists() {
            fs::read_to_string(&evolution_path)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        } else {
            Vec::new()
        };

        let (stage, score, _mutation_count) = if agent_json_path.exists() {
            fs::read_to_string(&agent_json_path)
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                .and_then(|v| v.get("evolution_state").cloned())
                .map(|es| {
                    let count = es
                        .get("mutation_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let score = es
                        .get("evolution_score")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0) as f32;
                    let stage = es
                        .get("stage")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Seedling")
                        .to_string();
                    (stage, score, count)
                })
                .unwrap_or_else(|| ("Seedling".to_string(), 0.0, 0))
        } else {
            ("Seedling".to_string(), 0.0, 0)
        };

        let approved_count = mutations
            .iter()
            .filter(|m| m.get("status").and_then(|v| v.as_str()) == Some("approved"))
            .count();

        let mut content = format!(
            "# Evolution Timeline\n\n> **{name}** \u{2014} [[Identity/Personality]] | [[Identity/SOUL]]\n\n             ---\n\n## Status\n\n| Metric | Value |\n|--------|-------|
             | **Stage** | {stage} |\n| **Score** | {score:.2} |\n             | **Total Mutations** | {total} |\n             | **Approved** | {approved} |\n             | **Pending** | {pending} |\n\n",
            name = self.agent_name,
            stage = stage,
            score = score,
            total = mutations.len(),
            approved = approved_count,
            pending = mutations.len() - approved_count,
        );

        let reports_dir = self
            .vault_path
            .join("Identity")
            .join("Evolution")
            .join("reports");
        if let Err(e) = fs::create_dir_all(&reports_dir) {
            tracing::warn!(
                "[obsidian] Failed to create evolution reports directory: {}",
                e
            );
        }
        for m in &mutations {
            if let Err(e) = self.write_mutation_report(m, &reports_dir).await {
                tracing::warn!("[obsidian] Failed to write mutation report: {}", e);
            }
        }

        if mutations.is_empty() {
            content.push_str("*No mutations recorded. The Evolution system proposes SOUL.md edits based on interaction patterns.*\n");
        } else {
            content.push_str("## Timeline\n\n| Date | Type | Section | Status | Evidence |\n|------|------|---------|--------|----------|\n");
            for m in &mutations {
                let ts = m.get("proposed_at").and_then(|v| v.as_i64()).unwrap_or(0);
                let date = if ts > 0 {
                    chrono::DateTime::from_timestamp(ts / 1000, 0)
                        .map(|d| d.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "\u{2014}".to_string())
                } else {
                    "\u{2014}".to_string()
                };
                let mtype = m
                    .get("mutation_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("\u{2014}");
                let section = m
                    .get("target_section")
                    .and_then(|v| v.as_str())
                    .unwrap_or("\u{2014}");
                let status = m
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pending");
                let report_id = m.get("mutation_id").and_then(|v| v.as_str()).unwrap_or("");
                let evidence_count = m
                    .get("source_evidence")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let evidence_link = if evidence_count > 0 {
                    format!("[[reports/{}|{} evidence]]", report_id, evidence_count)
                } else {
                    "\u{2014}".to_string()
                };
                content.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    date, mtype, section, status, evidence_link
                ));
            }
        }

        atomic_write(&path, &content).await
    }

    async fn write_mutation_report(
        &self,
        mutation: &serde_json::Value,
        reports_dir: &Path,
    ) -> Result<(), VaultError> {
        let report_id = mutation
            .get("mutation_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let path = reports_dir.join(format!("{}.md", report_id));

        let mtype = mutation
            .get("mutation_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let section = mutation
            .get("target_section")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let status = mutation
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        let confidence = mutation
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let reasoning = mutation
            .get("reasoning")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let proposed = mutation
            .get("proposed_content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let before = mutation
            .get("before_content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let ts = mutation
            .get("proposed_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let date = if ts > 0 {
            chrono::DateTime::from_timestamp(ts / 1000, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "\u{2014}".to_string())
        } else {
            "\u{2014}".to_string()
        };

        let evidence: Vec<String> = mutation
            .get("source_evidence")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let mut content = format!(
            "# Mutation Report \u{2014} {report_id}\n\n             > **Type:** {mtype} | **Section:** {section} | **Status:** {status} | **Confidence:** {confidence:.2}\n             > **Proposed:** {date}\n\n             ---\n\n             ## Reasoning\n\n             {reasoning}\n\n",
            report_id = report_id, mtype = mtype, section = section, status = status,
            confidence = confidence, date = date, reasoning = reasoning,
        );

        if !before.is_empty() {
            content.push_str("## Diff\n\n```diff\n");
            content.push_str(&format!("- {}\n", before.replace('\n', "\n- ")));
            content.push_str(&format!("+ {}\n", proposed.replace('\n', "\n+ ")));
            content.push_str("```\n\n");
        } else {
            content.push_str("## Proposed Content\n\n");
            content.push_str(proposed);
            content.push_str("\n\n");
        }

        if !evidence.is_empty() {
            content.push_str("## Evidence\n\n");
            for ev in &evidence {
                content.push_str(&format!("- [[Episodic/{}]]\n", ev));
            }
            content.push('\n');
        }

        content.push_str("## Navigation\n\n[[Identity/Evolution]] | [[Identity/SOUL]] | [[Identity/Personality]]\n");

        atomic_write(&path, &content).await
    }

    pub async fn write_themes_index(&self) -> Result<(), VaultError> {
        let path = self.vault_path.join("Themes").join("INDEX.md");
        let content = "# Themes — Cross-Domain Patterns\n\n> Concept clusters discovered by the Dream Engine \
             during REM cycles. [[Semantic/Concepts]] | [[Semantic/Relations]]\n\n---\n\n\
             *Themes are generated during Dream REM consolidation. The engine explores HNSW latent \
             space to find disparate concept nodes, abstracts them into higher-level themes, and ranks \
             clusters by Vendi Score. No clusters found yet.*\n".to_string();
        atomic_write(&path, &content).await
    }

    // ─── WORKING ──────────────────────────────────────────────────────────

    pub async fn clear_working(&self) -> Result<(), VaultError> {
        let working = self.vault_path.join("Working");
        let working_clone = working.clone();
        tokio::task::spawn_blocking(move || {
            if working_clone.exists() {
                for entry in fs::read_dir(&working_clone)? {
                    let path = entry?.path();
                    if path.is_file() {
                        fs::remove_file(&path)?;
                    }
                }
            }
            Ok::<(), VaultError>(())
        })
        .await
        .map_err(|e| VaultError::Config(e.to_string()))??;
        let readme = working.join("README.md");
        atomic_write(
            &readme,
            "# Working\n\n> Transient scratchpad. Cleared on task completion.\n",
        )
        .await
    }

    // ─── DASHBOARD ────────────────────────────────────────────────────────

    pub async fn write_dashboard_recent(&self) -> Result<(), VaultError> {
        let path = self.vault_path.join("Dashboard").join("Recent.md");
        let today = Utc::now().format("%Y-%m-%d");
        let _cutoff = Utc::now().timestamp() - 86400;
        let mut session_count = 0u64;
        let mut msg_count = 0u64;

        if let Some(enclave) = &self.enclave {
            let lsm = enclave.lsm();
            let recent = lsm.iter_recent_messages(24);
            let mut sessions_seen: Vec<String> = Vec::new();
            for msg in &recent {
                msg_count += 1;
                if !sessions_seen.contains(&msg.session_id) {
                    sessions_seen.push(msg.session_id.clone());
                }
            }
            session_count = sessions_seen.len() as u64;
        }

        let content = format!(
            "# Recent Activity\n\n> Last 24 hours — {date}\n> [[Dashboard/Health]]\n\n---\n\n\
             ## Summary\n\n| Metric | Value |\n|--------|-------|\n\
             | **Active Sessions** | {sessions} |\n| **Messages** | {messages} |\n\n\
             Full session data: [[Episodic/{date}]]\n",
            date = today,
            sessions = session_count,
            messages = msg_count,
        );
        atomic_write(&path, &content).await
    }

    pub async fn write_dashboard_health(&self, stats: &VaultStats) -> Result<(), VaultError> {
        let path = self.vault_path.join("Dashboard").join("Health.md");
        let content = format!(
            "# Memory Health\n\n> **{name}** — [[Dashboard/Recent]]\n\n---\n\n\
             ## Storage\n\n| Metric | Value |\n|--------|-------|\n\
             | **Sessions** | {sessions} |\n| **Messages** | {memories} |\n\
             | **Vectors** | {vectors} |\n| **Vault Files** | {files} |\n\
             | **Stage** | {stage} |\n\n## Projection\n\n| Metric | Value |\n\
             |--------|-------|\n| **Sync Interval** | {sync}s |\n\
             | **Max Files** | {max} |\n| **Cold Storage** | >{cold}d |\n\
             | **Last Sync** | {ts} |\n\n\
             *Memory substrate: CortexaDB LSM + ruvector-core HNSW. \
             The vault is a read-write projection of the canonical store.*\n",
            name = self.agent_name,
            sessions = stats.session_count,
            memories = stats.memory_count,
            vectors = stats.vector_count,
            files = stats.vault_file_count,
            stage = stats.stage,
            sync = self.config.sync_interval_secs,
            max = self.config.max_files,
            cold = self.config.cold_storage_days,
            ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
        );
        atomic_write(&path, &content).await
    }

    /// Writes a delegation artifact to the vault as a markdown file.
    ///
    /// Creates a structured markdown document under `Delegation/` containing:
    /// - Task metadata (task_id, parent_agent, target_agent, priority, deadline)
    /// - Artifact parts (text, JSON data, file references) rendered as sections
    /// - Task state and timing information
    ///
    /// This provides the Obsidian vault as a secondary rich communication path
    /// for inter-agent delegation results, complementing the fast shared-memory
    /// A2A protocol.
    #[allow(clippy::too_many_arguments)]
    pub async fn write_delegation_artifact(
        &self,
        task_id: &str,
        parent_agent_id: &str,
        target_agent_id: &str,
        artifact: &savant_ipc::a2a::protocol::Artifact,
        parts: &[savant_ipc::a2a::protocol::ArtifactPart],
        state: savant_ipc::a2a::protocol::TaskState,
        token_budget: u32,
        priority: u8,
        deadline_timestamp: u64,
    ) -> Result<(), VaultError> {
        let delegation_dir = self.vault_path.join("Delegation");
        fs::create_dir_all(&delegation_dir)?;

        let path = delegation_dir.join(format!("{}.md", task_id));
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

        let deadline_str = if deadline_timestamp > 0 {
            let dt = chrono::DateTime::from_timestamp(deadline_timestamp as i64 / 1000, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "Expired".to_string());
            dt
        } else {
            "No deadline".to_string()
        };

        let mut content = format!(
            "# Delegation Artifact — {task_id}\n\n\
             > **State:** {state} | **Created:** {now}\n\n\
             ---\n\n\
             ## Task Metadata\n\n\
             | Field | Value |\n\
             |-------|-------|\n\
             | **Task ID** | `{task_id}` |\n\
             | **Parent Agent** | `{parent_agent_id}` |\n\
             | **Target Agent** | `{target_agent_id}` |\n\
             | **Priority** | {priority} |\n\
             | **Token Budget** | {token_budget} |\n\
             | **Deadline** | {deadline} |\n\n\
             ## Artifact Parts\n\n\
             **{part_count} part(s)**\n\n",
            task_id = task_id,
            state = state,
            now = now,
            parent_agent_id = parent_agent_id,
            target_agent_id = target_agent_id,
            priority = priority,
            token_budget = token_budget,
            deadline = deadline_str,
            part_count = artifact.part_count,
        );

        for (idx, part) in parts.iter().enumerate() {
            let type_name = match part.part_type {
                savant_ipc::a2a::protocol::ArtifactPartType::Text => "Text",
                savant_ipc::a2a::protocol::ArtifactPartType::Json => "JSON",
                savant_ipc::a2a::protocol::ArtifactPartType::FileReference => "File Reference",
            };
            content.push_str(&format!(
                "### Part {} — {} (offset: {}, len: {})\n\n",
                idx + 1,
                type_name,
                part.data_offset,
                part.data_len
            ));

            match part.part_type {
                savant_ipc::a2a::protocol::ArtifactPartType::Text => {
                    content.push_str(&format!(
                        "```text\n[Text content at shared memory offset {} — {} bytes]\n```\n\n",
                        part.data_offset, part.data_len
                    ));
                }
                savant_ipc::a2a::protocol::ArtifactPartType::Json => {
                    content.push_str(&format!(
                        "```json\n[JSON data at shared memory offset {} — {} bytes]\n```\n\n",
                        part.data_offset, part.data_len
                    ));
                }
                savant_ipc::a2a::protocol::ArtifactPartType::FileReference => {
                    content.push_str(&format!(
                        "[File reference at shared memory offset {} — {} bytes]\n\n",
                        part.data_offset, part.data_len
                    ));
                }
            }
        }

        content.push_str("## Navigation\n\n[[Dashboard/Recent]] | [[Dashboard/Health]]\n");

        atomic_write(&path, &content).await?;

        info!(
            task_id = %task_id,
            parts = artifact.part_count,
            "Delegation artifact written to vault"
        );
        Ok(())
    }

    // ─── PROCEDURAL (GH-02) ───────────────────────────────────────────────

    pub async fn write_procedures(
        &self,
        procedures: &[ProceduralMemory],
    ) -> Result<(), VaultError> {
        let path = self.vault_path.join("Procedural").join("INDEX.md");
        let mut content = String::from(
            "# Learned Procedures\n\n\
             > Procedures extracted from recurring tool-call patterns across sessions. \
             Each procedure represents a validated workflow the agent has learned.\n\n---\n\n",
        );

        if procedures.is_empty() {
            content.push_str(
                "*No procedures learned yet. Procedures emerge when the agent observes \
                 the same tool-call sequence 3+ times across different sessions.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        // Sort by strength descending
        let mut sorted: Vec<&ProceduralMemory> = procedures.iter().collect();
        sorted.sort_by(|a, b| {
            b.strength
                .partial_cmp(&a.strength)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        content.push_str(&format!("**{} procedure(s)**\n\n", sorted.len()));

        // Write INDEX with summary table
        content.push_str("| Procedure | Trigger | Steps | Frequency | Strength |\n");
        content.push_str("|-----------|---------|-------|-----------|----------|\n");
        for proc in &sorted {
            let name_link = format!("[[Procedural/{}|{}]]", slugify(&proc.name), proc.name);
            let steps_count = proc.steps.len();
            let strength_bar = strength_bar(proc.strength);
            content.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                name_link,
                truncate_to_line(&proc.trigger_condition),
                steps_count,
                proc.frequency,
                strength_bar,
            ));
        }

        // Write individual procedure files
        for proc in &sorted {
            let proc_path = self
                .vault_path
                .join("Procedural")
                .join(format!("{}.md", slugify(&proc.name)));
            let mut proc_content = format!(
                "# {}\n\n\
                 > **Frequency:** {} | **Strength:** {} | **Trigger:** {}\n\n---\n\n\
                 ## Steps\n\n",
                proc.name,
                proc.frequency,
                strength_bar(proc.strength),
                proc.trigger_condition,
            );

            for step in &proc.steps {
                let critical_tag = if step.critical {
                    " ⚠️ critical"
                } else {
                    ""
                };
                proc_content.push_str(&format!(
                    "{}. `{}`{}\n",
                    step.index + 1,
                    step.action,
                    critical_tag,
                ));
            }

            proc_content.push_str(&format!(
                "\n## Source Sessions\n\n{}\n\n\
                 ## Tags\n\n{}\n",
                if proc.source_sessions.is_empty() {
                    "*None recorded*".to_string()
                } else {
                    proc.source_sessions
                        .iter()
                        .map(|s| format!("- `{}`", s))
                        .collect::<Vec<_>>()
                        .join("\n")
                },
                if proc.tags.is_empty() {
                    "*None*".to_string()
                } else {
                    proc.tags.join(", ")
                },
            ));

            atomic_write(&proc_path, &proc_content).await?;
        }

        atomic_write(&path, &content).await
    }

    // ─── LESSONS (GH-03) ─────────────────────────────────────────────────

    pub async fn write_lessons(&self, lessons: &[Lesson]) -> Result<(), VaultError> {
        let path = self.vault_path.join("Lessons").join("INDEX.md");
        let mut content = String::from(
            "# Lessons Learned\n\n\
             > Lessons synthesized from repeated experiences. Each lesson has a confidence \
             score that decays over time unless reinforced.\n\n---\n\n",
        );

        if lessons.is_empty() {
            content.push_str(
                "*No lessons synthesized yet. Lessons emerge when 3+ related memories \
                 share common themes with average importance > 5.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        // Group by category
        let mut by_category: BTreeMap<String, Vec<&Lesson>> = BTreeMap::new();
        for lesson in lessons {
            by_category
                .entry(lesson.category.clone())
                .or_default()
                .push(lesson);
        }

        content.push_str(&format!(
            "**{} lesson(s)** in {} categories\n\n",
            lessons.len(),
            by_category.len()
        ));

        for (cat, cat_lessons) in &by_category {
            content.push_str(&format!("## {}\n\n", cat));
            content.push_str("| Lesson | Confidence | Reinforcements | Decay Rate |\n");
            content.push_str("|--------|-----------|----------------|------------|\n");
            for lesson in cat_lessons {
                let snippet: String = lesson.content.chars().take(80).collect();
                let ellipsis = if lesson.content.len() > 80 { "…" } else { "" };
                content.push_str(&format!(
                    "| {}{} | {:.2} | {} | {:.3} |\n",
                    truncate_to_line(&snippet),
                    ellipsis,
                    lesson.confidence,
                    lesson.reinforcements,
                    lesson.decay_rate,
                ));
            }
            content.push('\n');
        }

        atomic_write(&path, &content).await
    }

    // ─── INSIGHTS (GH-04) ────────────────────────────────────────────────

    pub async fn write_insights(&self, insights: &[Insight]) -> Result<(), VaultError> {
        let path = self.vault_path.join("Insights").join("INDEX.md");
        let mut content = String::from(
            "# Insights\n\n\
             > Insights synthesized from concept cluster analysis. Each insight represents \
             a higher-order pattern discovered across related concepts.\n\n---\n\n",
        );

        if insights.is_empty() {
            content.push_str(
                "*No insights synthesized yet. Insights emerge when concept clusters \
                 form dense subgraphs with 3+ nodes.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!("**{} insight(s)**\n\n", insights.len()));
        content.push_str("| Title | Category | Confidence | Source Concepts |\n");
        content.push_str("|-------|----------|------------|----------------|\n");
        for insight in insights {
            content.push_str(&format!(
                "| {} | {} | {:.2} | {} |\n",
                insight.title,
                insight.category,
                insight.confidence,
                insight.source_concept_cluster.len(),
            ));
        }

        // Write detailed insight sections
        content.push_str("\n---\n\n");
        for insight in insights {
            content.push_str(&format!(
                "## {}\n\n> **Category:** {} | **Confidence:** {:.2}\n\n{}\n\n\
                 *Source: {} concept(s)*\n\n",
                insight.title,
                insight.category,
                insight.confidence,
                insight.content,
                insight.source_concept_cluster.len(),
            ));
        }

        atomic_write(&path, &content).await
    }

    // ─── RETENTION TIERS (GH-05) ─────────────────────────────────────────

    pub async fn write_retention_tiers(
        &self,
        entries: &[MemoryEntry],
        scorer: &EbbinghausScorer,
    ) -> Result<(), VaultError> {
        let path = self.vault_path.join("Retention").join("INDEX.md");
        let now = Utc::now().timestamp();

        let mut content = String::from(
            "# Memory Retention Tiers\n\n\
             > Ebbinghaus forgetting curve analysis. Memories are scored based on \
             type salience, time since last access, and access frequency.\n\n---\n\n",
        );

        if entries.is_empty() {
            content.push_str("*No memories to analyze.*\n");
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        // Compute tiers for all entries
        let mut tier_buckets: BTreeMap<String, Vec<(&MemoryEntry, f32)>> = BTreeMap::new();
        for entry in entries {
            let days_since = if entry.last_accessed_at > 0 {
                ((now - i64::from(entry.last_accessed_at)).max(0) as f32) / 86400.0
            } else {
                365.0 // treat never-accessed as very old
            };
            let access_ts: Vec<i64> = entry
                .access_timestamps
                .iter()
                .map(|t| i64::from(*t))
                .collect();
            let score = scorer.score(&entry.category, days_since, &access_ts, now);
            let tier = scorer.tier(score);
            let tier_name = match tier {
                savant_memory::promotion::RetentionTier::Hot => "Hot",
                savant_memory::promotion::RetentionTier::Warm => "Warm",
                savant_memory::promotion::RetentionTier::Cold => "Cold",
                savant_memory::promotion::RetentionTier::Dead => "Dead",
            };
            tier_buckets
                .entry(tier_name.to_string())
                .or_default()
                .push((entry, score));
        }

        // Summary table
        content.push_str("## Tier Distribution\n\n");
        content.push_str("| Tier | Count | Description |\n");
        content.push_str("|------|-------|-------------|\n");
        for (tier, desc) in &[
            ("Hot", "High retention — full detail preserved"),
            ("Warm", "Medium retention — summary recommended"),
            ("Cold", "Low retention — archive reference only"),
            ("Dead", "Below threshold — eligible for eviction"),
        ] {
            let count = tier_buckets.get(*tier).map(|v| v.len()).unwrap_or(0);
            content.push_str(&format!("| **{}** | {} | {} |\n", tier, count, desc));
        }

        // Per-tier listings (top 10 per tier)
        for tier_name in &["Hot", "Warm", "Cold", "Dead"] {
            if let Some(bucket) = tier_buckets.get(*tier_name) {
                if bucket.is_empty() {
                    continue;
                }
                content.push_str(&format!(
                    "\n## {} ({} memories)\n\n",
                    tier_name,
                    bucket.len()
                ));
                content.push_str("| Content | Score | Category | Last Accessed |\n");
                content.push_str("|---------|-------|----------|---------------|\n");
                let mut sorted = bucket.clone();
                sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (entry, score) in sorted.iter().take(10) {
                    let snippet: String = entry.content.chars().take(60).collect();
                    let last_acc = if entry.last_accessed_at > 0 {
                        chrono::DateTime::from_timestamp(i64::from(entry.last_accessed_at), 0)
                            .map(|d| d.format("%Y-%m-%d").to_string())
                            .unwrap_or_default()
                    } else {
                        "never".to_string()
                    };
                    content.push_str(&format!(
                        "| {}… | {:.3} | {} | {} |\n",
                        truncate_to_line(&snippet),
                        score,
                        entry.category,
                        last_acc,
                    ));
                }
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── VERSION HISTORY (GH-24) ─────────────────────────────────────────

    pub async fn write_version_history(&self, entries: &[MemoryEntry]) -> Result<(), VaultError> {
        let path = self
            .vault_path
            .join("Identity")
            .join("Evolution")
            .join("VersionHistory.md");
        let mut content = String::from(
            "# Version History\n\n\
             > Memory version chains and supersession timelines. \
             Shows how facts evolved over time.\n\n---\n\n",
        );

        // Filter to entries with versioning activity
        let versioned: Vec<&MemoryEntry> = entries
            .iter()
            .filter(|e| {
                let v: u32 = e.version.into();
                v > 1 || !e.supersedes.is_empty()
            })
            .collect();

        if versioned.is_empty() {
            content.push_str(
                "*No versioned memories yet. Versioning activates when updated facts \
                 supersede prior knowledge.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!(
            "**{} versioned memory chain(s)**\n\n",
            versioned.len()
        ));
        content.push_str("| Memory ID | Version | Content | Supersedes | Date |\n");
        content.push_str("|-----------|---------|---------|------------|------|\n");

        for entry in &versioned {
            let v: u32 = entry.version.into();
            let snippet: String = entry.content.chars().take(50).collect();
            let supersedes_str = if entry.supersedes.is_empty() {
                "—".to_string()
            } else {
                entry
                    .supersedes
                    .iter()
                    .map(|id| format!("`{}`", u64::from(*id)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let date = chrono::DateTime::from_timestamp(i64::from(entry.created_at) / 1000, 0)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "—".to_string());
            content.push_str(&format!(
                "| `{}` | v{} | {}… | {} | {} |\n",
                u64::from(entry.id),
                v,
                truncate_to_line(&snippet),
                supersedes_str,
                date,
            ));
        }

        atomic_write(&path, &content).await
    }

    // ─── MULTIMODAL (GH-25) ──────────────────────────────────────────────

    pub async fn write_multimodal_references(
        &self,
        images: &[savant_memory::multimodal::MultimodalMemory],
    ) -> Result<(), VaultError> {
        let path = self.vault_path.join("Multimodal").join("INDEX.md");
        let mut content = String::from(
            "# Multimodal Memory\n\n\
             > Image references with CLIP-generated descriptions. \
             Images are stored as file references with vector embeddings for cross-modal search.\n\n---\n\n",
        );

        if images.is_empty() {
            content.push_str(
                "*No images stored yet. Images are captured during agent interactions \
                 and indexed with CLIP embeddings.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!("**{} image(s)**\n\n", images.len()));
        content.push_str("| Image | Description | Dimensions | MIME |\n");
        content.push_str("|-------|-------------|------------|------|\n");

        for img in images {
            let dims = match (img.width, img.height) {
                (Some(w), Some(h)) => format!("{}×{}", w, h),
                _ => "unknown".to_string(),
            };
            content.push_str(&format!(
                "| ![[{}]] | {} | {} | {} |\n",
                img.file_path,
                truncate_to_line(&img.description),
                dims,
                img.mime_type,
            ));
        }

        atomic_write(&path, &content).await
    }

    // ─── AUDIT LOG (GH-26) ──────────────────────────────────────────────

    pub async fn write_audit_log(&self, trail: &AuditTrail) -> Result<(), VaultError> {
        let path = self.vault_path.join("Dashboard").join("Audit.md");
        let mut content = String::from(
            "# Audit Trail\n\n\
             > Recent memory operations. The audit trail provides observability \
             into all memory reads, writes, and transformations.\n\n---\n\n",
        );

        let entries = trail.entries();
        if entries.is_empty() {
            content.push_str("*No audit entries recorded.*\n");
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        // Show last 100 entries
        let display_count = entries.len().min(100);
        let start = entries.len().saturating_sub(display_count);

        content.push_str(&format!(
            "**{} total entries** (showing last {})\n\n",
            entries.len(),
            display_count,
        ));
        content.push_str("| # | Timestamp | Operation | Target | Description |\n");
        content.push_str("|---|-----------|-----------|--------|-------------|\n");

        for entry in entries.iter().skip(start) {
            let ts = chrono::DateTime::from_timestamp(entry.timestamp / 1000, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "—".to_string());
            let targets = if entry.target_ids.is_empty() {
                "—".to_string()
            } else {
                entry
                    .target_ids
                    .iter()
                    .map(|id| format!("`{}`", id))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            content.push_str(&format!(
                "| {} | {} | {:?} | {} | {} |\n",
                entry.index,
                ts,
                entry.operation,
                targets,
                truncate_to_line(&entry.description),
            ));
        }

        atomic_write(&path, &content).await
    }

    // ─── NOTIFICATIONS (GH-27) ───────────────────────────────────────────

    pub async fn write_notifications(
        &self,
        notifications: &[savant_memory::notifications::MemoryNotification],
    ) -> Result<(), VaultError> {
        let path = self.vault_path.join("Dashboard").join("Notifications.md");
        let mut content = String::from(
            "# Hive-Mind Notifications\n\n\
             > High-importance memory discoveries broadcast across the agent swarm. \
             These represent knowledge that transcends individual agent boundaries.\n\n---\n\n",
        );

        if notifications.is_empty() {
            content.push_str("*No notifications. Notifications are generated when agents discover high-importance memories.*\n");
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!("**{} notification(s)**\n\n", notifications.len()));
        content.push_str("| Timestamp | Source Session | Importance | Content |\n");
        content.push_str("|-----------|---------------|------------|--------|\n");

        for notif in notifications {
            let ts = chrono::DateTime::from_timestamp(notif.timestamp / 1000, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "—".to_string());
            let snippet: String = notif.content_preview.chars().take(80).collect();
            content.push_str(&format!(
                "| {} | `{}` | {} | {}… |\n",
                ts,
                notif.source_session,
                notif.importance,
                truncate_to_line(&snippet),
            ));
        }

        atomic_write(&path, &content).await
    }

    // ─── SYNC STATUS (GH-32) ─────────────────────────────────────────────

    pub async fn write_sync_status(
        &self,
        sync_manager: Option<&savant_memory::mesh_sync::MeshSyncManager>,
    ) -> Result<(), VaultError> {
        let path = self.vault_path.join("Dashboard").join("Sync.md");
        let mut content = String::from(
            "# Mesh Sync Status\n\n\
             > P2P synchronization state across Savant instances. \
             Shows vector clock, active peers, and pending operations.\n\n---\n\n",
        );

        match sync_manager {
            None => {
                content.push_str("*Mesh sync is not active. Enable `mesh_sync.enabled` in config to activate P2P synchronization.*\n");
            }
            Some(manager) => {
                let clock = manager.clock();
                content.push_str(&format!(
                    "## Instance\n\n| Field | Value |\n|-------|-------|\n\
                     | **Instance ID** | `{}` |\n\
                     | **Pending Operations** | — |\n\n",
                    manager.instance_id(),
                ));

                content.push_str(
                    "## Vector Clock\n\n| Instance | Timestamp |\n|----------|----------|\n",
                );
                for (instance, ts) in &clock.clocks {
                    content.push_str(&format!("| `{}` | {} |\n", instance, ts));
                }
                if clock.clocks.is_empty() {
                    content.push_str("| *No clock entries* | — |\n");
                }
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── TEMPORAL GRAPH (GH-08) ───────────────────────────────────────────

    pub async fn write_temporal_graph(
        &self,
        graph: &savant_memory::reflective::NamespaceGraph,
    ) -> Result<(), VaultError> {
        let path = self
            .vault_path
            .join("Graphs")
            .join("Temporal")
            .join("INDEX.md");
        let mut content = String::from(
            "# Temporal Graph\n\n\
             > Event ordering: superseded_by, evolved_into, prior_state, follows, precedes. \
             This graph tracks how memories evolve over time.\n\n---\n\n",
        );

        if graph.concepts.is_empty() && graph.relations.is_empty() {
            content.push_str(
                "*No temporal events recorded yet. The temporal graph populates \
                 as memories are updated, superseded, or evolved.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!(
            "**{} event nodes**, **{} temporal edges**\n\n",
            graph.concepts.len(),
            graph.relations.len(),
        ));

        // Render event nodes
        if !graph.concepts.is_empty() {
            content.push_str("## Events\n\n| ID | Label | Type | Created | Last Accessed |\n");
            content.push_str("|----|-------|------|---------|---------------|\n");
            for concept in &graph.concepts {
                let created = chrono::DateTime::from_timestamp(concept.created_at, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "—".to_string());
                content.push_str(&format!(
                    "| `{}` | {} | {:?} | {} | {} |\n",
                    concept.id, concept.label, concept.concept_type, created, concept.last_accessed,
                ));
            }
        }

        // Render temporal edges
        if !graph.relations.is_empty() {
            content.push_str("\n## Temporal Edges\n\n| Source | Relation | Target | Weight |\n");
            content.push_str("|--------|----------|--------|--------|\n");
            for rel in &graph.relations {
                content.push_str(&format!(
                    "| `{}` | {} | `{}` | {:.2} |\n",
                    rel.source_concept, rel.relation_type, rel.target_concept, rel.weight,
                ));
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── CAUSAL GRAPH (GH-09) ────────────────────────────────────────────

    pub async fn write_causal_graph(
        &self,
        graph: &savant_memory::reflective::NamespaceGraph,
    ) -> Result<(), VaultError> {
        let path = self
            .vault_path
            .join("Graphs")
            .join("Causal")
            .join("INDEX.md");
        let mut content = String::from(
            "# Causal Graph\n\n\
             > Action/outcome relationships: requires, generates, modifies, enables, prevents. \
             This graph tracks what causes what.\n\n---\n\n",
        );

        if graph.concepts.is_empty() && graph.relations.is_empty() {
            content.push_str(
                "*No causal relationships recorded yet. The causal graph populates \
                 as the agent observes action/outcome patterns.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!(
            "**{} action/outcome nodes**, **{} causal edges**\n\n",
            graph.concepts.len(),
            graph.relations.len(),
        ));

        if !graph.concepts.is_empty() {
            content.push_str("## Actions & Outcomes\n\n| ID | Label | Type | Created |\n");
            content.push_str("|----|-------|------|--------|\n");
            for concept in &graph.concepts {
                let created = chrono::DateTime::from_timestamp(concept.created_at, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "—".to_string());
                content.push_str(&format!(
                    "| `{}` | {} | {:?} | {} |\n",
                    concept.id, concept.label, concept.concept_type, created,
                ));
            }
        }

        if !graph.relations.is_empty() {
            content.push_str("\n## Causal Edges\n\n| Cause | Relation | Effect | Weight |\n");
            content.push_str("|-------|----------|--------|--------|\n");
            for rel in &graph.relations {
                content.push_str(&format!(
                    "| `{}` | {} | `{}` | {:.2} |\n",
                    rel.source_concept, rel.relation_type, rel.target_concept, rel.weight,
                ));
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── ENTITY GRAPH (GH-10) ────────────────────────────────────────────

    pub async fn write_entity_graph(
        &self,
        graph: &savant_memory::reflective::NamespaceGraph,
    ) -> Result<(), VaultError> {
        let path = self
            .vault_path
            .join("Graphs")
            .join("Entity")
            .join("INDEX.md");
        let mut content = String::from(
            "# Entity Graph\n\n\
             > Social/professional relations: works_for, founded, advises, knows, collaborates_with. \
             This graph tracks people, projects, and services.\n\n---\n\n",
        );

        if graph.concepts.is_empty() && graph.relations.is_empty() {
            content.push_str(
                "*No entities recorded yet. The entity graph populates as the agent \
                 encounters people, projects, and services in conversations.*\n",
            );
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        content.push_str(&format!(
            "**{} entity nodes**, **{} relation edges**\n\n",
            graph.concepts.len(),
            graph.relations.len(),
        ));

        if !graph.concepts.is_empty() {
            content.push_str("## Entities\n\n| ID | Name | Type | Created |\n");
            content.push_str("|----|------|------|--------|\n");
            for concept in &graph.concepts {
                let created = chrono::DateTime::from_timestamp(concept.created_at, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "—".to_string());
                content.push_str(&format!(
                    "| `{}` | {} | {:?} | {} |\n",
                    concept.id, concept.label, concept.concept_type, created,
                ));
            }
        }

        if !graph.relations.is_empty() {
            content.push_str("\n## Relations\n\n| Source | Relation | Target | Weight |\n");
            content.push_str("|--------|----------|--------|--------|\n");
            for rel in &graph.relations {
                content.push_str(&format!(
                    "| `{}` | {} | `{}` | {:.2} |\n",
                    rel.source_concept, rel.relation_type, rel.target_concept, rel.weight,
                ));
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── THEMES (GH-31) ──────────────────────────────────────────────────

    pub async fn write_themes_index_full(&self, entries: &[MemoryEntry]) -> Result<(), VaultError> {
        let path = self.vault_path.join("Themes").join("INDEX.md");
        let mut content = String::from(
            "# Themes — Cross-Domain Patterns\n\n\
             > Concept clusters discovered by the Dream Engine during REM cycles. \
             Themes emerge when concepts form dense subgraphs.\n\n---\n\n",
        );

        // Analyze concept density: group by category, find clusters
        let mut by_category: BTreeMap<String, Vec<&MemoryEntry>> = BTreeMap::new();
        for entry in entries {
            let cat = if entry.category.is_empty() {
                "uncategorized".to_string()
            } else {
                entry.category.clone()
            };
            by_category.entry(cat).or_default().push(entry);
        }

        // Find "themes" — categories with 3+ entries and average importance > 5
        let mut themes: Vec<(String, usize, f32)> = Vec::new();
        for (cat, cat_entries) in &by_category {
            if cat_entries.len() >= 3 {
                let avg_importance: f32 =
                    cat_entries.iter().map(|e| e.importance as f32).sum::<f32>()
                        / cat_entries.len() as f32;
                if avg_importance > 5.0 {
                    themes.push((cat.clone(), cat_entries.len(), avg_importance));
                }
            }
        }

        if themes.is_empty() {
            let total_concepts = entries.len();
            let total_relations: usize = entries.iter().map(|e| e.related_to.len()).sum();
            content.push_str(&format!(
                "No cross-domain themes discovered yet. Themes emerge when concepts form \
                 dense subgraphs (min 3 concepts, avg importance > 5.0). Current graph has \
                 **{}** concepts and **{}** relations.\n\n\
                 Themes are generated during Dream REM consolidation cycles. \
                 As the agent accumulates more sessions, theme density will increase.\n",
                total_concepts, total_relations,
            ));
            atomic_write(&path, &content).await?;
            return Ok(());
        }

        // Sort by entry count descending
        themes.sort_by(|a, b| b.1.cmp(&a.1));

        content.push_str(&format!("**{} theme(s) discovered**\n\n", themes.len()));
        content.push_str("| Theme | Concepts | Avg Importance |\n");
        content.push_str("|-------|----------|----------------|\n");
        for (cat, count, avg_imp) in &themes {
            content.push_str(&format!("| {} | {} | {:.1} |\n", cat, count, avg_imp));
        }

        // Detail sections
        content.push_str("\n---\n\n");
        for (cat, _, _) in &themes {
            if let Some(cat_entries) = by_category.get(cat) {
                content.push_str(&format!("## {}\n\n", cat));
                for entry in cat_entries.iter().take(5) {
                    let snippet: String = entry.content.chars().take(100).collect();
                    content.push_str(&format!(
                        "- {}… (importance: {})\n",
                        truncate_to_line(&snippet),
                        entry.importance,
                    ));
                }
                if cat_entries.len() > 5 {
                    content.push_str(&format!("- *…and {} more*\n", cat_entries.len() - 5,));
                }
                content.push('\n');
            }
        }

        atomic_write(&path, &content).await
    }

    // ─── FULL SYNC ────────────────────────────────────────────────────────

    pub async fn run_full_sync(&self, workspace_root: &Path) -> Result<VaultStats, VaultError> {
        self.ensure_structure().await?;
        let stats = self.collect_stats(workspace_root);
        let today = Utc::now().date_naive();

        // Core projections (existing)
        self.write_index(&stats).await?;
        self.write_episodic(&today).await?;
        self.write_concepts().await?;
        self.write_relations().await?;
        self.write_triplets().await?;
        self.write_entities().await?;
        self.write_soul(workspace_root).await?;
        self.write_personality(workspace_root).await?;
        self.write_evolution_index(workspace_root).await?;
        self.clear_working().await?;
        self.write_dashboard_recent().await?;
        self.write_dashboard_health(&stats).await?;

        // New projections (GH-02 through GH-05, GH-24 through GH-27, GH-31, GH-32)
        let entries: Vec<MemoryEntry> = if let Some(enclave) = &self.enclave {
            enclave.lsm().iter_metadata().unwrap_or_default()
        } else {
            Vec::new()
        };

        // GH-31: Themes (upgraded from stub — always writes)
        if let Err(e) = self.write_themes_index_full(&entries).await {
            tracing::warn!("[obsidian] Failed to write themes: {}", e);
        }

        // GH-24: Version history
        if let Err(e) = self.write_version_history(&entries).await {
            tracing::warn!("[obsidian] Failed to write version history: {}", e);
        }

        // GH-08 through GH-10: MAGMA graph projections
        if let Some(enclave) = &self.enclave {
            let reflective = enclave.reflective().await;
            if let Err(e) = self.write_temporal_graph(&reflective.temporal).await {
                tracing::warn!("[obsidian] Failed to write temporal graph: {}", e);
            }
            if let Err(e) = self.write_causal_graph(&reflective.causal).await {
                tracing::warn!("[obsidian] Failed to write causal graph: {}", e);
            }
            if let Err(e) = self.write_entity_graph(&reflective.entity).await {
                tracing::warn!("[obsidian] Failed to write entity graph: {}", e);
            }
        }

        // CP-17..CP-25: Wire remaining write methods with config toggles
        if let Some(enclave) = &self.enclave {
            // CP-17: Procedural memory (config-gated)
            if self.config.project_procedures {
                let procedures = enclave.procedures().await;
                if let Err(e) = self.write_procedures(&procedures).await {
                    tracing::warn!("[obsidian] Failed to write procedures: {}", e);
                }
            }

            // CP-18: Lessons (config-gated)
            if self.config.project_lessons {
                let lessons = enclave.lessons().await;
                if let Err(e) = self.write_lessons(&lessons).await {
                    tracing::warn!("[obsidian] Failed to write lessons: {}", e);
                }
            }

            // CP-19: Insights (config-gated, same toggle as lessons)
            if self.config.project_lessons {
                let insights = enclave.insights().await;
                if let Err(e) = self.write_insights(&insights).await {
                    tracing::warn!("[obsidian] Failed to write insights: {}", e);
                }
            }

            // CP-20: Retention tiers (config-gated)
            if self.config.project_retention_tiers {
                let scorer = savant_memory::promotion::EbbinghausScorer::default();
                if let Err(e) = self.write_retention_tiers(&entries, &scorer).await {
                    tracing::warn!("[obsidian] Failed to write retention tiers: {}", e);
                }
            }

            // CP-21: Multimodal references (config-gated)
            if self.config.project_multimodal {
                let multimodal = enclave.multimodal().await;
                if let Err(e) = self.write_multimodal_references(multimodal.entries()).await {
                    tracing::warn!("[obsidian] Failed to write multimodal: {}", e);
                }
            }

            // CP-22: Audit log (config-gated)
            if self.config.project_audit_trail {
                let audit = enclave.audit().await;
                if let Err(e) = self.write_audit_log(&audit).await {
                    tracing::warn!("[obsidian] Failed to write audit log: {}", e);
                }
            }

            // CP-23: Notifications — emit vault sync completion event
            enclave.notify(savant_memory::MemoryNotification {
                notification_id: format!("vault-sync-{}", chrono::Utc::now().timestamp_millis()),
                source_session: self.agent_name.clone(),
                memory_id: 0,
                domain_tags: vec!["obsidian".to_string(), "vault-sync".to_string()],
                importance: 5,
                timestamp: chrono::Utc::now().timestamp_millis(),
                content_preview: format!(
                    "Vault sync complete: {} files, {} memories, {} sessions",
                    stats.vault_file_count, stats.memory_count, stats.session_count
                ),
            });

            // CP-24: Sync status (config-gated via mesh_sync.enabled)
            let mesh_sync = enclave.mesh_sync().await;
            if mesh_sync.is_some() {
                if let Err(e) = self.write_sync_status(mesh_sync.as_ref()).await {
                    tracing::warn!("[obsidian] Failed to write sync status: {}", e);
                }
            }
        }

        info!(
            "[obsidian] Full vault sync: {} files, {} sessions, {} memories",
            stats.vault_file_count, stats.session_count, stats.memory_count,
        );
        Ok(stats)
    }

    fn collect_stats(&self, workspace_root: &Path) -> VaultStats {
        let mut stats = VaultStats {
            agent_name: self.agent_name.clone(),
            ..Default::default()
        };

        if let Some(enclave) = &self.enclave {
            let lsm = enclave.lsm();
            if let Ok(s) = lsm.stats() {
                stats.session_count = s.total_sessions;
                stats.memory_count = s.total_messages;
            }
            stats.vector_count = enclave.vector_count() as u64;
        }

        if self.vault_path.exists() {
            stats.vault_file_count = count_md_files(&self.vault_path);
        }

        let evolution_path = workspace_root.join("EVOLUTION.jsonl");
        if evolution_path.exists() {
            if let Ok(content) = fs::read_to_string(&evolution_path) {
                stats.mutation_count = content.lines().count() as u64;
            }
        }

        stats
    }
}

#[derive(Debug, Clone)]
pub struct VaultStats {
    pub agent_name: String,
    pub session_count: u64,
    pub memory_count: u64,
    pub vector_count: u64,
    pub vault_file_count: usize,
    pub mutation_count: u64,
    pub evolution_score: f32,
    pub stage: String,
}

impl Default for VaultStats {
    fn default() -> Self {
        Self {
            agent_name: String::new(),
            session_count: 0,
            memory_count: 0,
            vector_count: 0,
            vault_file_count: 0,
            mutation_count: 0,
            evolution_score: 0.0,
            stage: "Seedling".to_string(),
        }
    }
}

// ─── HELPERS ─────────────────────────────────────────────────────────────

/// Synchronous atomic write helper (blocking). Used internally and by sync callers.
fn atomic_write_sync(path: &Path, content: &str) -> Result<(), VaultError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Async atomic write — wraps blocking file I/O in `spawn_blocking` to avoid
/// blocking the tokio runtime.
pub async fn atomic_write(path: &Path, content: &str) -> Result<(), VaultError> {
    let path = path.to_path_buf();
    let content = content.to_string();
    tokio::task::spawn_blocking(move || atomic_write_sync(&path, &content))
        .await
        .map_err(|e| VaultError::Config(e.to_string()))?
}

pub fn count_md_files(path: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                count += count_md_files(&p);
            } else if p.extension().is_some_and(|e| e == "md") {
                count += 1;
            }
        }
    }
    count
}

pub fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .collect::<String>()
        .trim()
        .replace(' ', "-")
}

pub fn truncate_to_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).to_string()
}

fn strength_bar(strength: f32) -> String {
    let filled = (strength * 10.0).round() as usize;
    let empty = 10_usize.saturating_sub(filled);
    format!(
        "{}{} ({:.2})",
        "█".repeat(filled),
        "░".repeat(empty),
        strength
    )
}

const STOPWORDS: &[&str] = &[
    "The",
    "This",
    "That",
    "These",
    "Those",
    "When",
    "Where",
    "What",
    "Which",
    "Who",
    "Whom",
    "How",
    "Why",
    "If",
    "Then",
    "Else",
    "For",
    "And",
    "But",
    "Or",
    "Nor",
    "Not",
    "So",
    "Yet",
    "Both",
    "Either",
    "Neither",
    "Each",
    "Every",
    "All",
    "Any",
    "Few",
    "More",
    "Most",
    "Other",
    "Some",
    "Such",
    "No",
    "Only",
    "Own",
    "Same",
    "Than",
    "Too",
    "Very",
    "Just",
    "Because",
    "Before",
    "After",
    "Above",
    "Below",
    "Between",
    "During",
    "Into",
    "Through",
    "About",
    "Against",
    "Along",
    "Among",
    "Around",
    "However",
    "Although",
    "Meanwhile",
    "Perhaps",
    "Also",
    "Still",
    "Already",
];

fn extract_entities_from_text(text: &str, entities: &mut BTreeMap<String, (u32, u32, i64, i64)>) {
    let now = Utc::now().timestamp();
    // Title-case multi-word patterns (potential project/person names)
    for word in text.split_whitespace() {
        let cleaned: String = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string();
        if cleaned.len() >= 3
            && !STOPWORDS.contains(&cleaned.as_str())
            && cleaned.chars().next().is_some_and(|c| c.is_uppercase())
            && cleaned.chars().skip(1).any(|c| c.is_lowercase())
        {
            let entry = entities.entry(cleaned).or_insert((0, 0, now, now));
            entry.0 += 1;
        }
    }
    // URL-like patterns (potential services)
    if text.contains("://") || text.contains(".com") || text.contains(".ai") {
        let service = "web-service".to_string();
        let entry = entities.entry(service).or_insert((0, 0, now, now));
        entry.0 += 1;
    }
}
