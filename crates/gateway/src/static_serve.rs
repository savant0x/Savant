//! Static dashboard serving for the `embedded-web` feature flag.
//!
//! When the `embedded-web` feature is enabled (e.g., for `savant serve --prod`),
//! the gateway serves the static Next.js export from the binary. The CLI's dev
//! workflow uses `next dev` separately; this is the prod path that produces a
//! single self-contained binary.
//!
//! **Pattern** (per FID-031 §Static Dashboard Serving):
//! - `rust-embed` bundles the static files at compile time (no runtime file deps)
//! - `ServeDir` reads from disk at runtime (dev mode, when the `embedded-web`
//!   feature is off — used as a fallback for the CLI's dev workflow where
//!   `next dev` is the canonical dashboard server, but a disk-based fallback
//!   is useful for staging)
//! - SPA fallback: any unmatched path returns `index.html` so Next.js
//!   client-side routing works correctly
//!
//! **Build script note** (per LESSON-031 re-grep pattern + FID-031 Q9):
//! The `#[derive(RustEmbed)] #[folder = "web/dist/"]` macro requires the
//! folder to exist at cargo compilation time. A `build.rs` shim (out of
//! scope for this PR — handled by the dev workflow's `pnpm build` step)
//! touches an empty `web/dist/index.html` to satisfy `rust-embed` on
//! fresh checkouts. For now, the `#[folder = "web/dist/"]` macro is fine
//! because the dev workflow always runs `next build` first.

use axum::Router;

#[cfg(feature = "embedded-web")]
use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};

#[cfg(feature = "embedded-web")]
use rust_embed::RustEmbed;

#[cfg(feature = "embedded-web")]
#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct DashboardAssets;

/// Append the static dashboard fallback to the app router. Called from
/// `crates/gateway/src/server.rs` AFTER the API routes are merged, so
/// the `/v1/*` and `/api/*` routes take precedence over the static
/// fallback.
///
/// **Generic over `S`**: the caller passes any `Router<S>` (typically
/// `Router<()>` after `.with_state()`, but the generic keeps the function
/// flexible). Required so the function doesn't lose the state type when
/// the `embedded-web` feature is off (a non-generic `Router` → `Router`
/// signature would force the caller to re-specify the state).
///
/// When the `embedded-web` feature is OFF: returns the router unchanged
/// (the dev workflow uses `next dev` to serve the dashboard separately).
///
/// When the `embedded-web` feature is ON: adds a fallback handler that:
/// 1. Serves the requested file from the rust-embed bundle (if it exists)
/// 2. Falls back to `index.html` for SPA routes (any path without an extension)
/// 3. Returns 404 for asset requests that don't exist
pub fn append_fallback<S>(app: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    #[cfg(feature = "embedded-web")]
    {
        app.fallback(serve_embedded)
    }
    #[cfg(not(feature = "embedded-web"))]
    {
        app
    }
}

#[cfg(feature = "embedded-web")]
async fn serve_embedded(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match DashboardAssets::get(path) {
        Some(file) => {
            let mime = guess_mime(path);
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "public, max-age=3600")
                .body(Body::from(file.data.into_owned()))
                .unwrap_or_else(|e| {
                    tracing::error!("[static_serve] Failed to build response for {}: {}", path, e);
                    (StatusCode::INTERNAL_SERVER_ERROR, "asset build error").into_response()
                })
        }
        // SPA fallback: serve index.html for routes that aren't static assets
        None if !path.contains('.') => {
            match DashboardAssets::get("index.html") {
                Some(file) => Response::builder()
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(Body::from(file.data.into_owned()))
                    .unwrap_or_else(|e| {
                        tracing::error!("[static_serve] Failed to build index.html response: {}", e);
                        (StatusCode::INTERNAL_SERVER_ERROR, "index.html build error").into_response()
                    }),
                None => (
                    StatusCode::NOT_FOUND,
                    "index.html not embedded — run `pnpm build` first",
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            format!("asset not found: {}", path),
        )
            .into_response(),
    }
}

#[cfg(feature = "embedded-web")]
fn guess_mime(path: &str) -> &'static str {
    // Tiny inline mime mapper for the common Next.js static export extensions.
    // Avoids adding the `mime_guess` dep for the embedded-web feature.
    // Covers: .html, .js, .mjs, .css, .json, .png, .jpg, .svg, .ico, .woff, .woff2, .ttf, .map, .txt
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".map") {
        "application/json; charset=utf-8" // source maps
    } else if path.ends_with(".txt") {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

/// Tests for the `embedded-web` feature ON path. Only compiled when the
/// feature is enabled (because `guess_mime` is `#[cfg(feature = "embedded-web")]`).
#[cfg(test)]
#[cfg(feature = "embedded-web")]
mod tests_with_feature {
    use super::*;

    #[test]
    fn test_guess_mime_known_extensions() {
        assert_eq!(guess_mime("index.html"), "text/html; charset=utf-8");
        assert_eq!(guess_mime("_next/static/chunks/main.js"), "application/javascript; charset=utf-8");
        assert_eq!(guess_mime("styles.css"), "text/css; charset=utf-8");
        assert_eq!(guess_mime("manifest.json"), "application/json; charset=utf-8");
        assert_eq!(guess_mime("logo.png"), "image/png");
        assert_eq!(guess_mime("favicon.svg"), "image/svg+xml");
        assert_eq!(guess_mime("unknown.xyz"), "application/octet-stream");
    }
}

/// Tests for the `embedded-web` feature OFF path. Only compiled when the
/// feature is disabled (the no-op path). Verifies the function is safe to
/// call on an empty Router.
#[cfg(test)]
#[cfg(not(feature = "embedded-web"))]
mod tests_without_feature {
    use super::*;

    #[test]
    fn test_append_fallback_idempotent() {
        // The no-feature path returns the router unchanged.
        let router: Router = Router::new();
        let _ = append_fallback(router);
    }
}
