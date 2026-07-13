//! Performance benchmarks for Savant core components.
//! Run with: cargo test -p savant_core --test perf_benchmarks

use savant_core::types::{AgentOutputChannel, ChatMessage, ChatRole};
use std::time::Instant;

#[test]
fn bench_storage_append() {
    let temp_dir = std::env::temp_dir().join("savant_bench_append");
    let _ = std::fs::remove_dir_all(&temp_dir); // Clean up from previous runs
    let db_path = temp_dir.clone();

    let storage = match savant_core::db::Storage::with_defaults(db_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("SKIP: Could not create storage");
            return;
        }
    };

    let start = Instant::now();
    for i in 0..1000 {
        let msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: format!("Benchmark message {} with padding to simulate realistic content size for performance testing", i),
            sender: Some("bench".to_string()),
            recipient: None,
            agent_id: Some("bench-agent".to_string()),
            session_id: None,
            channel: AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        };
        storage
            .append_chat("bench-agent", &msg)
            .unwrap_or_else(|e| panic!("append failed: {}", e));
    }
    let elapsed = start.elapsed();

    println!(
        "Storage append: 1000 messages in {:?} ({:.0} msg/s)",
        elapsed,
        1000.0 / elapsed.as_secs_f64()
    );
    assert!(
        elapsed.as_secs() < 30,
        "1000 appends should complete within 30s"
    );
}

#[test]
fn bench_storage_retrieve() {
    let temp_dir = std::env::temp_dir().join("savant_bench_retrieve");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let db_path = temp_dir.clone();

    let storage = match savant_core::db::Storage::with_defaults(db_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("SKIP: Could not create storage");
            return;
        }
    };

    // Insert 500 messages
    for i in 0..500 {
        let msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: format!("Message {}", i),
            sender: Some("bench".to_string()),
            recipient: None,
            agent_id: Some("ret-agent".to_string()),
            session_id: None,
            channel: AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        };
        storage
            .append_chat("ret-agent", &msg)
            .unwrap_or_else(|e| panic!("append failed: {}", e));
    }

    let start = Instant::now();
    for _ in 0..100 {
        let _ = storage
            .get_history("ret-agent", 50)
            .unwrap_or_else(|e| panic!("get_history failed: {}", e));
    }
    let elapsed = start.elapsed();

    println!(
        "Storage retrieve: 100 queries in {:?} ({:.0} q/s)",
        elapsed,
        100.0 / elapsed.as_secs_f64()
    );
}

#[test]
fn bench_session_sanitization() {
    let start = Instant::now();
    for i in 0..10000 {
        let input = format!("test-session-{}!@#$%", i);
        let _ = savant_core::session::SessionMapper::sanitize(&input);
    }
    let elapsed = start.elapsed();

    println!(
        "Session sanitize: 10000 calls in {:?} ({:.0} calls/s)",
        elapsed,
        10000.0 / elapsed.as_secs_f64()
    );
}
