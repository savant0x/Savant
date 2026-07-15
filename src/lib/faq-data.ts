// FID-028 — Curated FAQ data. No real FAQ module exists in the
// savant-orig (verified via `find lib/cortexadb -name 'faq*' -o -name
// 'help*'` returning no matches; `grep -rn 'FAQ' lib/cortexadb/`
// returning no FAQ module). The 6 Q&A items below are grounded in
// the project's own artifacts (CHANGELOG.md, README.md,
// dev/LEARNINGS.md, the 22-crate Rust workspace structure, the FID
// lifecycle) — not invented FAQ engine content, per Spencer's "view
// the source" directive.
//
// Future FID candidate (NOT in this FID's scope, per LESSON-038):
// add a real FAQ module to the gateway (e.g., `GET /api/faq?topic=...`
// backed by `config/faq.json` or a markdown directory) + a new
// `get_faq_topic()` Tauri command. This is a separate work-item
// requiring Spencer's separate approval.

export type FaqItem = {
  /** The question (1 sentence, displayed as the accordion header). */
  question: string;
  /** The answer (1-3 sentences, displayed when the accordion expands). */
  answer: string;
};

export const FAQ_ITEMS: FaqItem[] = [
  {
    question: "What is Savant?",
    answer:
      "Savant is a proactive AI assistant with a Next.js 15 + React 19 dashboard (the renderer) over a Rust + Tauri 2 desktop host (the engine). It runs as a Tauri app for desktop users; the Next.js renderer also works in a browser preview for development.",
  },
  {
    question:
      "Why is the master key separate from the derived subkey?",
    answer:
      "The master key is vault-only (never reaches HTTP traffic); the renderer provisions a scoped derived subkey via OpenRouter's POST /v1/keys, uses that for chat outbound Authorization, and the master stays local. This is the OpenRouterMgmt::create_key two-tier architecture from savant-orig — eliminates the single-tier collapse where a leaked browser-localStorage master key would be unrecoverable.",
  },
  {
    question:
      "What's the difference between the renderer and the gateway?",
    answer:
      "The renderer (src/) is the Next.js + React UI. The gateway (crates/gateway/) is the Axum HTTP server that bridges to the Rust agent core. The renderer talks to the engine through the Tauri host (src-tauri/) IPC bridge — the Tauri host is the thin layer that routes IPC calls to either the Tauri runtime or (in browser preview) the mock IPC. The gateway is a separate process; the renderer does not talk to it directly in the current design.",
  },
  {
    question: "How does the Tauri IPC bridge work?",
    answer:
      "The renderer calls `invoke('command_name', { arg1, arg2 })` from `@tauri-apps/api/core`. In the Tauri desktop runtime, the call is dispatched to the matching `#[tauri::command]` in `src-tauri/src/lib.rs`. In browser preview, `setupMockIPC()` installs `@tauri-apps/api/mocks`' `mockIPC()` which intercepts and returns realistic data. The renderer code is identical in both modes — see `src/lib/ipc.ts` for the wrappers and `src/lib/mock-ipc.ts` for the mock handlers.",
  },
  {
    question: "What are the three credential tiers?",
    answer:
      "Tier 1 is the env var (`process.env.OPENROUTER_MASTER_KEY` or `.env` file) — shadows everything. Tier 2 is the vault file (set via Settings → OpenRouter Master Key, stored encrypted with DPAPI on Windows). Tier 3 is the derived subkey (OpenRouter POST /v1/keys, scoped to a savant-{randomHex} agent_name, auto-rotated every 24h). The master is vault-only; only the derived subkey reaches HTTP traffic.",
  },
  {
    question: "How do I update Savant to the latest version?",
    answer:
      "Savant ships in checkpoint releases via the v0.0.X versioning scheme (10 patch releases per minor; the rules live in coding-standards/release-workflow.md). At a release cut, the user runs `pnpm release:prep 0.0.X` which orchestrates the FID archive + version bump + README refresh + bloat cleanup + verification gates. Between releases, work is local-only — the project follows the build-freely + push-at-release discipline (LESSON-019 + coding-standards/release-workflow.md §Checkpoint Release Discipline).",
  },
  {
    question: "What is a FID and how does the lifecycle work?",
    answer:
      "A FID (Finding / Improvement Document) is the project's per-feature-or-fix work tracker, modeled on ECHO Protocol's FID template. Statuses flow: created → analyzed → fixed → verified → closed. On closed, the file is moved to dev/fids/archive/ and an entry is appended to CHANGELOG.md. The Perfection Loop (RED → GREEN → AUDIT on code changes) is the impl-iteration surface; the Verifier Pass (LESSON-049) is the separate meta-review surface. See templates/FID-TEMPLATE.md for the canonical shape.",
  },
  {
    question:
      "What is the inner monologue / reflections feature?",
    answer:
      "FID-017 introduced the Reflections Viewer — a 19-lens cognitive rotation (CRITIQUE, EMERGENCE, IDENTITY, AUTONOMY, RELATIONAL, etc.) that triggers an LLM call on a 5-second daemon cycle, appends the narrative to workspace-savant/REFLECTIONS.md, and displays the journal stream on the /reflections page. All lenses are merged into a single continuous stream (not partitioned by lens) per Spencer's 2026-07-13 directive.",
  },
];

/** Returns the curated FAQ items. */
export function getFaqItems(): FaqItem[] {
  return FAQ_ITEMS;
}
