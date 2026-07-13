#![allow(clippy::disallowed_methods)]

use savant_core::config::ObsidianConfig;
use savant_obsidian::cold_storage::ColdStorageManager;
use savant_obsidian::outbox::{CursorState, StateSnapshot};
use savant_obsidian::writer::{
    atomic_write, count_md_files, slugify, truncate_to_line, VaultStats, VaultWriter,
};
use savant_obsidian::VaultError;
use std::path::PathBuf;

fn make_temp_vault() -> PathBuf {
    let id = uuid::Uuid::new_v4();
    std::env::temp_dir().join(format!("savant-vault-test-{}", id))
}

fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_dir_all(path);
}

fn default_config() -> ObsidianConfig {
    ObsidianConfig {
        enabled: true,
        vault_path: None,
        sync_interval_secs: 30,
        max_files: 15000,
        cold_storage_days: 30,
        tombstone_prune_days: 30,
        db_only_dirs: vec!["Episodic".to_string()],
        project_procedures: true,
        project_lessons: true,
        project_graphs: true,
        project_retention_tiers: true,
        project_audit_trail: false,
        project_multimodal: false,
    }
}

// ─── VaultError tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_vault_error_io_display() {
    let err = VaultError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file not found",
    ));
    let msg = format!("{}", err);
    assert!(msg.contains("IO error"));
    assert!(msg.contains("file not found"));
}

#[tokio::test]
async fn test_vault_error_injection_display() {
    let err = VaultError::InjectionDetected("../../../etc/passwd".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Injection detected"));
    assert!(msg.contains("../../../etc/passwd"));
}

#[tokio::test]
async fn test_vault_error_config_display() {
    let err = VaultError::Config("missing vault path".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("Configuration error"));
    assert!(msg.contains("missing vault path"));
}

#[tokio::test]
async fn test_vault_error_vault_path_not_configured() {
    let err = VaultError::VaultPathNotConfigured;
    let msg = format!("{}", err);
    assert_eq!(msg, "Vault path not configured");
}

#[tokio::test]
async fn test_vault_error_serialization() {
    let bad_json = "not json";
    let result: Result<serde_json::Value, _> = serde_json::from_str(bad_json);
    let err = result.unwrap_err();
    let vault_err = VaultError::Serialization(err);
    let msg = format!("{}", vault_err);
    assert!(msg.contains("Serialization error"));
}

// ─── VaultStats tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_vault_stats_default() {
    let stats = VaultStats::default();
    assert_eq!(stats.agent_name, "");
    assert_eq!(stats.session_count, 0);
    assert_eq!(stats.memory_count, 0);
    assert_eq!(stats.vector_count, 0);
    assert_eq!(stats.vault_file_count, 0);
    assert_eq!(stats.mutation_count, 0);
    assert_eq!(stats.evolution_score, 0.0);
    assert_eq!(stats.stage, "Seedling");
}

#[tokio::test]
async fn test_vault_stats_clone() {
    let stats = VaultStats::default();
    let cloned = stats.clone();
    assert_eq!(cloned.stage, stats.stage);
}

#[tokio::test]
async fn test_vault_stats_debug() {
    let stats = VaultStats::default();
    let debug = format!("{:?}", stats);
    assert!(debug.contains("VaultStats"));
}

// ─── slugify tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_slugify_lowercase() {
    assert_eq!(slugify("Hello World"), "hello-world");
}

#[tokio::test]
async fn test_slugify_preserves_dashes() {
    assert_eq!(slugify("already-slugified"), "already-slugified");
}

#[tokio::test]
async fn test_slugify_removes_special_chars() {
    assert_eq!(slugify("Hello! @#$ World"), "hello--world");
}

#[tokio::test]
async fn test_slugify_trims_whitespace() {
    assert_eq!(slugify("  spaced out  "), "spaced-out");
}

#[tokio::test]
async fn test_slugify_empty_string() {
    assert_eq!(slugify(""), "");
}

#[tokio::test]
async fn test_slugify_single_word() {
    assert_eq!(slugify("Concept"), "concept");
}

#[tokio::test]
async fn test_slugify_multiple_spaces() {
    assert_eq!(slugify("multiple   spaces"), "multiple---spaces");
}

// ─── truncate_to_line tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_truncate_to_line_single() {
    assert_eq!(truncate_to_line("single line"), "single line");
}

#[tokio::test]
async fn test_truncate_to_line_multi() {
    assert_eq!(truncate_to_line("first line\nsecond line"), "first line");
}

#[tokio::test]
async fn test_truncate_to_line_empty() {
    assert_eq!(truncate_to_line(""), "");
}

#[tokio::test]
async fn test_truncate_to_line_with_newline_at_end() {
    assert_eq!(truncate_to_line("has newline\n"), "has newline");
}

// ─── count_md_files tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_count_md_files_empty_dir() {
    let dir = make_temp_vault();
    std::fs::create_dir_all(&dir).unwrap();
    let count = count_md_files(&dir);
    assert_eq!(count, 0);
    cleanup(&dir);
}

#[tokio::test]
async fn test_count_md_files_single_file() {
    let dir = make_temp_vault();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test.md"), "# Test").unwrap();
    let count = count_md_files(&dir);
    assert_eq!(count, 1);
    cleanup(&dir);
}

#[tokio::test]
async fn test_count_md_files_ignores_non_md() {
    let dir = make_temp_vault();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test.md"), "# Test").unwrap();
    std::fs::write(dir.join("test.txt"), "not counted").unwrap();
    std::fs::write(dir.join("test.json"), "{}").unwrap();
    let count = count_md_files(&dir);
    assert_eq!(count, 1);
    cleanup(&dir);
}

#[tokio::test]
async fn test_count_md_files_nested() {
    let dir = make_temp_vault();
    let sub = dir.join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(dir.join("root.md"), "# Root").unwrap();
    std::fs::write(sub.join("nested.md"), "# Nested").unwrap();
    let count = count_md_files(&dir);
    assert_eq!(count, 2);
    cleanup(&dir);
}

#[tokio::test]
async fn test_count_md_files_nonexistent_dir() {
    let dir = std::env::temp_dir().join("nonexistent-dir-12345");
    let count = count_md_files(&dir);
    assert_eq!(count, 0);
}

// ─── atomic_write tests ───────────────────────────────────────────────────

#[tokio::test]
async fn test_atomic_write_creates_parent_dirs() {
    let dir = make_temp_vault();
    let nested = dir.join("a").join("b").join("c").join("test.md");
    atomic_write(&nested, "# Test").await.unwrap();
    assert!(nested.exists());
    let content = std::fs::read_to_string(&nested).unwrap();
    assert_eq!(content, "# Test");
    cleanup(&dir);
}

#[tokio::test]
async fn test_atomic_write_overwrites() {
    let dir = make_temp_vault();
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("test.md");
    atomic_write(&file, "first").await.unwrap();
    atomic_write(&file, "second").await.unwrap();
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "second");
    cleanup(&dir);
}

#[tokio::test]
async fn test_atomic_write_no_temp_left_behind() {
    let dir = make_temp_vault();
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("test.md");
    atomic_write(&file, "content").await.unwrap();
    let tmp = file.with_extension("tmp");
    assert!(!tmp.exists());
    cleanup(&dir);
}

// ─── VaultWriter tests ────────────────────────────────────────────────────

fn make_writer(vault: PathBuf) -> VaultWriter {
    let config = ObsidianConfig {
        vault_path: Some(vault.to_string_lossy().into_owned()),
        ..default_config()
    };
    VaultWriter::new(vault, None, config, "TestAgent".to_string())
}

#[tokio::test]
async fn test_vault_writer_ensure_structure() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    let result = writer.ensure_structure().await;
    assert!(result.is_ok());
    assert!(vault.join(".obsidian").exists());
    assert!(vault.join("Episodic").exists());
    assert!(vault.join("Semantic").exists());
    assert!(vault.join("Identity").join("Evolution").exists());
    assert!(vault.join("Themes").exists());
    assert!(vault.join("Working").exists());
    assert!(vault.join("Dashboard").exists());
    assert!(vault.join(".stale").exists());
    let gitignore = vault.join(".stale").join(".gitignore");
    assert!(gitignore.exists());
    assert_eq!(std::fs::read_to_string(gitignore).unwrap(), "*\n");
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_ensure_structure_idempotent() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    assert!(writer.ensure_structure().await.is_ok());
    assert!(writer.ensure_structure().await.is_ok());
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_write_index() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    let stats = VaultStats {
        agent_name: "TestAgent".to_string(),
        session_count: 5,
        memory_count: 100,
        vector_count: 50,
        vault_file_count: 10,
        mutation_count: 3,
        evolution_score: 0.42,
        stage: "Apprentice".to_string(),
    };
    assert!(writer.write_index(&stats).await.is_ok());
    let index = vault.join("INDEX.md");
    assert!(index.exists());
    let content = std::fs::read_to_string(&index).unwrap();
    assert!(content.contains("TestAgent"));
    assert!(content.contains("Apprentice"));
    assert!(content.contains("0.42"));
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_write_episodic_empty() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    let today = chrono::Utc::now().date_naive();
    assert!(writer.write_episodic(&today).await.is_ok());
    let episodic = vault
        .join("Episodic")
        .join(format!("{}.md", today.format("%Y-%m-%d")));
    assert!(episodic.exists());
    assert!(std::fs::read_to_string(&episodic)
        .unwrap()
        .contains("No sessions recorded"));
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_write_themes_index() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_themes_index().await.is_ok());
    assert!(vault.join("Themes").join("INDEX.md").exists());
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_clear_working() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    let working = vault.join("Working");
    std::fs::write(working.join("scratch.md"), "# Scratch").unwrap();
    std::fs::write(working.join("notes.md"), "# Notes").unwrap();
    assert!(writer.clear_working().await.is_ok());
    assert!(!working.join("scratch.md").exists());
    assert!(!working.join("notes.md").exists());
    assert!(working.join("README.md").exists());
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_write_dashboard_recent() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_dashboard_recent().await.is_ok());
    let recent = vault.join("Dashboard").join("Recent.md");
    assert!(recent.exists());
    let content = std::fs::read_to_string(&recent).unwrap();
    assert!(content.contains("Recent Activity"));
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_write_dashboard_health() {
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    let stats = VaultStats {
        agent_name: "TestAgent".to_string(),
        session_count: 10,
        memory_count: 250,
        vector_count: 100,
        vault_file_count: 42,
        mutation_count: 5,
        evolution_score: 0.75,
        stage: "Journeyman".to_string(),
    };
    assert!(writer.write_dashboard_health(&stats).await.is_ok());
    let health = vault.join("Dashboard").join("Health.md");
    assert!(health.exists());
    let content = std::fs::read_to_string(&health).unwrap();
    assert!(content.contains("Memory Health"));
    assert!(content.contains("Journeyman"));
    cleanup(&vault);
}

#[tokio::test]
async fn test_vault_writer_write_soul_default() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_soul(&workspace).await.is_ok());
    let soul = vault.join("Identity").join("SOUL.md");
    assert!(soul.exists());
    let content = std::fs::read_to_string(&soul).unwrap();
    assert!(content.contains("SOUL.md"));
    assert!(content.contains("TestAgent"));
    cleanup(&vault);
    cleanup(&workspace);
}

#[tokio::test]
async fn test_vault_writer_write_soul_from_workspace() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let soul_content = "# My SOUL\n\nI am a test agent.\n";
    std::fs::write(workspace.join("SOUL.md"), soul_content).unwrap();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_soul(&workspace).await.is_ok());
    let soul = vault.join("Identity").join("SOUL.md");
    let content = std::fs::read_to_string(&soul).unwrap();
    assert_eq!(content.trim(), soul_content.trim());
    cleanup(&vault);
    cleanup(&workspace);
}

#[tokio::test]
async fn test_vault_writer_write_personality_default() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_personality(&workspace).await.is_ok());
    let personality = vault.join("Identity").join("Personality.md");
    assert!(personality.exists());
    let content = std::fs::read_to_string(&personality).unwrap();
    assert!(content.contains("Personality"));
    assert!(content.contains("TestAgent"));
    cleanup(&vault);
    cleanup(&workspace);
}

#[tokio::test]
async fn test_vault_writer_write_personality_from_agent_json() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let agent_json = serde_json::json!({
        "personality_traits": {
            "openness": 0.85,
            "conscientiousness": 0.72,
            "extraversion": 0.45,
            "agreeableness": 0.90,
            "neuroticism": 0.15
        }
    });
    std::fs::write(workspace.join("agent.json"), agent_json.to_string()).unwrap();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_personality(&workspace).await.is_ok());
    let personality = vault.join("Identity").join("Personality.md");
    let content = std::fs::read_to_string(&personality).unwrap();
    assert!(content.contains("0.85"));
    assert!(content.contains("0.72"));
    assert!(content.contains("0.45"));
    assert!(content.contains("0.90"));
    assert!(content.contains("0.15"));
    cleanup(&vault);
    cleanup(&workspace);
}

#[tokio::test]
async fn test_vault_writer_write_evolution_index_empty() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_evolution_index(&workspace).await.is_ok());
    let index = vault.join("Identity").join("Evolution").join("INDEX.md");
    assert!(index.exists());
    let content = std::fs::read_to_string(&index).unwrap();
    assert!(content.contains("Evolution Timeline"));
    assert!(content.contains("Seedling"));
    cleanup(&vault);
    cleanup(&workspace);
}

#[tokio::test]
async fn test_vault_writer_write_evolution_index_with_data() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let agent_json = serde_json::json!({
        "evolution_state": {
            "stage": "Journeyman",
            "evolution_score": 0.65,
            "mutation_count": 12
        }
    });
    std::fs::write(workspace.join("agent.json"), agent_json.to_string()).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let mutations = [
        serde_json::json!({
            "mutation_id": "mut-001",
            "mutation_type": "soul_edit",
            "target_section": "Terminal Mantra",
            "status": "approved",
            "confidence": 0.88,
            "proposed_at": now,
            "reasoning": "Test mutation",
            "proposed_content": "New mantra",
            "source_evidence": ["2026-05-14"]
        }),
        serde_json::json!({
            "mutation_id": "mut-002",
            "mutation_type": "trait_adjust",
            "target_section": "Personality",
            "status": "pending",
            "confidence": 0.55,
            "proposed_at": now,
            "reasoning": "Another test",
            "proposed_content": "New trait value"
        }),
    ];
    let jsonl = mutations
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(workspace.join("EVOLUTION.jsonl"), jsonl).unwrap();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    assert!(writer.write_evolution_index(&workspace).await.is_ok());
    let index = vault.join("Identity").join("Evolution").join("INDEX.md");
    let content = std::fs::read_to_string(&index).unwrap();
    assert!(content.contains("Journeyman"));
    assert!(content.contains("0.65"));
    assert!(content.contains("2"));
    assert!(content.contains("approved"));
    assert!(content.contains("pending"));
    let report = vault
        .join("Identity")
        .join("Evolution")
        .join("reports")
        .join("mut-001.md");
    assert!(report.exists());
    cleanup(&vault);
    cleanup(&workspace);
}

#[tokio::test]
async fn test_vault_writer_run_full_sync() {
    let vault = make_temp_vault();
    let workspace = make_temp_vault();
    std::fs::create_dir_all(&workspace).unwrap();
    let writer = make_writer(vault.clone());
    let result = writer.run_full_sync(&workspace).await;
    assert!(result.is_ok());
    let stats = result.unwrap();
    assert_eq!(stats.agent_name, "TestAgent");
    assert!(vault.join("INDEX.md").exists());
    assert!(vault.join("Semantic").join("Concepts.md").exists());
    assert!(vault.join("Semantic").join("Relations.md").exists());
    assert!(vault.join("Semantic").join("Triplets.md").exists());
    assert!(vault.join("Semantic").join("Entities.md").exists());
    assert!(vault.join("Identity").join("SOUL.md").exists());
    assert!(vault.join("Identity").join("Personality.md").exists());
    assert!(vault
        .join("Identity")
        .join("Evolution")
        .join("INDEX.md")
        .exists());
    assert!(vault.join("Themes").join("INDEX.md").exists());
    assert!(vault.join("Working").join("README.md").exists());
    assert!(vault.join("Dashboard").join("Recent.md").exists());
    assert!(vault.join("Dashboard").join("Health.md").exists());
    cleanup(&vault);
    cleanup(&workspace);
}

// ─── ColdStorageManager tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_cold_storage_manager_new() {
    let vault = make_temp_vault();
    let config = default_config();
    let manager = ColdStorageManager::new(vault.clone(), config);
    drop(manager);
    cleanup(&vault);
}

#[tokio::test]
async fn test_cold_storage_manager_run_empty_vault() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(&vault).unwrap();
    let config = ObsidianConfig {
        vault_path: Some(vault.to_string_lossy().into_owned()),
        max_files: 100,
        cold_storage_days: 30,
        ..default_config()
    };
    let writer = VaultWriter::new(vault.clone(), None, config.clone(), "TestAgent".to_string());
    let manager = ColdStorageManager::new(vault.clone(), config);
    assert!(manager.run(&writer).await.is_ok());
    cleanup(&vault);
}

#[tokio::test]
async fn test_cold_storage_manager_enforces_min_max() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(&vault).unwrap();
    let config = ObsidianConfig {
        vault_path: Some(vault.to_string_lossy().into_owned()),
        max_files: 0,
        cold_storage_days: 30,
        ..default_config()
    };
    let writer = VaultWriter::new(vault.clone(), None, config.clone(), "TestAgent".to_string());
    let manager = ColdStorageManager::new(vault.clone(), config);
    assert!(manager.run(&writer).await.is_ok());
    cleanup(&vault);
}

#[tokio::test]
async fn test_cold_storage_manager_archives_old_episodic() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(vault.join("Episodic")).unwrap();
    std::fs::create_dir_all(vault.join(".stale")).unwrap();
    let old_date = chrono::Utc::now().date_naive() - chrono::Duration::days(60);
    let old_file = vault
        .join("Episodic")
        .join(format!("{}.md", old_date.format("%Y-%m-%d")));
    std::fs::write(&old_file, "# Old session").unwrap();
    let recent_date = chrono::Utc::now().date_naive() - chrono::Duration::days(5);
    let recent_file = vault
        .join("Episodic")
        .join(format!("{}.md", recent_date.format("%Y-%m-%d")));
    std::fs::write(&recent_file, "# Recent session").unwrap();
    let config = ObsidianConfig {
        vault_path: Some(vault.to_string_lossy().into_owned()),
        max_files: 1000,
        cold_storage_days: 30,
        ..default_config()
    };
    let writer = VaultWriter::new(vault.clone(), None, config.clone(), "TestAgent".to_string());
    let manager = ColdStorageManager::new(vault.clone(), config);
    assert!(manager.run(&writer).await.is_ok());
    assert!(!old_file.exists());
    assert!(recent_file.exists());
    assert!(vault.join(".stale").exists());
    cleanup(&vault);
}

#[tokio::test]
async fn test_cold_storage_manager_file_ceiling_enforcement() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(vault.join("Episodic")).unwrap();
    for i in 0..15 {
        let date = chrono::Utc::now().date_naive() - chrono::Duration::days(60 + i as i64);
        let file = vault
            .join("Episodic")
            .join(format!("{}.md", date.format("%Y-%m-%d")));
        std::fs::write(&file, format!("# Session {}", i)).unwrap();
    }
    let config = ObsidianConfig {
        vault_path: Some(vault.to_string_lossy().into_owned()),
        max_files: 10,
        cold_storage_days: 30,
        ..default_config()
    };
    let writer = VaultWriter::new(vault.clone(), None, config.clone(), "TestAgent".to_string());
    let manager = ColdStorageManager::new(vault.clone(), config);
    assert!(manager.run(&writer).await.is_ok());
    let remaining = count_md_files(&vault);
    assert!(remaining <= 10);
    cleanup(&vault);
}

// ─── Outbox CursorState tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_cursor_state_load_nonexistent() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(&vault).unwrap();
    let cursor = CursorState::load(&vault).await;
    assert_eq!(cursor.session_count, 0);
    assert_eq!(cursor.memory_count, 0);
    assert_eq!(cursor.vector_count, 0);
    cleanup(&vault);
}

#[tokio::test]
async fn test_cursor_state_save_and_load() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(&vault).unwrap();
    let cursor = CursorState {
        session_count: 42,
        memory_count: 1000,
        vector_count: 500,
        mutation_count: 10,
        vault_file_count: 200,
        timestamp: 1234567890,
        procedure_count: 0,
        lesson_count: 0,
        insight_count: 0,
        audit_count: 0,
    };
    cursor.save(&vault).await;
    let loaded = CursorState::load(&vault).await;
    assert_eq!(loaded.session_count, 42);
    assert_eq!(loaded.memory_count, 1000);
    assert_eq!(loaded.vector_count, 500);
    assert_eq!(loaded.mutation_count, 10);
    assert_eq!(loaded.vault_file_count, 200);
    assert_eq!(loaded.timestamp, 1234567890);
    cleanup(&vault);
}

#[tokio::test]
async fn test_cursor_state_has_changed() {
    let cursor = CursorState {
        session_count: 10,
        memory_count: 100,
        vector_count: 50,
        mutation_count: 5,
        vault_file_count: 20,
        timestamp: 1234567890,
        procedure_count: 0,
        lesson_count: 0,
        insight_count: 0,
        audit_count: 0,
    };
    let same_state = StateSnapshot {
        session_count: 10,
        memory_count: 100,
        vector_count: 50,
        procedure_count: 0,
        lesson_count: 0,
        insight_count: 0,
        audit_count: 0,
    };
    assert!(!cursor.has_changed(&same_state));

    let diff_state = StateSnapshot {
        session_count: 11,
        memory_count: 100,
        vector_count: 50,
        procedure_count: 0,
        lesson_count: 0,
        insight_count: 0,
        audit_count: 0,
    };
    assert!(cursor.has_changed(&diff_state));

    let diff_state2 = StateSnapshot {
        session_count: 10,
        memory_count: 101,
        vector_count: 50,
        procedure_count: 0,
        lesson_count: 0,
        insight_count: 0,
        audit_count: 0,
    };
    assert!(cursor.has_changed(&diff_state2));

    let diff_state3 = StateSnapshot {
        session_count: 10,
        memory_count: 100,
        vector_count: 51,
        procedure_count: 0,
        lesson_count: 0,
        insight_count: 0,
        audit_count: 0,
    };
    assert!(cursor.has_changed(&diff_state3));
}

#[tokio::test]
async fn test_cursor_state_load_corrupted_json() {
    let vault = make_temp_vault();
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(vault.join(".cursor.json"), "not valid json {{{").unwrap();
    let cursor = CursorState::load(&vault).await;
    assert_eq!(cursor.session_count, 0);
    cleanup(&vault);
}

// ─── Delegation artifact tests ────────────────────────────────────────────

#[tokio::test]
async fn test_vault_writer_write_delegation_artifact() {
    use savant_ipc::a2a::protocol::{Artifact, ArtifactPart, ArtifactPartType, TaskState};
    let vault = make_temp_vault();
    let writer = make_writer(vault.clone());
    writer.ensure_structure().await.unwrap();
    let artifact = Artifact {
        task_id: [0u8; 16],
        part_count: 2,
        _padding: [0u8; 7],
    };
    let parts = vec![
        ArtifactPart {
            part_type: ArtifactPartType::Text,
            data_offset: 0,
            data_len: 100,
        },
        ArtifactPart {
            part_type: ArtifactPartType::Json,
            data_offset: 100,
            data_len: 200,
        },
    ];
    assert!(writer
        .write_delegation_artifact(
            "task-001",
            "agent-parent",
            "agent-child",
            &artifact,
            &parts,
            TaskState::Completed,
            50000,
            1,
            1700000000,
        )
        .await
        .is_ok());
    let artifact_file = vault.join("Delegation").join("task-001.md");
    assert!(artifact_file.exists());
    let content = std::fs::read_to_string(&artifact_file).unwrap();
    assert!(content.contains("task-001"));
    assert!(content.contains("agent-parent"));
    assert!(content.contains("completed"));
    cleanup(&vault);
}
