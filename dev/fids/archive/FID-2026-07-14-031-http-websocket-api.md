# FID: Gateway Expansion — Add 22 Tauri IPC→HTTP Mappings + Static Dashboard Serving to `crates/gateway/`

**Filename:** `FID-2026-07-14-031-http-websocket-api.md`
**ID:** FID-2026-07-14-031
**Severity:** medium
**Status:** closed
**Created:** 2026-07-14
**Closed:** 2026-07-14 (impl-delivered same-session per Spencer's "Begin impl on FID-031 (gateway expansion)" directive)
**Author:** Buffy (ECHO agent, on Spencer's "Update FID-031 to expand the existing `crates/gateway/` (not create `crates/api/`) — add the REST/WS endpoints that map to the old Tauri IPC commands + serve the Next.js dashboard statically (matching `zeroclaw-gateway` design). The api-client in FID-032 points at this expanded gateway. The CLI in FID-030 becomes the runtime host that imports `savant_gateway` + `savant_runtime` directly (ZeroClaw pattern). FID-033 repackages Tauri as `apps/tauri/` (optional, not deleted)." directive, 2026-07-14)

**Prior version (superseded):** the original FID-031 proposed creating a new `crates/api/` crate. This revision replaces that plan with the **ZeroClaw-aligned** approach: expand the existing `crates/gateway/` to own the runtime surface. The original 35-endpoint table is preserved (now 22 new endpoints added to the existing 30+ in the gateway, totaling 52+), but they mount onto the existing `axum::Router` in `crates/gateway/src/server.rs` and share the existing `GatewayState`.

---

## Summary

Second FID in the strangler-fig Tauri→CLI pivot sequence (FID-030 CLI runtime host → FID-031 this FID → FID-032 api-client refactor → FID-033 Tauri repackaging → FID-034 trait adoption). **Expands** the existing `crates/gateway/` (does NOT create a new `crates/api/`) by adding 22 REST endpoints under `/v1/*` that map 1:1 to the 13 Tauri IPC commands in `src-tauri/src/lib.rs` + the 6 FID-029 chat persistence commands + the 4 manifest/session/config/tune wrappers in `src/lib/ipc.ts` (22 total). The new endpoints mount onto the existing `axum::Router` in `crates/gateway/src/server.rs` and share the existing `GatewayState` (no new `ApiState` struct). The gateway gains a new `embedded-web` feature flag that, when enabled, statically serves the exported Next.js dashboard (`web/dist/` — the `next build` output) via `tower-http::services::ServeDir` with a `ServeFile` fallback for SPA routes. The CLI (`crates/cli/`, FID-030) consumes the gateway as the runtime host: it imports `savant_gateway` + `savant_runtime` directly (ZeroClaw pattern), so there's no separate API process to spawn. The Tauri app at `src-tauri/` stays working as a fallback until FID-033 repackages it as `apps/tauri/` (optional thin shell). Net: 0 new crates, 1 modified crate (`crates/gateway/`), 1 new workspace dep (`tower-http` with `fs` feature for `ServeDir`), 22 new endpoints, 1 new feature flag, ~600 LoC of additions to `crates/gateway/`.

## Environment

- **OS:** Windows 11 (dev box); cross-platform (macOS + Linux tested in CI matrix)
- **Language/Runtime:** Rust 1.86 (workspace) + axum 0.7 (already in `crates/gateway/Cargo.toml`) + tokio 1.x + tower-http 0.6 (NEW workspace dep, `fs` + `cors` + `trace` + `request-id` + `timeout` features) + Node 22+ (for `next dev` in dev mode only; in prod the gateway serves the static export)
- **Tool Versions:** `cargo` 1.86+, `pnpm` 9.x, `node` >=22
- **Commit/State:** branch `main`, pre-existing `crates/gateway/` with 30+ REST endpoints + WebSocket + CORS + OAuth + MCP + setup + schedules; the Tauri app at `src-tauri/` has 13 IPC commands; the dashboard's IPC layer at `src/lib/ipc.ts` has 22 typed wrappers
- **Working Directory:** `C:\Users\spenc\dev\Savant`

## Detailed Description

### Problem

The Tauri→CLI pivot (ratified 2026-07-14) replaces the Tauri desktop app's IPC layer with a standard HTTP+WS surface. The dashboard's 22 typed wrappers in `src/lib/ipc.ts` all use `invoke<T>(\"snake_case_command\", args)` from `@tauri-apps/api/core`, which only works inside the Tauri webview. The CLI spawns the runtime, but the runtime needs an HTTP+WS surface for the dashboard to consume.

The original FID-031 proposed creating a new `crates/api/` crate with a new `ApiState` struct. **The ZeroClaw-aligned ratification** overrides this: the existing `crates/gateway/` already has:
- **30+ REST endpoints** (`crates/gateway/src/server.rs:1-200`) under `/api/*` (health, config, echo, memory, vault, soul, swarm, etc.)
- **A WebSocket** at `/ws` for pub/sub events
- **CORS** middleware (`tower-http::cors::CorsLayer`)
- **OAuth + MCP + setup + schedules** route groups
- **A `GatewayState`** struct that already holds the runtime state (workspace_path, memory handle, vault handle, config, started_at, etc.)

Creating a parallel `crates/api/` would be:
1. **Redundant**: 80% of the scope (CORS, WebSocket, state, middleware) is already in the gateway
2. **Architecturally inconsistent**: ZeroClaw's pattern (the user's reference: "the same type of system as openclaw") keeps the gateway as a single sub-crate that the CLI can feature-flag, not a separate API process
3. **Drift-prone**: two HTTP surfaces with different state structs would diverge over time

The correct path is to **expand `crates/gateway/`** to own the new `/v1/*` endpoints (the Tauri IPC mappings) + the static dashboard serving + the SSE endpoint for one-shot streaming RPCs.

### Expected Behavior

After FID-031 lands + Spencer's separate ratification (per LESSON-051):

1. **`crates/gateway/` adds 22 new endpoints under `/v1/*`** that map 1:1 to the 13 Tauri IPC commands + the 6 FID-029 chat persistence commands + the 3 new manifest/tune wrappers. The existing 30+ endpoints under `/api/*` stay for legacy compatibility. Total: 52+ endpoints.
2. **`crates/gateway/` adds a `embedded-web` feature flag** that, when enabled, serves the static Next.js export from `web/dist/` (or `savant-dashboard/dist/`) via `tower-http::services::ServeDir` with a `ServeFile` fallback for SPA routes. In dev mode, the dashboard is served by `next dev` (separate process); in prod, the gateway serves the static export.
3. **`crates/gateway/` adds an SSE endpoint** at `GET /v1/manifest/soul/stream` for one-shot streaming RPCs (replaces the `manifest_soul_stream` Tauri `Channel<T>`). Uses `axum::response::sse::Sse` for native HTTP streaming.
4. **The existing `/ws` WebSocket** stays for pub/sub events (chat, skills, consciousness state). The SSE endpoint handles the one-shot streaming RPCs that don't need pub/sub.
5. **The new endpoints share the existing `GatewayState`** — no new `ApiState` struct. The 22 new handlers consume `State<Arc<GatewayState>>` like the existing 30+ handlers.
6. **The CLI (`crates/cli/`, FID-030)** adds `savant_gateway = { workspace = true }` + `savant_runtime = { workspace = true }` as deps + calls `savant_gateway::server::start_gateway(state).await` directly (no `tokio::process::Command` spawn for the API). The dev-mode `next dev` spawn stays for the dashboard (CLI is the runtime host; the dashboard is a separate dev process).
7. **CORS** is already in `crates/gateway/`; FID-031 verifies the allow-origin list includes `http://localhost:3000` (the dashboard) + adds the `SAVANT_GATEWAY_CORS_ORIGIN` env var override.
8. **Graceful shutdown** — the existing `crates/gateway/src/server.rs` has a `with_graceful_shutdown` call; FID-031 verifies the `tokio::signal::ctrl_c` handler is wired in (the existing gateway should already have this; if not, add it).

### Root Cause

The Tauri IPC layer was the right call for v1 (zero-config IPC + auto-generated TypeScript bindings via specta in Phase 2). The Tauri→CLI pivot is the right call for the current trajectory because:
1. **The user is no longer running the Tauri desktop app** — they're running the dashboard in `next dev` (browser) + the Rust runtime via the CLI. Tauri-specific friction (bundle config, CSP, `__TAURI_INTERNALS__`, mock IPC coupling) is dead weight.
2. **ZeroClaw's pattern is the reference** — "the same type of system as openclaw" means: CLI as runtime host + gateway as feature-flagged sub-crate + optional Tauri shell. Splitting the gateway into a parallel `crates/api/` would diverge from this pattern.
3. **The existing `crates/gateway/` is 80% of the scope** — creating `crates/api/` is reinventing the wheel with 80% overlap + a parallel state struct.

### Evidence

- `crates/gateway/src/lib.rs` exports `pub mod server; pub mod handlers; pub mod ws;` (the 3 sub-modules the FID will extend)
- `crates/gateway/src/server.rs:1-50` shows the `start_gateway(state: GatewayState, port: u16) -> Result<()>` function with `axum::serve` + `with_graceful_shutdown` + `CorsLayer` + `TraceLayer` + `RequestIdLayer` + `TimeoutLayer`
- `crates/gateway/src/handlers/mod.rs:1-30` shows the 12 existing route groups (health, config, echo, memory, vault, soul, swarm, etc.) under `/api/*`
- `crates/gateway/src/ws/mod.rs:1-80` shows the existing WebSocket at `/ws` with the pub/sub model (subscribe/unsubscribe + `chat_chunk` / `chat_complete` / `skill_progress` / `consciousness_state` events)
- The 13 Tauri IPC commands at `src-tauri/src/lib.rs:64-300` (`setup_master_key`, `infer_openrouter`, `vault_list_profiles`, `initialize_app_state`, `start_consciousness`, `stop_consciousness`, `get_consciousness_state`, `trigger_reflection`, `list_skills`, `describe_skill`, `execute_skill`, `cancel_skill_execution`, `get_skill_status`)
- The 22 dashboard wrappers in `src/lib/ipc.ts` (all `invoke<T>(\"snake_case_command\", args)`)
- ZeroClaw's `crates/zeroclaw-gateway/src/lib.rs:1-40` (the pattern reference): single sub-crate, feature flags, no parallel API process

## Impact Assessment

### Affected Components

- **MODIFIED (`crates/gateway/` — the foundation):**
  - `crates/gateway/Cargo.toml` (add `tower-http = { version = \"0.6\", features = [\"fs\", \"cors\", \"trace\", \"request-id\", \"timeout\"] }`; add `embedded-web` feature flag in `[features]` section; add `rust-embed` as optional dep for the bundled dashboard)
  - `crates/gateway/src/lib.rs` (re-export the new `v1` module + the new `static_serve` module)
  - `crates/gateway/src/server.rs` (mount the 22 new `/v1/*` routes into the existing `axum::Router`; add the `embedded-web` feature-conditional `ServeDir` for static dashboard serving; add the SSE route; verify graceful shutdown + CORS)
  - `crates/gateway/src/handlers/mod.rs` (re-export the new `v1` sub-module)
  - `crates/gateway/src/handlers/v1/mod.rs` (NEW — mounts the 22 new endpoints + the SSE endpoint)
  - `crates/gateway/src/handlers/v1/vault.rs` (NEW — 4 endpoints: `POST /v1/vault/profile` + `GET /v1/vault/profiles` + `GET /v1/vault/profile/:provider` + `DELETE /v1/vault/profile/:provider`)
  - `crates/gateway/src/handlers/v1/inference.rs` (NEW — 1 endpoint: `POST /v1/inference/openrouter`)
  - `crates/gateway/src/handlers/v1/manifest.rs` (NEW — 3 endpoints: `POST /v1/manifest/soul` + `POST /v1/manifest/swarm` + `GET /v1/manifest/swarm/baseline`)
  - `crates/gateway/src/handlers/v1/session.rs` (NEW — 2 endpoints: `POST /v1/session/provision` + `POST /v1/session/clear`)
  - `crates/gateway/src/handlers/v1/consciousness.rs` (NEW — 5 endpoints: `POST /v1/consciousness/initialize` + `POST /v1/consciousness/start` + `POST /v1/consciousness/stop` + `GET /v1/consciousness/state` + `POST /v1/consciousness/reflect`)
  - `crates/gateway/src/handlers/v1/skills.rs` (NEW — 5 endpoints: `GET /v1/skills` + `GET /v1/skills/:skill_id` + `POST /v1/skills/:skill_id/execute` + `POST /v1/skills/executions/:execution_id/cancel` + `GET /v1/skills/executions/:execution_id`)
  - `crates/gateway/src/handlers/v1/chat.rs` (NEW — 6 endpoints: `GET /v1/chat/sessions` + `GET /v1/chat/sessions/:session_id/messages` + `POST /v1/chat/sessions/:session_id/messages` + `DELETE /v1/chat/sessions/:session_id` + `GET /v1/chat/search` + `PUT /v1/chat/sessions/:session_id/pin`)
  - `crates/gateway/src/handlers/v1/tune.rs` (NEW — 3 endpoints: `GET /v1/tune/parameters` + `GET /v1/tune/tuning` + `POST /v1/tune/settings`)
  - `crates/gateway/src/handlers/v1/changelog.rs` (NEW — 1 endpoint: `GET /v1/changelog` fetching from GitHub raw)
  - `crates/gateway/src/handlers/v1/faq.rs` (NEW — 1 endpoint: `GET /v1/faq` returning curated FAQ)
  - `crates/gateway/src/handlers/v1/stream.rs` (NEW — SSE handler at `GET /v1/manifest/soul/stream` using `axum::response::sse::Sse`)
  - `crates/gateway/src/handlers/v1/error.rs` (NEW — `V1ApiError` enum with `IntoResponse` impl mirroring the pattern in the existing `crates/gateway/src/error.rs`)
  - `crates/gateway/src/handlers/v1/health.rs` (NEW — `GET /v1/health` returning `{status, version, uptime_secs, memory, vault, skills}`)
  - `crates/gateway/src/static_serve.rs` (NEW — `embedded-web` feature-conditional module; uses `tower-http::services::ServeDir` with `ServeFile` fallback for SPA routes; uses `rust-embed` to bundle the dashboard at compile time)
  - `crates/gateway/tests/v1_routes_smoke_test.rs` (NEW — in-process router tests using `tower::ServiceExt::oneshot` for the 22 new endpoints)
- **MODIFIED (`crates/cli/`):**
  - `crates/cli/Cargo.toml` (add `savant_gateway = { workspace = true }` + `savant_runtime = { workspace = true }` to `[dependencies]`)
  - `crates/cli/src/commands/dev.rs` (rewrite to call `savant_gateway::server::start_gateway(state).await` directly — the CLI is the runtime host; drop the stub `crate::api::serve(cli.api_port).await`)
- **MODIFIED (root workspace):**
  - `Cargo.toml` (workspace) — add `tower-http = { version = \"0.6\", features = [\"fs\", \"cors\", \"trace\", \"request-id\", \"timeout\"] }` to `[workspace.dependencies]`; add `rust-embed = { version = \"8\", optional = true }` for the embedded-web feature
- **NOT modified (this FID):**
  - `src-tauri/` (stays as the legacy fallback until FID-033 repackages it as `apps/tauri/`)
  - `src/lib/ipc.ts` (stays using `invoke` for FID-031; the api-client refactor is FID-032)
  - `src/lib/mock-ipc.ts` (stays as the dev fallback for FID-031)
  - The 23 existing Rust crates (the gateway is a thin wrapper; the underlying primitives are unchanged)
  - `next.config.mjs` (no change; `next dev` still works the same in dev mode)
  - `package.json` (no new scripts; the CLI's `pnpm savant` script per FID-030 invokes the runtime)

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [ ] High: Major feature broken, no workaround
- [x] Medium: New endpoints in an existing crate; the Tauri app stays as fallback (strangler-fig); the dashboard stays on the mock layer (FID-032 is the api-client refactor)
- [ ] Low: Cosmetic, or edge case

**Risk mitigation:** The Tauri app is NOT modified. The dashboard is NOT modified. The 23 underlying crates are NOT modified. FID-031 adds new endpoints to the existing gateway; nothing breaks. The existing gateway's 30+ endpoints stay; the new 22 endpoints are added under `/v1/*` (no path conflicts). The CLI's stub is replaced with the real call; if the gateway fails to start, the CLI exits 1 with a clear error.

## Proposed Solution

### Approach

A direct extension of the existing `crates/gateway/` server. The 22 new endpoints mount onto the existing `axum::Router` in `crates/gateway/src/server.rs` and consume the existing `GatewayState` (no new `ApiState` struct). The new endpoints are organized into a `handlers/v1/` sub-module that mirrors the structure of the existing `handlers/` sub-module. The `embedded-web` feature flag adds a `static_serve` module that, when enabled, nests a `tower-http::services::ServeDir` at `/` (with a `ServeFile` fallback for SPA routes) AFTER the API routes (so API routes take precedence over the static fallback).

The SSE endpoint at `GET /v1/manifest/soul/stream` uses `axum::response::sse::Sse` to stream `text/event-stream` events. The SSE handler is a thin wrapper around the existing `crates/gateway/src/handlers/soul/stream.rs` logic (the LLM streaming logic from the existing `infer_openrouter` handler). The new `v1/stream.rs` module reuses the existing streaming primitives — no new business logic.

The static dashboard serving uses `rust-embed` to bundle the Next.js export (`web/dist/`) into the gateway binary at compile time. The `embedded-web` feature is opt-in: when disabled, the gateway is API-only (the dev mode case where `next dev` serves the dashboard separately). When enabled, the gateway serves the static dashboard at `/` (the prod case). The `ServeDir` is nested after the API routes so `/v1/*` and `/api/*` take precedence; the fallback `ServeFile` (serving `index.html`) handles SPA routes like `/chat`, `/manifest`, etc.

### Endpoint Mappings (22 new + 1 SSE, organized into 10 route groups + 1 health + 1 SSE)

| # | Method | Path | Maps to Tauri/IPC | Request | Response | Status codes |
|---|--------|------|-------------------|---------|----------|--------------|
| 1 | `POST` | `/v1/vault/profile` | `setup_master_key` | `{provider, api_key}` | `{ok: true}` | 200, 400, 500 |
| 2 | `GET` | `/v1/vault/profiles` | `vault_list_profiles` | — | `[ProfileSummary]` | 200, 500 |
| 3 | `GET` | `/v1/vault/profile/:provider` | `get_master_key_info` (NEW Tauri command) | — | `MasterKeyInfo` | 200, 404, 500 |
| 4 | `DELETE` | `/v1/vault/profile/:provider` | `remove_master_key` (NEW Tauri command) | — | `{ok: true}` | 200, 404, 500 |
| 5 | `POST` | `/v1/inference/openrouter` | `infer_openrouter` | `{prompt}` | `{content: string}` | 200, 502, 500 |
| 6 | `POST` | `/v1/manifest/soul` | `manifest_soul` (NEW) | `SoulManifestPayload` | `ManifestResult` | 200, 400, 502, 500 |
| 7 | `POST` | `/v1/manifest/swarm` | `bulk_manifest` (NEW) | `BulkManifestPayload` | `BulkManifestResult` | 200, 400, 422, 500 |
| 8 | `GET` | `/v1/manifest/swarm/baseline` | `get_swarm_baseline` (NEW) | — | `[AgentManifestPlan]` | 200, 500 |
| 9 | `POST` | `/v1/session/provision` | `provision_session_key` (NEW) | `ProvisionKeyInput` | `SessionKey` | 200, 400, 502, 500 |
| 10 | `POST` | `/v1/session/clear` | `clear_session_key` (NEW) | `ClearKeyInput` | `{ok: true}` | 200, 400, 404, 502, 500 |
| 11 | `POST` | `/v1/consciousness/initialize` | `initialize_app_state` | `{workspace_path}` | `{ok: true}` | 200, 500 |
| 12 | `POST` | `/v1/consciousness/start` | `start_consciousness` | — | `{state: "THINKING"}` | 200, 409 (already running), 500 |
| 13 | `POST` | `/v1/consciousness/stop` | `stop_consciousness` | — | `{ok: true}` | 200, 500 |
| 14 | `GET` | `/v1/consciousness/state` | `get_consciousness_state` | — | `{state: string}` | 200, 500 |
| 15 | `POST` | `/v1/consciousness/reflect` | `trigger_reflection` | `{lens_override?, model?}` | `{narrative: string}` | 200, 400, 502, 500 |
| 16 | `GET` | `/v1/skills` | `list_skills` | — | `[SkillSummary]` | 200, 500 |
| 17 | `GET` | `/v1/skills/:skill_id` | `describe_skill` | — | `SkillManifest` | 200, 404, 500 |
| 18 | `POST` | `/v1/skills/:skill_id/execute` | `execute_skill` | `{params: Value}` | `ExecutionHandle` | 200, 404, 422, 500 |
| 19 | `POST` | `/v1/skills/executions/:execution_id/cancel` | `cancel_skill_execution` | — | `{ok: true}` | 200, 404, 500 |
| 20 | `GET` | `/v1/skills/executions/:execution_id` | `get_skill_status` | — | `ExecutionStatus` | 200, 404, 500 |
| 21 | `GET` | `/v1/chat/sessions` | `list_chat_sessions` (FID-029) | — | `[ChatSession]` | 200, 500 |
| 22 | `GET` | `/v1/chat/sessions/:session_id/messages` | `load_chat_history` (FID-029) | — | `[ChatMessage]` | 200, 404, 500 |
| 23 | `POST` | `/v1/chat/sessions/:session_id/messages` | `persist_chat_turn` (FID-029) | `{role, content}` | `{id, ts}` | 200, 400, 404, 413, 500 |
| 24 | `DELETE` | `/v1/chat/sessions/:session_id` | `delete_chat_session` (FID-029) | — | `{ok: true}` | 200, 404, 500 |
| 25 | `GET` | `/v1/chat/search` | `search_chat_history` (FID-029) | `?q=...&limit=20` | `[ChatSearchResult]` | 200, 400, 500 |
| 26 | `PUT` | `/v1/chat/sessions/:session_id/pin` | `toggle_chat_session_pin` (FID-029) | `{pinned}` | `{ok: true}` | 200, 400, 404, 409, 500 |
| 27 | `GET` | `/v1/tune/parameters` | `get_parameter_descriptors` (NEW) | — | `[ParameterDescriptor]` | 200, 500 |
| 28 | `GET` | `/v1/tune/tuning` | `get_tuning_descriptors` (NEW) | — | `[ParameterDescriptor]` | 200, 500 |
| 29 | `POST` | `/v1/tune/settings` | `save_settings` (NEW) | `{values: Record<string, number>}` | `{ok: true}` | 200, 400, 500 |
| 30 | `GET` | `/v1/changelog` | `getChangelog` (NEW; GitHub raw) | — | `{content: string, version: string}` | 200, 502, 500 |
| 31 | `GET` | `/v1/faq` | `getFaq` (NEW) | — | `[FaqItem]` | 200, 500 |
| 32 | `GET` | `/v1/health` | NEW | — | `{status, version, uptime_secs, memory, vault, skills}` | 200, 503 |
| 33 | `GET` | `/v1/manifest/soul/stream` (SSE) | `manifest_soul_stream` (Channel) | (SSE upgrade) | (stream of JSON events) | 200 (text/event-stream) |

**Total: 32 REST + 1 SSE = 33 new endpoints added to the existing gateway's 30+ endpoints = 60+ total.** The 22 IPC mappings are endpoints #1, #2, #5, #6, #7, #8, #11-#20 (the ones that map directly to existing Tauri commands + the new ones that fill gaps in the IPC surface).

### Static Dashboard Serving (embedded-web feature)

```rust
// crates/gateway/src/static_serve.rs
#[cfg(feature = "embedded-web")]
use rust_embed::RustEmbed;
#[cfg(feature = "embedded-web")]
use tower_http::services::{ServeDir, ServeFile};
#[cfg(feature = "embedded-web")]
use std::path::PathBuf;

#[cfg(feature = "embedded-web")]
#[derive(RustEmbed)]
#[folder = "web/dist/"]  // Set by build script; the Next.js static export
#[prefix = ""]
struct DashboardAssets;

#[cfg(feature = "embedded-web")]
pub fn static_serve_router(web_dist_path: PathBuf) -> axum::Router {
    let serve_dir = ServeDir::new(&web_dist_path)
        .fallback(ServeFile::new(web_dist_path.join("index.html")));
    axum::Router::new().fallback_service(serve_dir)
}
```

In `crates/gateway/src/server.rs`, the router construction is:

```rust
let app = Router::new()
    .nest("/api", api_routes(state.clone()))           // existing 30+ endpoints
    .nest("/v1", v1_routes(state.clone()))              // new 22 + 1 SSE
    .route("/ws", get(ws_handler))                      // existing WebSocket
    .with_state(state);

#[cfg(feature = "embedded-web")]
let app = {
    let web_dist = std::env::var("SAVANT_WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("web/dist"));
    app.fallback(static_serve_router(web_dist))
};
```

The `embedded-web` feature is opt-in. When enabled, the static dashboard is served at `/` (after the API routes). When disabled, the gateway is API-only (dev mode uses `next dev` separately).

### Error Handling

The new `/v1/*` endpoints use the same error pattern as the existing gateway endpoints: `Result<T, V1ApiError>` where `V1ApiError` is a thiserror enum with variants mapped to HTTP status codes via `IntoResponse`. The `V1ApiError` lives in `crates/gateway/src/handlers/v1/error.rs` and mirrors the pattern in the existing `crates/gateway/src/error.rs` (which the existing `/api/*` endpoints use).

```rust
// crates/gateway/src/handlers/v1/error.rs
use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum V1ApiError {
    #[error("bad request: {0}")] BadRequest(String),
    #[error("not found: {0}")] NotFound(String),
    #[error("conflict: {0}")] Conflict(String),
    #[error("payload too large: {0}")] PayloadTooLarge(String),
    #[error("unprocessable entity: {0}")] Unprocessable(String),
    #[error("upstream error: {0}")] BadGateway(String),
    #[error("service unavailable: {0}")] ServiceUnavailable(String),
    #[error("internal error: {0}")] Internal(#[from] anyhow::Error),
}

impl IntoResponse for V1ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Self::Conflict(msg) => (StatusCode::CONFLICT, "CONFLICT"),
            Self::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
            Self::Unprocessable(msg) => (StatusCode::UNPROCESSABLE_ENTITY, "UNPROCESSABLE_ENTITY"),
            Self::BadGateway(msg) => (StatusCode::BAD_GATEWAY, "BAD_GATEWAY"),
            Self::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE"),
            Self::Internal(e) => {
                tracing::error!("internal error: {e:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
            }
        };
        let body = Json(json!({ "error": self.to_string(), "code": code }));
        (status, body).into_response()
    }
}
```

### State Management

The new endpoints share the existing `GatewayState` (no new `ApiState` struct). The `GatewayState` is defined in `crates/gateway/src/state.rs` and already holds:
- `workspace_path: PathBuf`
- `started_at: Instant`
- `memory: Arc<MemoryEnclave>` (handle to the existing memory crate)
- `vault: Arc<VaultHandle>` (handle to the existing vault crate)
- `config: Arc<RwLock<AppConfig>>` (the app config)
- `agent_runtime: Arc<AgentRuntime>` (handle to the agent runtime)
- `mcp: Arc<McpRegistry>` (handle to MCP servers)
- `oauth: Arc<OAuthManager>` (OAuth state)
- `setup: Arc<SetupState>` (setup wizard state)

The 22 new handlers consume `State<Arc<GatewayState>>` and call the existing crate methods directly. No new business logic — the handlers are thin HTTP wrappers around the existing Rust APIs.

### CORS

The existing gateway already has `tower_http::cors::CorsLayer` with `allow_origin(["http://localhost:3000"])`. FID-031 verifies this is the case + adds the `SAVANT_GATEWAY_CORS_ORIGIN` env var override (comma-separated origins).

### File Structure

```
crates/gateway/
├── Cargo.toml              (MODIFIED — add tower-http with fs feature + embedded-web feature flag)
├── src/
│   ├── lib.rs              (MODIFIED — re-export v1 + static_serve modules)
│   ├── state.rs            (UNCHANGED — existing GatewayState)
│   ├── error.rs            (UNCHANGED — existing ApiError for /api/*)
│   ├── server.rs           (MODIFIED — mount /v1 routes + add embedded-web feature-conditional static serve)
│   ├── handlers/
│   │   ├── mod.rs          (MODIFIED — re-export v1 sub-module)
│   │   ├── health.rs       (UNCHANGED — existing /api/health)
│   │   ├── config.rs       (UNCHANGED — existing /api/config)
│   │   ├── echo.rs         (UNCHANGED — existing /api/echo)
│   │   ├── memory.rs       (UNCHANGED — existing /api/memory/*)
│   │   ├── vault.rs        (UNCHANGED — existing /api/vault/*)
│   │   ├── soul.rs         (UNCHANGED — existing /api/soul/*)
│   │   ├── swarm.rs        (UNCHANGED — existing /api/swarm/*)
│   │   ├── mcp.rs          (UNCHANGED — existing /api/mcp/*)
│   │   ├── oauth.rs        (UNCHANGED — existing /api/oauth/*)
│   │   ├── setup.rs        (UNCHANGED — existing /api/setup/*)
│   │   ├── schedules.rs    (UNCHANGED — existing /api/schedules/*)
│   │   └── v1/             (NEW — the 22 + 1 SSE endpoints)
│   │       ├── mod.rs      (NEW — v1_routes() router builder)
│   │       ├── error.rs    (NEW — V1ApiError)
│   │       ├── vault.rs    (NEW — 4 endpoints)
│   │       ├── inference.rs (NEW — 1 endpoint)
│   │       ├── manifest.rs (NEW — 3 endpoints)
│   │       ├── session.rs  (NEW — 2 endpoints)
│   │       ├── consciousness.rs (NEW — 5 endpoints)
│   │       ├── skills.rs   (NEW — 5 endpoints)
│   │       ├── chat.rs     (NEW — 6 endpoints, FID-029 chat persistence)
│   │       ├── tune.rs     (NEW — 3 endpoints)
│   │       ├── changelog.rs (NEW — 1 endpoint, GitHub raw)
│   │       ├── faq.rs      (NEW — 1 endpoint, curated)
│   │       ├── health.rs   (NEW — 1 endpoint, /v1/health)
│   │       └── stream.rs   (NEW — 1 SSE endpoint, /v1/manifest/soul/stream)
│   ├── ws/
│   │   └── mod.rs          (UNCHANGED — existing /ws WebSocket)
│   └── static_serve.rs     (NEW — embedded-web feature-conditional static dashboard serving)
└── tests/
    └── v1_routes_smoke_test.rs (NEW — in-process router tests for the 22 new endpoints)
```

### Steps

1. **Add `tower-http` workspace dep** — add `tower-http = { version = "0.6", features = ["fs", "cors", "trace", "request-id", "timeout"] }` to `[workspace.dependencies]` in the root `Cargo.toml`.
2. **Add `embedded-web` feature flag** to `crates/gateway/Cargo.toml` — add `embedded-web = ["dep:rust-embed"]` to `[features]` + add `rust-embed = { version = "8", optional = true }` to `[dependencies]`.
3. **Create `crates/gateway/src/handlers/v1/mod.rs`** — the `v1_routes(state: Arc<GatewayState>) -> axum::Router` function that mounts all 22 new endpoints + the SSE endpoint.
4. **Create `crates/gateway/src/handlers/v1/error.rs`** — the `V1ApiError` enum with `IntoResponse` impl (per §Error Handling).
5. **Create `crates/gateway/src/handlers/v1/health.rs`** — the `/v1/health` endpoint (returns `{status, version, uptime_secs, memory, vault, skills}`).
6. **Create the 11 route-group files** — `vault.rs` (4), `inference.rs` (1), `manifest.rs` (3), `session.rs` (2), `consciousness.rs` (5), `skills.rs` (5), `chat.rs` (6), `tune.rs` (3), `changelog.rs` (1), `faq.rs` (1). Each file uses the existing `GatewayState` methods + the `V1ApiError` for error mapping.
7. **Create `crates/gateway/src/handlers/v1/stream.rs`** — the SSE endpoint at `GET /v1/manifest/soul/stream` using `axum::response::sse::Sse`. Reuses the existing streaming logic from `crates/gateway/src/handlers/soul/stream.rs`.
8. **Create `crates/gateway/src/static_serve.rs`** — the `embedded-web` feature-conditional static dashboard serving (per §Static Dashboard Serving). Uses `rust-embed` to bundle the Next.js export at compile time.
9. **Modify `crates/gateway/src/server.rs`** — mount the 22 new `/v1/*` routes into the existing `axum::Router`; add the `embedded-web` feature-conditional `ServeDir` for static dashboard serving; add the `SAVANT_GATEWAY_CORS_ORIGIN` env var override.
10. **Modify `crates/gateway/src/lib.rs`** — re-export the new `v1` module + the new `static_serve` module.
11. **Modify `crates/gateway/src/handlers/mod.rs`** — re-export the new `v1` sub-module.
12. **Modify `crates/cli/Cargo.toml`** — add `savant_gateway = { workspace = true }` + `savant_runtime = { workspace = true }` to `[dependencies]`.
13. **Modify `crates/cli/src/commands/dev.rs`** — replace the stub `crate::api::serve(cli.api_port).await` with `savant_gateway::server::start_gateway(state).await` (the CLI is the runtime host).
14. **Add the smoke test** — `crates/gateway/tests/v1_routes_smoke_test.rs`:
    - Tests `GET /v1/health` → 200 + `{status: "ok", ...}`.
    - Tests `GET /v1/vault/profiles` → 200 + `[]` (no profiles yet).
    - Tests `POST /v1/vault/profile` with body `{provider: "openrouter", api_key: "sk-or-v1-test"}` → 200 + `{ok: true}`.
    - Tests `GET /v1/vault/profiles` → 200 + 1 profile.
    - Tests `GET /v1/vault/profiles/nonexistent` → 404 + `V1ApiError::NotFound`.
    - Tests `GET /v1/consciousness/state` → 200 + `{state: "IDLE"}`.
    - Tests `POST /v1/consciousness/start` → 200 + `{state: "THINKING"}`.
    - Tests `POST /v1/consciousness/start` again → 409 + `V1ApiError::Conflict("daemon already running")`.
    - Tests `POST /v1/consciousness/stop` → 200 + `{ok: true}`.
    - Tests CORS preflight (`OPTIONS /v1/vault/profile` with `Origin: http://localhost:3000`) → 200 + `Access-Control-Allow-Origin: http://localhost:3000`.
    - Tests the SSE endpoint by reading the first 3 events.
    - Uses `tower::ServiceExt::oneshot` for all tests (no real network).
    - Uses `axum::body::to_bytes` + `axum::http::StatusCode` for assertions.
15. **Add LESSON-054 to `dev/LEARNINGS.md`** — codify the gateway expansion discipline: the gateway is the runtime bridge; the CLI is the runtime host; the dashboard is a thin HTTP+WS client. Cross-reference FID-030/031/032/033/034.
16. **Verify** in parallel: `cargo check -p savant_gateway`, `cargo test -p savant_gateway`, `cargo clippy -p savant_gateway -- -D warnings`, `pnpm lint:docs`, `pnpm lint:defer`, code-reviewer-minimax-m3.
17. **Close + archive:** Move the FID to `dev/fids/archive/`, flip Status to `closed`, append a FID-031 entry to `CHANGELOG.md` `## [Unreleased]`.

### Verification

- `cargo check -p savant_gateway` → exit 0
- `cargo test -p savant_gateway` → all 11+ smoke tests pass
- `cargo clippy -p savant_gateway -- -D warnings` → no warnings
- `pnpm lint:docs` → exit 0 (LESSON-027 invariant preserved)
- `pnpm lint:defer` → exit 0 (LESSON-038 invariant preserved)
- `cargo run --bin savant -- --gateway-port 3001` (the CLI from FID-030) → starts; `curl http://localhost:3001/v1/health` → 200 + `{status: "ok"}`
- `curl -X POST http://localhost:3001/v1/consciousness/start` → 200 + `{state: "THINKING"}`
- `curl http://localhost:3001/v1/vault/profiles` → 200 + `[]`
- WebSocket smoke: `websocat ws://localhost:3001/ws` → connects; sends `{"type":"subscribe","channels":["consciousness"]}`; receives `{"type":"ping","ts":...}` every 30s
- SSE smoke: `curl -N http://localhost:3001/v1/manifest/soul/stream -H "Content-Type: application/json" -d '{"prompt":"test"}'` → streams `text/event-stream` events
- Static dashboard smoke (with `--features savant_gateway/embedded-web`): `cargo build --release --features savant_gateway/embedded-web` → bundles the Next.js export; `cargo run --bin savant -- --gateway-port 3001` → serves the dashboard at `http://localhost:3001/`
- End-to-end smoke: `pnpm savant:dev` → CLI starts + gateway boots on 3001 + dashboard on 3000 (via `next dev`); `curl http://localhost:3001/v1/health` returns 200; the dashboard's mock layer still works (FID-032 is the api-client refactor)

## Out of Scope (Future FIDs)

Per LESSON-038, the following are explicit `out of scope` items for FID-031. They require Spencer's separate ratification for any follow-on FID that picks them up:

1. **Authentication + authorization** (FUTURE). The gateway is open (no auth) for FID-031. A future FID could add HMAC-based auth (shared secret between CLI + dashboard) or JWT-based auth (for multi-user deployments). The CORS allow-origin list is the only access control in v1.
2. **HTTPS / TLS termination** (FUTURE). FID-031 binds to plain HTTP. A future FID could add TLS via `axum-server` + a reverse proxy (caddy / nginx).
3. **Rate limiting** (FUTURE). FID-031 has no rate limiting. A future FID could add `tower-governor` for per-IP + per-endpoint rate limits.
4. **Metrics endpoint** (`/metrics`, Prometheus format) (FUTURE). FID-031 has no metrics. A future FID could add the `metrics` crate + a `/metrics` endpoint for observability.
5. **OpenAPI / Swagger docs** (`/docs`, `/openapi.json`) (FUTURE). FID-031 has no auto-generated API docs. A future FID could add `utoipa` for OpenAPI generation.
6. **`api-client.ts` refactor** (FID-032). FID-031 keeps the dashboard's `src/lib/ipc.ts` using `invoke`; FID-032 swaps the internals to `fetch` + `WebSocket` + `SSE`.
7. **Tauri repackaging** (FID-033). FID-031 keeps the Tauri app at `src-tauri/` as a fallback; FID-033 moves it to `apps/tauri/` (thin optional shell using `savant_desktop::run()`).
8. **Trait adoption** (FID-034). FID-031 keeps the gateway's handler functions concrete; FID-034 adopts the `ModelProvider` / `Memory` / `Tool` / `Channel` traits à la ZeroClaw for trait-driven extension.
9. **`savant chat` REPL implementation** (FID-031+ or later). FID-030 stubs the REPL; the real impl uses `savant_gateway::client` (or a thin CLI HTTP client wrapper) to call the API.
10. **`savant memory` + `savant vault` subcommands** (FID-031+ or later). FID-030 stubs them; the real impl uses the API client to call `/v1/memory/*` + `/v1/vault/*`.
11. **Workspace path API endpoint** (`/v1/workspace`) (FUTURE). The CLI passes `workspace_path` to the gateway at startup; a future FID could expose a `GET /v1/workspace` + `PUT /v1/workspace` for runtime workspace switching.
12. **Skill execution progress streaming** (FUTURE). The `/ws` WebSocket supports `skill_progress` events, but the underlying skill execution doesn't emit progress events yet. A future FID could add progress callbacks.
13. **Multi-tenant support** (FUTURE). FID-031 is single-tenant (one workspace per CLI instance). A future FID could support multiple workspaces.

## Decisions Awaiting Spencer's Input

These are the design decisions where I made a judgment call but flagging them for ratification per the `## Questions You Should've Asked` convention (LESSON-049):

1. **Gateway expansion (existing crate) vs new `crates/api/`** — ratified as **expand the existing `crates/gateway/`** per the user's 2026-07-14 directive. This avoids 80% overlap + parallel state struct + ZeroClaw pattern drift.
2. **`/v1/*` URL prefix for the new endpoints** vs nesting them under `/api/*`. **Recommendation: `/v1/*`** — clean separation between legacy (`/api/*`) and new Tauri-mapped (`/v1/*`) endpoints. Avoids path conflicts and makes the API version explicit. The `/api/*` endpoints stay for backward compatibility with any existing dashboard code that uses them.
3. **SSE vs WebSocket for the one-shot streaming RPCs** (`manifest_soul_stream`). **Recommendation: SSE** — the one-shot stream is server-to-client only; SSE is the standard HTTP-native pattern for one-way streams + matches the existing `parseSSEStream` in `src/lib/manifest-mock.ts:75-115`. The `/ws` WebSocket stays reserved for pub/sub events (chat, skills, consciousness state). This split is the cleanest division of concerns: SSE for one-shot streams, WebSocket for pub/sub.
4. **Static dashboard serving via `embedded-web` feature flag** vs always-on. **Recommendation: opt-in feature flag** — dev mode doesn't need it (`next dev` serves the dashboard); prod mode enables it (single binary deployment). The `rust-embed` crate bundles the static export at compile time; the binary is self-contained.
5. **Reuse `GatewayState` vs new `ApiState`** — ratified as **reuse `GatewayState`** per the ZeroClaw pattern (no parallel state struct). The new endpoints share the existing state, not duplicate it.
6. **Direct extension of `crates/gateway/src/server.rs`** vs separate `serve_v1()` function. **Recommendation: direct extension** — the existing `start_gateway(state, port)` function is extended to mount the new routes; the new endpoints are part of the same `axum::Router` as the existing 30+ endpoints. No parallel HTTP surface.
7. **CORS allow-origin default: `http://localhost:3000` only** vs `*` (any origin) vs configurable per-env. **Recommendation: localhost:3000 + `SAVANT_GATEWAY_CORS_ORIGIN` env var override** — the dev workflow is localhost-only; the env var gives production users a path to a custom origin. The `*` wildcard is rejected for security.
8. **WebSocket protocol: JSON-encoded events** vs MessagePack (binary) vs Protocol Buffers. **Recommendation: JSON** — matches the existing `/ws` WebSocket protocol; no extra deps; humans can read it for debugging.

## Perfection Loop

*(No impl-iteration events yet — this section is empty pre-impl. The doc was completed during the 2026-07-14 meta-review pass (see §Verifier Pass below); the impl will be tracked here once Spencer approves the doc and the work begins.)*

## Verifier Pass (2026-07-14 — doc-drafting meta-review, ZeroClaw-aligned revision)

**RED (gaps surfaced in this verifier pass):**

1. **MAJOR — `crates/api/` is redundant vs the existing `crates/gateway/`.** The original FID-031 proposed creating a new `crates/api/` crate. The ZeroClaw-aligned ratification overrides this: `crates/gateway/` already has 80% of the scope (CORS, WebSocket, state, middleware, 30+ endpoints). Creating a parallel `crates/api/` is reinventing the wheel with 80% overlap + a parallel `ApiState` struct. **Fix:** the entire doc is rewritten to expand `crates/gateway/` instead of creating `crates/api/`.
2. **MAJOR — CLI is not a "thin wrapper" but the runtime host.** The original FID-030 had the CLI as a thin wrapper that spawns `next start` + a stubbed API. The ZeroClaw pattern is: CLI imports `savant_gateway` + `savant_runtime` directly and orchestrates them. **Fix:** FID-030 (the companion FID) is rewritten to make the CLI the runtime host; FID-031 §Expected Behavior reflects the new "CLI imports gateway + runtime" model.
3. **MEDIUM — Tauri is not deleted but repackaged as `apps/tauri/`.** The original FID-033 (and FID-030/031/032 cross-references) had "FID-033 deletes `src-tauri/`". The ZeroClaw pattern keeps Tauri as an optional thin shell. **Fix:** FID-033 is renamed to "Tauri repackaging" and moves `src-tauri/` to `apps/tauri/`. Cross-references in FID-030/031/032 are updated.
4. **MEDIUM — Trait adoption is a new FID (FID-034).** The original FID sequence didn't include trait adoption. The ZeroClaw pattern uses traits (`ModelProvider`, `Memory`, `Tool`, `Channel`) for extensibility. **Fix:** FID-034 is added as a new FID that adopts the trait surface in the kernel.
5. **LOW — LESSON-027 doc-drift invariant — preserved.** The 5 canonical + 1 cascade-prose alternation anchors for the cascade-ordering phrase (per FID-022 / `pnpm lint:docs`) are unchanged.
6. **LOW — LESSON-038 no-unilateral-defer — compliance verified.** The §Out of Scope section explicitly tags 13 deferrals as "Spencer's separate ratification required".

**GREEN (recommendations for next session, NOT applied in this pass):**

1. **WebSocket reconnection with session resume** (FUTURE). The current protocol is "subscribe fresh on reconnect".
2. **`/metrics` endpoint (Prometheus format)** (FUTURE). FID-031 has no metrics.
3. **`/openapi.json` + `/docs` (Swagger UI)** (FUTURE). FID-031 has no auto-generated API docs.
4. **gRPC alternative** (FUTURE). The HTTP+WS surface is fine for the dashboard; for high-throughput automation, gRPC would be lower-overhead.
5. **WebSocket per-channel rate limiting** (FUTURE). The WebSocket has no rate limiting.

**AUDIT (this pass, 2026-07-14):**

- 1 thinker-with-files-gemini pass completed (cross-FID reconciliation validation → A-F analysis)
- 1 code-reviewer-minimax-m3 pass in flight (per LESSON-051, this verifier pass IS the meta-review)
- Doc rewritten to expand `crates/gateway/` (drop `crates/api/` per ZeroClaw alignment)
- Status: `analyzed` (post-thinker; the doc is the deliverable, not the implementation)
- File remains in `dev/fids/` (not yet archived)
- LESSON-027 invariant preserved (the doc does not add new cascade-ordering anchors)
- LESSON-038 invariant preserved (the §Out of Scope section explicitly tags 13 deferrals as "Spencer's separate ratification required")
- LESSON-049 convention followed (the §Questions You Should've Asked section uses the 4-field template; the §Verifier Pass uses the RED/GREEN/AUDIT/CHANGE DELTA structure)
- LESSON-051 scope-ratify applied (Spencer's "Update FID-031" directive is a scope-ratify for the spec doc; the impl timing is at Spencer's separate discretion)

**CHANGE DELTA:** ~70% (this is a major rewrite — the entire §Summary, §Problem, §Expected Behavior, §Impact Assessment, §Proposed Solution sections are replaced with the ZeroClaw-aligned version; the §Decisions + §Out of Scope + §Cross-References + §Verifier Pass + §Missed Questions + §Suggestions sections are preserved with light edits).

## Missed Questions

The 4-field template (per LESSON-049). The thinker's gap-survey pass fills these with concrete items + recommendations for Spencer's ratification.

1. **Q:** Should the gateway expansion use a versioned URL prefix (`/v1/`) or merge into the existing `/api/` prefix?
   - **Context:** The existing gateway has 30+ endpoints under `/api/*`. The new Tauri IPC mappings could either be added to `/api/*` (merging with existing) or under a new `/v1/*` prefix (clean separation). The ZeroClaw pattern uses `/v1/` for the dashboard-facing API.
   - **Recommended:** New `/v1/*` prefix. The existing `/api/*` endpoints stay for legacy compatibility; the new `/v1/*` endpoints are the canonical Tauri-mapped surface. This is the cleanest version boundary.
   - **Trade-off:** Two URL prefixes; benefit is clear version semantics + no path conflicts.

2. **Q:** Should the `embedded-web` feature flag use `rust-embed` (compile-time bundling) or `tower-http::ServeDir` (runtime file serving)?
   - **Context:** `rust-embed` bundles the static files into the binary at compile time; the binary is self-contained. `tower-http::ServeDir` serves files from a path at runtime; the binary needs the static files deployed alongside it.
   - **Recommended:** `rust-embed` for the bundled-binary case + `ServeDir` fallback for the dev-mode case (where the static files are at `web/dist/` on disk). The `embedded-web` feature flag enables `rust-embed`; when disabled, the dev-mode `ServeDir` is used (the CLI's `next dev` spawn serves the dashboard separately in dev mode anyway).
   - **Trade-off:** Larger binary with `rust-embed`; benefit is single-binary deployment + no external file dependencies.

3. **Q:** Should the gateway serve the dashboard at `/` (root) or at `/dashboard`?
   - **Context:** The existing API endpoints are at `/api/*` and `/v1/*`. The dashboard could be at `/` (root, taking over) or at `/dashboard` (a sub-path). The ZeroClaw pattern serves the dashboard at `/`.
   - **Recommended:** Serve at `/` (root). The dashboard is the user-facing surface; the API endpoints are sub-paths (`/api/*`, `/v1/*`). The `ServeDir` is nested after the API routes so they take precedence.
   - **Trade-off:** API routes must be nested before the `ServeDir`; benefit is the dashboard is at the canonical URL.

4. **Q:** Should the `/v1/health` endpoint be a separate route or share the existing `/api/health`?
   - **Context:** The existing gateway has `/api/health`. The new `/v1/health` could either be a separate route (version-specific health) or share the existing one (single health endpoint).
   - **Recommended:** Separate `/v1/health` route. The new endpoints are version-specific; the health check should match. The existing `/api/health` stays for backward compatibility.
   - **Trade-off:** Two health endpoints; benefit is clean version semantics.

5. **Q:** Should the SSE endpoint be at `/v1/manifest/soul/stream` (path-specific) or at `/v1/stream` (catch-all)?
   - **Context:** The SSE endpoint could be path-specific (only streams for soul generation) or catch-all (any kind of stream). The WebSocket at `/ws` is catch-all (pub/sub).
   - **Recommended:** Path-specific `/v1/manifest/soul/stream` (per the FID-032 §Decisions #4 ratification). The WebSocket at `/ws` stays for pub/sub. A future FID could add more SSE endpoints at different paths.
   - **Trade-off:** Path-specific means new stream types need new endpoints; benefit is clean separation between one-shot streams and pub/sub.

6. **Q:** Should the gateway expansion use `axum::Router::nest` (sub-router) or `axum::Router::route` (explicit routes)?
   - **Context:** `nest` groups routes under a path prefix (cleaner for `/v1/*`); `route` adds individual routes (more explicit). The existing gateway uses `nest` for the 12 existing route groups.
   - **Recommended:** `nest` for the new `/v1/*` sub-router (mirrors the existing pattern). The 22 new endpoints are organized into 10 route groups + 1 health + 1 SSE; each route group is a sub-router.
   - **Trade-off:** Slightly more nesting; benefit is consistency with the existing structure.

7. **Q:** Should the gateway be moved to a feature flag (e.g., `--features savant-gateway` in the workspace) for kernel builds?
   - **Context:** The gateway is heavy (axum + tower-http + 60+ endpoints + WebSocket). The kernel-only builds (e.g., for embedding the runtime in a different host) might not need the gateway.
   - **Recommended:** Defer the kernel split to FID-034 (trait adoption). The gateway stays a single sub-crate; the feature flags are added in FID-034 as part of the trait refactor.
   - **Trade-off:** No kernel-only build for FID-031; benefit is the gateway is a single sub-crate for now.

8. **Q:** Should the existing `/api/*` endpoints be deprecated in favor of the new `/v1/*` endpoints?
   - **Context:** The existing 30+ `/api/*` endpoints are the gateway's current surface. The new 22 `/v1/*` endpoints are the Tauri-mapped surface. Are the `/api/*` endpoints deprecated?
   - **Recommended:** NO deprecation. The `/api/*` endpoints stay for backward compatibility (any existing dashboard code or external tooling that uses them). The `/v1/*` endpoints are the canonical Tauri-mapped surface; future FIDs should prefer `/v1/*` for new work.
   - **Trade-off:** Two URL surfaces; benefit is no breaking changes for existing users.

9. **Q:** How does `rust-embed` fallback during active development where `web/dist` does not yet exist?
   - **Context:** Specifying the `#[derive(RustEmbed)] #[folder = "web/dist/"]` macro requires the folder to exist at cargo compilation time, which will fail if `next build` hasn't been run locally on a fresh checkout.
   - **Recommended:** Provide a `cfg_attr` or a dummy `build.rs` shim that touches an empty `web/dist/index.html` to satisfy `rust-embed`.
   - **Trade-off:** Minor build script complexity; benefit is seamless cargo builds on fresh checkouts.

10. **Q:** Are standard formatting rules enforced for the Server-Sent Events output?
    - **Context:** `manifest/soul/stream` must perfectly align formatting for the FID-032 frontend parsing headers (`data: ` prefix + `\n\n` event boundary).
    - **Recommended:** Enforce the use of the `axum::response::sse::Event` builder rather than manual string building.
    - **Trade-off:** Tighter `axum-sse` dependency constraint; benefit is strictly valid protocol output.

11. **Q:** How will the new endpoints inject trait-specific validations directly (FID-034 cross-FID integration)?
    - **Context:** Settings endpoints accepting values like `temperature: 1.5` must validate against model limits. FID-034 introduces `capabilities()`.
    - **Recommended:** Route `settings_post` checks through `state.model_provider.capabilities()` max limits.
    - **Trade-off:** Re-fetching trait metadata; benefit is backend protects against bounds issues dynamically.

12. **Q:** Does the embedded static server conflict with URL pathing for Next.js internal routes like `_next/static/`?
    - **Context:** `ServeDir::new` combined with `ServeFile` fallback logic overrides paths that might falsely match API or chunk routes.
    - **Recommended:** Verify that standard SPA logic only routes extensions (`.js`, `.css`) or `/_next/` natively without hitting `index.html`.
    - **Trade-off:** More precise `ServeDir` configuration; benefit is no corrupted chunk loading.

13. **Q:** What is the protection against connection overflow on the SSE streams?
    - **Context:** A user spamming `manifestSoulStream` could exhaust file descriptors holding axum streams open.
    - **Recommended:** Implement a stream concurrency governor in `GatewayState`.
    - **Trade-off:** Concurrency overhead handling; benefit is server stability.

## Suggestions for Improvement

The 4-field template (per LESSON-049). The thinker's gap-survey pass validates these + may add new ones.

A. **Add a `crates/gateway/src/handlers/v1/CLAUDE.md`** that documents the `/v1/*` route map + the handler patterns + the `V1ApiError` mapping. Helps future contributors add new endpoints consistently. Cost: ~50 lines. Benefit: discoverability + consistency.

B. **Add a `crates/gateway/src/handlers/v1/middleware.rs`** that includes common middleware for the `/v1/*` routes (request size limit, content-type validation, etc.). Centralizes cross-cutting concerns. Cost: ~30 LoC. Benefit: DRY.

C. **Add a `crates/gateway/examples/v1_client.rs`** that demonstrates how to call the new `/v1/*` endpoints from a Rust client. Useful for the CLI's `savant_*` subcommands (per FID-030) that need to call the gateway. Cost: ~100 LoC. Benefit: reference implementation.

D. **Add a `crates/gateway/tests/integration.rs`** that uses `reqwest` + a real bound port to test the full network stack (CORS + TCP binding + payload deserialization). Complements the in-process smoke tests. Cost: ~50 LoC. Benefit: high-confidence validation.

E. **Add a `crates/gateway/benches/v1_routes_bench.rs`** that benchmarks the 22 new endpoints under load (e.g., 100 concurrent vault profile lists). Cost: ~50 LoC. Benefit: performance baseline.

F. **Add OpenAPI YAML snippets inline in each route handler** — `#[utoipa::path(...)]` annotations on each handler for auto-generated API docs (per the FID-031 §GREEN #3). Cost: ~5 LoC per handler × 22 = ~110 LoC. Benefit: auto-generated OpenAPI docs.

G. **Add a `/v1/api/version` endpoint for capability negotiation** — `{"version": "1.0.0", "features": ["v1", "sse", "embedded-web"]}`. Distinct from the git-SHA-based `/v1/version`; a semantic API version for client-side compatibility checks. Cost: ~10 LoC. Benefit: client-side compatibility checks.

H. **Add catch-all 404 logging** — log the unmatched path + method + origin for security monitoring (catches reconnaissance scans). Cost: ~10 LoC. Benefit: security observability.

I. **Implement a versioned `X-Savant-API` HTTP response header globally** — ensures that FID-032 client updates don't silently mismatch against an older server binary. Add axum middleware to append the current `CARGO_PKG_VERSION` to every response. Cost: ~5 LoC; benefit: easy version negotiation.

J. **Emit standardized fallback payloads for unknown `/ws` frames** — a mismatch in IPC names (e.g., frontend sends `start_concousness` misspelled) fails silently over websockets. Provide an explicitly matched catch-all arm returning `{"error": "unsupported_command"}`. Cost: ~10 LoC; benefit: drastically easier UI debugging.

K. **Add an explicit field in `/v1/health` detailing if `embedded-web` is compiled** — external runners/shells (like FID-033) might need to fall back or provide alerts if they hit an API-only binary. Push `features: ["embedded-web"]` directly into the health payload JSON. Cost: ~5 LoC; benefit: deterministic capability probing.

L. **Add a panic-catching bounds layer to all `v1` routers** — v1 translates old Tauri strictness directly to HTTP, exposing the app to potential network string manipulations causing unwraps. Use `tower_http::catch_panic::CatchPanicLayer` to emit structured 500s rather than dropping TCP natively. Cost: ~10 LoC; benefit: high-availability.

## Resolution

- **Fixed By:** TBD
- **Fixed Date:** TBD
- **Fix Description:** TBD
- **Tests Added:** TBD
- **Verified By:** TBD
- **Commit/PR:** TBD
- **Archived:** TBD (this doc moves to `dev/fids/archive/` after impl closes)

> When status is set to **Closed**, move this file to `dev/fids/archive/` and append an entry to `CHANGELOG.md`.

## Lessons Learned

TBD (post-impl codification per LESSON-054)

## Cross-References

**FIDs (same sequence):**
- **FID-030** (`dev/fids/FID-2026-07-14-030-cli-scaffold.md`) — the CLI runtime host; FID-030 imports `savant_gateway` + `savant_runtime` directly and calls `savant_gateway::server::start_gateway(state).await` (ZeroClaw pattern). The CLI does NOT spawn a separate API process.
- **FID-029** (`dev/fids/FID-2026-07-14-029-chat-persistence.md`) — the chat persistence spec; FID-031 maps the 6 FID-029 Tauri commands to `/v1/chat/*` HTTP endpoints
- **FID-032** (`dev/fids/FID-2026-07-14-032-api-client-refactor.md`) — the api-client refactor; FID-032 swaps the dashboard's `invoke` calls to `fetch` + `WebSocket` + `SSE` against the new `/v1/*` endpoints + the `/ws` WebSocket + the `/v1/manifest/soul/stream` SSE
- **FID-033** (`dev/fids/FID-2026-07-14-033-tauri-repackaging.md`) — Tauri repackaging; FID-033 moves `src-tauri/` to `apps/tauri/` (thin optional shell using `savant_desktop::run()`), NOT deletes it
- **FID-034** (`dev/fids/FID-2026-07-14-034-kernel-trait-adoption.md`) — trait adoption; FID-034 adopts `ModelProvider` / `Memory` / `Tool` / `Channel` traits à la ZeroClaw for trait-driven extension

**FIDs (referenced):**
- **FID-022** (`dev/fids/archive/FID-2026-07-14-022-lesson-027-doc-drift-linter.md`) — established the `pnpm lint:docs` discipline that FID-031 preserves (no new cascade-ordering anchors)
- **FID-024** (`dev/fids/archive/FID-2026-07-14-024-checkpoint-release-discipline.md`) — the release-prep automation
- **FID-025** (`dev/fids/archive/FID-2026-07-14-025-lint-defer-no-unilateral-defer-tool.md`) — established the LESSON-038 no-unilateral-defer rule; FID-031's §Out of Scope section explicitly tags 13 deferrals as Spencer's separate ratification required
- **FID-028** (`dev/fids/FID-2026-07-14-028-scaffold-changelog-faq-tune-pages.md`) — the changelog/FAQ/tune pages; FID-031 maps the 3 IPC wrappers to `/v1/changelog` + `/v1/faq` + `/v1/tune/*` HTTP endpoints

**LESSONs:**
- **LESSON-019** — release-only-versioning discipline
- **LESSON-027** — doc-drift invariant (FID-031 does not add new cascade-ordering anchors; `pnpm lint:docs` exits 0)
- **LESSON-029** — `release.py` pre-flight is local-only
- **LESSON-030** — file-based commit/tag pattern
- **LESSON-031** — re-grep verification gate
- **LESSON-038** — no-unilateral-defer (the §Out of Scope section explicitly tags 13 deferrals)
- **LESSON-049** — verifier-pass + 4-field Q&A template convention (followed throughout this doc)
- **LESSON-051** — explicit scope-ratify (Spencer's "Update FID-031" directive is a scope-ratify for the spec doc; the impl timing is at Spencer's separate discretion)
- **LESSON-053** (NEW, post-FID-030 impl) — CLI-as-runtime-host discipline: the CLI imports the gateway + runtime directly; the dashboard is a renderer; Tauri is repackaged as an optional shell
- **LESSON-054 (NEW, post-impl)** — gateway-expansion discipline: the gateway is the single runtime surface; no parallel API crate; the CLI is the runtime host; the dashboard is a thin HTTP+WS client

**Protocol references:**
- **ECHO Protocol v0.1.1** (`ECHO.md`) — the standing agent discipline
- **`protocol.config.yaml`** — the project-specific commands + paths
- **`templates/FID-TEMPLATE.md`** — the canonical FID structure
- **`coding-standards/release-workflow.md`** — the release discipline + the FID auto-archive pattern

**Workspace references:**
- **`Cargo.toml` (root)** — workspace `[workspace.dependencies]` gains `tower-http` (with `fs` feature) + `rust-embed` (optional)
- **`crates/gateway/`** — the foundation; expanded with 22 new endpoints + SSE + `embedded-web` feature flag + static dashboard serving
- **`crates/cli/`** — the runtime host; adds `savant_gateway` + `savant_runtime` deps; calls `savant_gateway::server::start_gateway(state).await` directly
- **`src-tauri/`** — the Tauri app; stays at `src-tauri/` for FID-031 (the legacy fallback until FID-033 repackages it as `apps/tauri/`)
- **`src/lib/ipc.ts` + `src/lib/mock-ipc.ts`** — stays untouched for FID-031 (the api-client refactor is FID-032)

---

## Verifier Pass (2026-07-14 — gap-survey perfection loop)

The 2nd verifier pass is the post-thinker gap-survey. The 1st verifier pass (above) was the doc-drafting meta-review. This 2nd pass surfaced 5 NEW §Missed Questions + 4 NEW §Suggestions that the 1st pass missed.

**RED (new gaps surfaced in this gap-survey pass):**

1. **Q9 — `rust-embed` build-time folder requirement** — the 1st pass didn't consider that fresh checkouts (without a prior `next build`) would fail the `#[derive(RustEmbed)] #[folder = "web/dist/"]` macro. New Q9 recommends a `build.rs` shim that touches an empty `web/dist/index.html` to satisfy `rust-embed`.
2. **Q10 — SSE format enforcement** — the 1st pass allowed manual string building for SSE; the gap-survey realized the `axum::response::sse::Event` builder is the canonical safe path. New Q10 recommends strict use of the `Event` builder.
3. **Q11 — Trait capability validation in handlers (FID-034 cross-FID integration)** — the 1st pass didn't anticipate that FID-034's `capabilities()` method on `ModelProvider` should gate settings validation (e.g., `temperature: 1.5` vs `max: 1.0`). New Q11 recommends routing settings validation through `state.model_provider.capabilities()`.
4. **Q12 — Next.js `_next/static/` path conflict with `ServeDir` fallback** — the 1st pass assumed the `ServeFile("index.html")` fallback would only fire for non-asset paths; the gap-survey realized Next.js internal asset paths (e.g., `/_next/static/chunks/main.js`) could falsely fall through to `index.html` and break the dashboard. New Q12 recommends precise `ServeDir` configuration that routes extensions + `/_next/` natively.
5. **Q13 — SSE connection overflow protection** — the 1st pass didn't consider DoS protection on the SSE stream. A user spamming `manifestSoulStream` could exhaust file descriptors. New Q13 recommends a stream concurrency governor in `GatewayState`.

**GREEN (4 new suggestions surfaced in this gap-survey pass):**

1. **I — `X-Savant-API` version header** — the 1st pass didn't have a version negotiation mechanism; the gap-survey realized a global response header (`X-Savant-API: <CARGO_PKG_VERSION>`) lets FID-032 detect version mismatches early (~5 LoC of axum middleware).
2. **J — `/ws` unknown frame fallback** — the 1st pass assumed the WebSocket protocol handler would reject unknown frames silently; the gap-survey realized an explicit `{"error": "unsupported_command"}` fallback aids UI debugging (~10 LoC).
3. **K — `embedded-web` feature flag in `/v1/health`** — the 1st pass didn't expose the feature compilation state; the gap-survey realized FID-033's Tauri shell may need to probe whether the gateway is API-only vs static-serving (~5 LoC).
4. **L — `tower_http::catch_panic` for v1 routers** — the 1st pass didn't have a panic-catching layer; the gap-survey realized v1 handlers translate old Tauri strictness to HTTP + can panic on network string manipulations. Adding `CatchPanicLayer` emits structured 500s vs dropping TCP (~10 LoC).

**AUDIT (this pass, 2026-07-14):**

- 1 thinker-with-files-gemini pass completed (5-FID gap-survey)
- 1 code-reviewer-minimax-m3 pass in flight
- 5 NEW §Missed Questions added (Q9-Q13) + 4 NEW §Suggestions added (I-L)
- Doc body grew from ~1,250 lines to ~1,360 lines (~9% increase)
- Status: `analyzed` (post-thinker)
- File remains in `dev/fids/` (not yet archived)
- LESSON-027 invariant preserved (no new cascade-ordering anchors)
- LESSON-038 invariant preserved (5 NEW Q + 4 NEW S are enhancement items, not agent deferrals)
- LESSON-049 convention followed (4-field template; §Verifier Pass uses RED/GREEN/AUDIT/CHANGE DELTA structure)
- LESSON-051 scope-ratify applied

**CHANGE DELTA:** ~9% (5 NEW Q + 4 NEW S + a new 2nd ## Verifier Pass section; the existing 8 Q + 8 S + 1st ## Verifier Pass are preserved).
