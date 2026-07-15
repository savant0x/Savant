// FID-028 + Spencer correction 2026-07-14 — Changelog source is
// GitHub, not the local file.
//
// Per Spencer: "the changelog needs to come from github because
// that's the source of truth for the changelog, other people will
// be downloading this and will not have the changlog locally, the
// project on gh does."
//
// The renderer fetches the changelog from the canonical GitHub
// raw URL at runtime. The local `CHANGELOG.md` file (read by the
// `?raw` import in the prior version of this file) is for the
// developer's reference; it is NOT bundled with the shipped app.
//
// Repo: `savant0x/Savant` — derived from:
// - `package.json` `repository` field (canonical)
// - `scripts/release.py:33` `REPO_SLUG` default
// - `scripts/release.py:211` `User-Agent` (post v0.0.5 rename)
// - `MIGRATION.md:11` `git clone` example URL
// - `CHANGELOG.md:54` FID-006 v3 HTTP-Referer header correction
//
// Branch: `main` (matches the local checkout's default branch
// per `git status`; verified in v0.0.5 release commit `592da64`).
//
// Fallback policy: NO local fallback. If GitHub is unreachable
// (offline, rate limit, 404), the page surfaces a clear error
// with a "Retry" button. The local file is NOT a fallback by
// design — it would mask the case where the GitHub copy is newer
// than the local file (which is the whole point of the correction).
//
// CORS: `raw.githubusercontent.com` serves with permissive
// `Access-Control-Allow-Origin: *` (verified). No proxy needed
// for browser preview or Tauri webview runtime.

// Canonical GitHub raw URL for the changelog. Exported as a
// shared constant so the URL appears in exactly one place — the
// changelog page's source label + any future consumers (CLI tools,
// release scripts, etc.) can import this constant instead of
// hardcoding the URL. If the repo moves or the branch renames, this
// is the only line that changes.
export const CHANGELOG_GITHUB_RAW_URL =
  "https://raw.githubusercontent.com/savant0x/Savant/main/CHANGELOG.md";

/** Human-readable source label for UI display (e.g., the changelog
 *  page's header). Shorter than the raw URL. */
export const CHANGELOG_SOURCE_LABEL = "github.com/savant0x/Savant/CHANGELOG.md";

/**
 * Fetches the changelog markdown from the canonical GitHub URL.
 * Returns the raw markdown text. Throws on network error / non-2xx
 * status (the caller is responsible for rendering the error state).
 *
 * The `cache: "no-store"` directive ensures each visit gets the
 * latest version — the changelog is a small file (~30KB) and
 * freshness matters more than cache hit rate.
 */
export async function fetchChangelog(): Promise<string> {
  const response = await fetch(CHANGELOG_GITHUB_RAW_URL, {
    cache: "no-store",
    headers: { Accept: "text/plain, */*;q=0.5" },
  });
  if (!response.ok) {
    throw new Error(
      `Failed to fetch changelog from GitHub (${response.status} ${response.statusText}). ` +
        `The canonical source is at ${CHANGELOG_GITHUB_RAW_URL}.`,
    );
  }
  return response.text();
}
