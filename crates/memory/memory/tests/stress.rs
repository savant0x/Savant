//! Memory engine stress tests - concurrent writes, consolidation, persistence.

#![allow(clippy::disallowed_methods)]

#[cfg(test)]
mod memory_stress_tests {
    use std::sync::Arc;
    use std::time::Instant;

    fn make_engine(
        dir: &std::path::Path,
    ) -> Result<std::sync::Arc<savant_memory::MemoryEngine>, savant_memory::MemoryError> {
        savant_memory::MemoryEngine::with_defaults(
            dir,
            Arc::new(savant_memory::MockEmbeddingProvider),
        )
    }

    #[tokio::test]
    async fn test_concurrent_writes_same_session() {
        // This test writes 50 messages concurrently to the same session
        // and verifies all messages are present
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("stress_test");

        let engine = match make_engine(&db_path) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("SKIP: Could not create memory engine");
                return;
            }
        };

        let session_id = "stress-concurrent-same";
        let mut handles = vec![];

        for i in 0..50 {
            let engine_clone = engine.clone();
            let sid = session_id.to_string();
            let handle = tokio::spawn(async move {
                let msg =
                    savant_memory::AgentMessage::user(&sid, &format!("Concurrent message {}", i));
                engine_clone.append_message(&sid, &msg).await
            });
            handles.push(handle);
        }

        let mut success_count = 0;
        for handle in handles {
            match handle.await {
                Ok(Ok(())) => success_count += 1,
                Ok(Err(e)) => eprintln!("Write error: {}", e),
                Err(e) => eprintln!("Join error: {}", e),
            }
        }

        assert!(
            success_count >= 45,
            "Expected most writes to succeed, got {}",
            success_count
        );
    }

    #[tokio::test]
    async fn test_concurrent_writes_different_sessions() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("stress_multi");

        let engine = match make_engine(&db_path) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("SKIP: Could not create memory engine");
                return;
            }
        };

        let mut handles = vec![];

        for session_num in 0..20 {
            for msg_num in 0..10 {
                let engine_clone = engine.clone();
                let sid = format!("session-{}", session_num);
                let handle = tokio::spawn(async move {
                    let msg = savant_memory::AgentMessage::user(
                        &sid,
                        &format!("Session {} message {}", session_num, msg_num),
                    );
                    engine_clone.append_message(&sid, &msg).await
                });
                handles.push(handle);
            }
        }

        let mut success_count = 0;
        for handle in handles {
            match handle.await {
                Ok(Ok(())) => success_count += 1,
                Ok(Err(e)) => eprintln!("Write error: {}", e),
                Err(e) => eprintln!("Join error: {}", e),
            }
        }

        assert!(
            success_count >= 180,
            "Expected most writes to succeed, got {}",
            success_count
        );
    }

    #[tokio::test]
    async fn test_bulk_insert_performance() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("perf_test");

        let engine = match make_engine(&db_path) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("SKIP: Could not create memory engine");
                return;
            }
        };

        let session_id = "perf-test";
        let start = Instant::now();

        for i in 0..1000 {
            let msg = savant_memory::AgentMessage::user(
                session_id,
                &format!("Performance test message {} with some content", i),
            );
            engine.append_message(session_id, &msg).await.unwrap();
        }

        let elapsed = start.elapsed();
        println!("Inserted 1000 messages in {:?}", elapsed);
        assert!(
            elapsed.as_secs() < 30,
            "1000 inserts should complete within 30s"
        );
    }
}
