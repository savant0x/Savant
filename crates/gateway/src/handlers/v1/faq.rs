//! GET /v1/faq — returns curated FAQ items as JSON.
//!
//! Real impl. The FAQ is curated at the crate level (small list of common
//! questions + answers). Future FIDs can extend with a dynamic FAQ from
//! the agent's knowledge base.

use axum::Json;
use serde_json::{json, Value};

pub async fn v1_faq_handler() -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "items": [
            {
                "id": "what-is-savant",
                "category": "overview",
                "question": "What is Savant?",
                "answer": "Savant is a Rust-native AI shell — a CLI-first runtime host (per ZeroClaw pattern) that orchestrates 23+ Rust crates for memory, vault, agents, skills, channels, and the gateway HTTP/WS surface. The dashboard is a thin renderer served by the gateway.",
            },
            {
                "id": "how-to-start",
                "category": "operations",
                "question": "How do I start Savant?",
                "answer": "Run `pnpm dev` for the dashboard-only workflow, or `pnpm savant` (post-FID-030) for the full CLI runtime. The CLI imports `savant_gateway` + `savant_runtime` directly and serves the dashboard via the gateway's `embedded-web` feature.",
            },
            {
                "id": "v1-vs-api",
                "category": "architecture",
                "question": "What's the difference between /v1/* and /api/*?",
                "answer": "/api/* is the legacy gateway surface (30+ endpoints); /v1/* is the canonical Tauri-mapped + dashboard-`useCli` surface (33 new endpoints + SSE). Both share the same `GatewayState`. Future FIDs should prefer /v1/* for new work.",
            },
            {
                "id": "what-is-lesson-038",
                "category": "governance",
                "question": "What is LESSON-038?",
                "answer": "LESSON-038 (no-unilateral-defer) prohibits agents from marking FIDs as deferred without Spencer's explicit approval. Enforced by `pnpm lint:defer` + `scripts/lint-defer.sh`.",
            },
            {
                "id": "what-is-zero-claw",
                "category": "architecture",
                "question": "What is the ZeroClaw pattern?",
                "answer": "ZeroClaw is a reference architecture where the CLI is the runtime host (imports the gateway + runtime directly), the gateway is a single feature-flagged sub-crate, and Tauri is an optional thin shell. Savant's FID-030/031/032/033/034 follow this pattern.",
            },
        ],
    }))
}
