//! Crash recovery verification tests.

#![allow(clippy::disallowed_methods)]

#[cfg(test)]
mod crash_recovery {
    use savant_memory::{
        models::{AgentMessage, MessageRole},
        MemoryEngine, MockEmbeddingProvider,
    };
    use std::sync::Arc;

    fn make_engine(dir: &std::path::Path) -> Arc<MemoryEngine> {
        MemoryEngine::with_defaults(dir, Arc::new(MockEmbeddingProvider)).unwrap()
    }

    fn make_msg(i: usize) -> AgentMessage {
        AgentMessage {
            id: format!("msg-{}", i),
            session_id: "test".into(),
            role: MessageRole::User,
            content: format!("Content {}", i),
            tool_calls: vec![],
            tool_results: vec![],
            timestamp: (i as i64).into(),
            parent_id: None,
            channel: "Chat".into(),
        }
    }

    #[tokio::test]
    async fn test_append_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let engine = make_engine(dir.path());
        for i in 0..10 {
            engine.append_message("sess", &make_msg(i)).await.unwrap();
        }
        let msgs = engine.fetch_session_tail("sess", 5);
        assert!(
            msgs.len() >= 5,
            "Should read back at least 5 messages, got {}",
            msgs.len()
        );
    }

    #[tokio::test]
    async fn test_reopen_persists() {
        let dir = tempfile::tempdir().unwrap();
        {
            let engine = make_engine(dir.path());
            for i in 0..20 {
                engine
                    .append_message("crash-sess", &make_msg(i))
                    .await
                    .unwrap();
            }
        }
        let engine2 = make_engine(dir.path());
        let msgs = engine2.fetch_session_tail("crash-sess", 100);
        assert!(
            msgs.len() >= 15,
            "Should persist across reopens, got {}",
            msgs.len()
        );
    }

    #[tokio::test]
    async fn test_write_order_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let engine = make_engine(dir.path());
        for i in 0..50 {
            engine.append_message("ord", &make_msg(i)).await.unwrap();
        }
        let msgs = engine.fetch_session_tail("ord", 50);
        for i in 1..msgs.len() {
            assert!(
                i64::from(msgs[i].timestamp) <= i64::from(msgs[i - 1].timestamp),
                "Messages should be in newest-first timestamp order"
            );
        }
    }

    #[tokio::test]
    async fn test_multi_session_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let engine = make_engine(dir.path());
        engine.append_message("sess-a", &make_msg(1)).await.unwrap();
        engine.append_message("sess-b", &make_msg(2)).await.unwrap();
        let a = engine.fetch_session_tail("sess-a", 100);
        let b = engine.fetch_session_tail("sess-b", 100);
        assert!(a.iter().all(|m| m.content.contains("1")));
        assert!(b.iter().all(|m| m.content.contains("2")));
    }

    #[tokio::test]
    async fn test_bulk_insert_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let engine = make_engine(dir.path());
        for i in 0..100 {
            engine.append_message("bulk", &make_msg(i)).await.unwrap();
        }
        let msgs = engine.fetch_session_tail("bulk", 100);
        assert!(
            msgs.len() >= 90,
            "Should read back most messages, got {}",
            msgs.len()
        );
    }
}
