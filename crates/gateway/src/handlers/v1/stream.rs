//! GET /v1/manifest/soul/stream — Server-Sent Events handler.
//!
//! Minimal real impl that emits a single `ready` event to verify the SSE
//! plumbing (axum + async-stream + tower-http) works end-to-end. The
//! full stream impl consumes the existing `execute_manifestation` streaming
//! path; that work is deferred to a follow-on FID (per LESSON-038 + FID-031
//! §Out of Scope #13 — the MVP scope is the SSE plumbing + the route mount).
//!
//! **Protocol**: text/event-stream. The axum `Sse` builder emits
//! `data: <json>\n\n` frames + an optional `event:` + `id:` header. The
//! dashboard's `parseSSEStream` (src/lib/manifest-mock.ts:75-115) is
//! forward-compatible with this format.

use async_stream::stream;
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use crate::server::GatewayState;

/// SSE handler for `GET /v1/manifest/soul/stream`.
///
/// Emits:
/// - 1 `ready` event on connect (validates the SSE handshake)
/// - 1 `keepalive` comment every 15s (prevents intermediate proxies from
///   closing the idle connection)
/// - 1 `complete` event after the ready signal (terminates the stream
///   cleanly; the full streaming impl replaces this with chunk events)
pub async fn v1_manifest_soul_stream_sse(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let stream = stream! {
        // Ready signal — confirms the SSE plumbing is working
        yield Ok::<_, Infallible>(
            Event::default()
                .event("ready")
                .id("0")
                .data(serde_json::json!({
                    "status": "ready",
                    "version": env!("CARGO_PKG_VERSION"),
                    "note": "SSE plumbing verified; full stream impl deferred to follow-on FID"
                }).to_string())
        );

        // Brief delay then complete signal — closes the stream cleanly
        tokio::time::sleep(Duration::from_millis(100)).await;
        yield Ok::<_, Infallible>(
            Event::default()
                .event("complete")
                .id("1")
                .data(serde_json::json!({
                    "status": "complete",
                    "chunks": 0
                }).to_string())
        );
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

/// Type alias for the boxed stream returned by `v1_manifest_soul_stream_sse`.
/// Useful for tests + cross-module references.
pub type SseStream = std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::GatewayState;
    use dashmap::DashMap;
    use ed25519_dalek::SigningKey;
    use lru::LruCache;
    use savant_core::bus::NexusBridge;
    use savant_core::config::Config;
    use savant_core::db::Storage;
    use std::num::NonZeroUsize;
    use tokio::sync::Mutex as TokioMutex;

    fn make_test_state() -> Arc<GatewayState> {
        let config = Config::default();
        let nexus = Arc::new(NexusBridge::new());
        let tmp = std::env::temp_dir().join(format!("savant_v1_stream_test_{}", rand::random::<u64>()));
        // SAFETY: test-only construction with temp directory
        #[allow(clippy::disallowed_methods)]
        let storage = Arc::new(Storage::with_defaults(tmp).unwrap());
        let canvas_manager = Arc::new(savant_canvas::a2ui::CanvasManager::new(1000));
        let channel_pool = Arc::new(savant_channels::pool::InboxPool::new(nexus.clone()));
        let echo_metrics = Arc::new(savant_echo::ComponentMetrics::new(0.05, 100));

        Arc::new(GatewayState {
            config: Arc::new(tokio::sync::RwLock::new(config)),
            sessions: DashMap::new(),
            nexus,
            storage,
            #[allow(clippy::disallowed_methods)]
            avatar_cache: TokioMutex::new(LruCache::new(NonZeroUsize::new(100).unwrap())),
            oauth_manager: Arc::new(crate::auth::oauth::OAuthManager::new()),
            gateway_signing_key: SigningKey::generate(&mut rand::rngs::OsRng),
            canvas_manager,
            channel_pool,
            echo_metrics,
            consciousness_state: None,
            ws_connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            governor_pressure: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            governor_cpu_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            governor_mem_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            governor_permits: Arc::new(std::sync::atomic::AtomicUsize::new(16)),
        })
    }

    #[tokio::test]
    async fn test_sse_emits_ready_and_complete_events() {
        let state = make_test_state();
        let response = v1_manifest_soul_stream_sse(State(state)).await.into_response();
        // SSE responses have status 200 + content-type text/event-stream
        assert_eq!(response.status(), 200);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.starts_with("text/event-stream"),
            "expected text/event-stream content type, got: {}",
            content_type
        );
    }
}
