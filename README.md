<!-- markdownlint-disable MD033 -->
<div align="center">

<img src="img/savant.png" alt="Savant Logo" width="180" />

**Savant — Sovereign AI Substrate.**

A CLI-launched AI shell built on Next.js 15 + Rust. Phase 1 ships a renderer-first dashboard with a 22-crate Rust workspace restored on disk; the cognitive core begins Phase 2+ wiring via the gateway IPC.

<!-- BADGES_START -->
<div align="center">

[![React](https://img.shields.io/badge/React-19-%23000000?style=flat-square&logo=react&logoColor=%2300fbff)](https://react.dev/)[![Next.js](https://img.shields.io/badge/Next.js-15-%23000000?style=flat-square&logo=nextdotjs&logoColor=%2300fbff)](https://nextjs.org/)[![Rust](https://img.shields.io/badge/Rust-1.86+-%23000000?style=flat-square&logo=rust&logoColor=%2300fbff)](https://www.rust-lang.org/)[![TypeScript](https://img.shields.io/badge/TypeScript-5.7-%23000000?style=flat-square&logo=typescript&logoColor=%2300fbff)](https://www.typescriptlang.org/)[![HeroUI](https://img.shields.io/badge/HeroUI-v3_Alpha-%23000000?style=flat-square&logo=react&logoColor=%2300fbff)](heroui.com/)[![License](https://img.shields.io/badge/License-Apache_2.0-%23000000?style=flat-square&logo=github&logoColor=%2300fbff)](LICENSE)[![Status](https://img.shields.io/badge/Status-v0.0.6_Released-%23000000?style=flat-square&color=brightgreen)](CHANGELOG.md)

</div>
<!-- BADGES_END -->

</div>

---

<!-- WHATS_NEW_START -->
## What's New in v0.0.6

- **`pnpm lint:docs`** [`scripts/lint-docs.sh`]: enforces the LESSON-027
  substring-match drift invariant (5 anchors across 4 source files
  for the cascade-ordering phrase; 1 cascade-prose alternation
  variant in the canonical `crates/vault/src/master_key.rs` only).
  Wired as `pnpm lint:docs` (standalone) + part of `pnpm lint:ci`
  (chained with `pnpm lint:markdown`). Implementation: `git grep -c
  '<canonical phrase>'` (expect 5) + `git grep -ciE '<cascade-prose
  alternation>'` (expect 1); exits 1 if either count is wrong. See
  [`coding-standards/doc-drift-lint.md`] for the invariant description.
- **`pnpm release:check`** [`scripts/release-check.sh`]: LESSON-029
  3-gate pre-flight companion to `scripts/release.py`. Gate 1 = clean
  working tree (`git status --porcelain | wc -l` == 0). Gate 2 = no
  transient temp files (LESSON-030 cleanup discipline; dectects
  `.tmp-*` + `*.bak` patterns). Gate 3 = remote tag presence
  (advisory; warns if missing). Wired as `pnpm release:check`. Exit
  codes match the failing gate (1/2/3/4).
- **`pnpm git:commit` + `pnpm git:tag`** [`scripts/commit-with-message.sh`,
  `scripts/tag-with-message.sh`]: LESSON-030 file-based wrappers for
  `git commit -F <msg-file>` + `git tag -a <tag> -F <msg-file>`. The
  file-based pattern (`write_file` + `git commit -F` + `git tag -F`)
  avoids the multi-`-m` shell-escape brittleness for messages
  containing backticks, em-dashes, multi-byte UTF-8. Includes
  pre-flight checks: msg-file exists + non-empty + conventional-commits
  subject for commits; tag name matches `v<semver>` + doesn't already
  exist for tags. Wired as `pnpm git:commit` + `pnpm git:tag`.
- **`pnpm verify:fix`** [`scripts/verify-fix.sh`]: LESSON-031
  dual-check re-grep pattern. Counts occurrences of `--old <pattern>`
  (expect 0; fail if any remaining) + counts occurrences of `--new
  <pattern>` (expect >= 1; fail if absent). Codifies the
  "re-grep after every fix" discipline that closed the v0.0.5
  session-summary 4-site reference bug. Wired as `pnpm verify:fix`.
- **[`coding-standards/doc-drift-lint.md`]** (NEW): canonical
  reference for the LESSON-027 invariant + LESSON-028 field-specific
  anchor discipline + LESSON-031 dual-check pattern. Cross-references
  the 3 LESSON entries in [`dev/LEARNINGS.md`] (Session 2026-07-13-2200
  + Session 2026-07-14-0400).
- **Skills + sandbox IPC surface (FID-025)** — 5 new Tauri commands
  expose `savant_skills` (WASM runtime + Docker executor) and
  `savant_sandbox` (Linux landlock + seccomp + Windows Job Objects)
  to the renderer. Closes the renderer → process-isolation story.
  Renderer can discover skills (`list_skills` / `describe_skill`),
  execute in a sandboxed boundary (`execute_skill` wraps `SkillManager::discover_all_skills`
  + `SandboxDispatcher::create_executor(...).execute(...)` inside
  `savant_sandbox::secure_runtime().isolation_boundary()`), cancel
  mid-flight (`cancel_skill_execution` via `CancellationToken` lookup
  in the `SkillExecutionRegistry`), and observe status
  (`get_skill_status` — `Running` / `Completed` / `Failed` /
  `Cancelled` / `TimedOut`). 5 IPC types in
  [`src-tauri/src/skills/mod.rs`] (`SkillSummary`, `SkillManifest`,
  `ExecutionHandle`, `ExecutionStatus`, `ExecutionState`). The
  `SkillExecutionRegistry` is a `Tauri::State`-managed
  `Arc<tokio::Mutex<HashMap<Uuid, ExecutionRecord>>>` whose
  `ExecutionRecord` holds the live `CancellationToken` — the single
  source of truth for cancellation across the 5 commands. Gap-close
  in [`crates/sandbox/src/lib.rs`] adds 3 thin wrappers (`secure_runtime()`,
  `RuntimeHandle`, `IsolationGuard`) around the existing
  `vmm::select_backend()` so the renderer-side adapter compiles
  against real crate surface. Pure consumer-side wiring per the
  FID-019 vault stabilization precedent (declare-deps →
  adapter-mod → commands → wiring test); no code changes inside
  `savant_skills` or `savant_sandbox` crates. 1 wiring test added at
  [`src-tauri/tests/skill_execution_smoke_test.rs`] (4 sub-tests:
  unknown-id-status errors, unknown-id-cancel errors,
  execute_skill-registers-Running, cancel-flips-state-to-Cancelled) —
  exercises the in-process registry state machine without Docker/WASM
  binaries via `cfg!(test)`-gated happy-path stubs in the adapter.
  New workspace deps in [`src-tauri/Cargo.toml`]: `savant_skills` +
  `savant_sandbox` (both `= { workspace = true }`). Ready for the
  v0.0.6 feature batch release cut.
- **`pnpm lint:defer`** [`scripts/lint-defer.sh`]: enforces the LESSON-038 adjacency invariant — every `deferred` line in any `dev/fids/FID-*.md` must have a permit-context marker within ±3 lines (verbatim Spencer quote OR negation phrasing OR LESSON cross-reference OR historical marker OR compound form OR release-window reference OR FID-specific `Companion tooling` phrase). 30 permit markers across 6 categories. Wired as `pnpm lint:defer` (standalone) + 3rd link of `pnpm lint:ci` (chained with `pnpm lint:markdown` + `pnpm lint:docs`). Implementation: `grep -nE "deferred"` (per-line match) + `sed -n "<line±3>p"` (context window) + `grep -qiE "$PERMIT_REGEX"` (permit-marker match); exits 1 on any violation, 0 otherwise. See [`coding-standards/doc-drift-lint.md`] §LESSON-038 for the policy, remediation paths, and worked example. Post-ratification compliance: 6/6 active FIDs PASS (0 violations); the FID-026 ratification round broadened the original 11-marker PERMIT_REGEX to 30 markers per Spencer's Option A amendment (covering historical/compound/meta patterns + the `deferred to vN.N.N` / `deferred to the vN.N.N` release-window alternation).
- **[`coding-standards/doc-drift-lint.md`]** §LESSON-038 subsection (NEW): canonical reference for the LESSON-038 prohibition rule + adjacency-validation invariant + 3 remediation paths (verbatim Spencer quote, PAUSE-AND-ASK escape hatch, negation phrasing per §Permitted Use 2) + worked example of violating vs permitted lines + compliance remediation notes. The Companion Tooling table at the end of the doc is updated to include `scripts/lint-defer.sh` (LESSON-038 → `pnpm lint:defer`).
- **33 NEW `/v1/*` endpoints (32 REST + 1 SSE) (FID-031)** [`crates/gateway/src/handlers/v1/`]: collapses the Tauri IPC surface into a host-agnostic HTTP+WebSocket layer for the Strangler-Fig Tauri→CLI pivot. 32 REST handlers map directly to existing Tauri IPC commands (changelog / faq / vault / consciousness / inference / manifest / skills / chat / tune / session) + 1 SSE handler (`/v1/stream`) drives long-lived channels. Static dashboard serving via new `embedded-web` Cargo feature flag (rust-embed + SPA fallback — `cargo build --features embedded-web` bundles the Next.js `out/` into the binary; OFF keeps the gateway headless + dashboard loads via `localhost:3000`). 10 NEW in-process smoke tests at [`crates/gateway/tests/v1_routes_smoke_test.rs`] cover health / changelog / faq / stream-format / vault-empty / manifest-empty / session-roundtrip / inference-401 / skills-empty / error-envelope shape. 16 NEW Rust files + `tower_0_5` workspace alias dev-dep (axum-0.8-compatible tower-http-style API surface).

Full release notes in [`CHANGELOG.md`](CHANGELOG.md) `## v0.0.6 — 2026-07-15`.

### Past Releases

For prior release notes (`v0.0.1` through `v0.0.5`), see [`CHANGELOG.md`](CHANGELOG.md) — only the most-recent version's notes live in this README per the single-latest rule codified 2026-07-15.

<!-- WHATS_NEW_END -->

---

<!-- ABOUT_BODY_START -->
## About

Savant is a CLI-launched sovereign AI substrate built on Next.js 15 + Rust. The project follows the **ECHO Protocol** — a Perfection Loop FSM (RED → GREEN → AUDIT → SELF-CORRECT → COMPLETE) over a 15-Law governance layer that codifies mathematical correctness, extreme robustness, and multi-year maintainability as the explicit quality bars.

### Headline Numbers (v0.0.6)

- **22-crate Rust workspace** restored on disk and mechanically verified (`cargo check --workspace` 0/0; `cargo build --workspace` 0/0)
- **33 `/v1/*` endpoints + 1 SSE handler** at [`crates/gateway/`](crates/gateway/) for the gateway IPC substrate (Layer 0 of the strangler-fig sequence)
- **73 vitest unit tests + 15 `privacy.rs` + 4 `forensic_capture.rs` + 2 Playwright round-trips** across the renderer
- **6 open FIDs** (029 / 030 / 032 / 033 / 034 + master-FID-035) scheduled for v0.0.7+ via the strangler-fig Tauri → CLI migration
- **5 LESSONs codified** (027 / 028 / 029 / 030 / 031) for the release-tooling discipline, plus LESSON-053 + 054 for the ECHO-boot + stale-session disposition

### Pathway

- **Phase 1 (current)** — Renderer-first dashboard; cognitive core on disk
- **Phase 2** — Trigger bus + hybrid tick + SQLite WAL + dual-loop init + Rust module split (`trigger/` + `state/` + `cognitive/`)
- **Phase 3** — Tiered inference (fast + slow reflection) + observability + 16-provider chain
- **Phase 4** — Mandatory Security Scanner + Two-Tier Agent System + Distributed Memory Substrate + Channels
- **Phase 5** — Full UI shell (multi-pane dashboard + agent observability) + MCP + Windows DPAPI + release signing
<!-- ABOUT_BODY_END -->

---

## Current State & Live Features

v0.0.6 ships the post-cascade-recovery pre-pivot baseline. Renderer is the live surface; the Rust core is on disk and mechanically verified but awaits Phase 2 wiring. The Tauri 2 desktop shell remains available as a legacy fallback (Option B) while the strangler-fig migration to a CLI-launched Next.js dashboard proceeds in v0.0.7+.

- **Tauri 2 Desktop Shell (Option B, legacy).** Rust shell providing OS-level persistent vault + secure OpenRouter inference client + the FID-017 reflections IPC. `cargo tauri dev` launches a native window with real Rust IPC; the mock IPC self-disables when `window.__TAURI_INTERNALS__` is set. Cross-platform vault at `%APPDATA%/savant/auth.json` (Windows) / `~/.config/savant/auth.json` (Unix).
- **Mock-IPC Browser Preview (Option A, default).** Iterate at Phase 1 velocity at `http://localhost:3000` via [`@tauri-apps/api/mocks`](https://www.npmjs.com/package/@tauri-apps/api) (installed in [`src/lib/mock-ipc.ts`](src/lib/mock-ipc.ts)). Master-key vault + Soul Builder + Reflections Viewer all work end-to-end without a Tauri host. Fast visual iteration; no Rust rebuild loop.
- **Two-Tier Credential Architecture.** Master key in OS app-data vault (5-strategy cascade: env vars → cwd `.env` → exe `.env` → JSON vault → UI prompt). `/v1/keys` provision derives a session subkey the chat outbound traffic uses. The master never reaches HTTP. Auto-rotation ≥24h. Cross-tab `localStorage` sync via `storage` event listener.
- **OpenRouter Inference.** reqwest-based chat-completions client. Surfaces 401 with active key source (`env` / `vault` / `none`) + key length so the user can identify which tier is the problem.
- **Soul Manifestation Engine.** LLM-powered builder ([`src/lib/manifest-mock.ts`](src/lib/manifest-mock.ts), [`src/lib/soul-generation-system-prompt.ts`](src/lib/soul-generation-system-prompt.ts)). System prompt leads with a `CRITICAL DIRECTIVE: PROMPT-DRIVEN IDENTITY` section that bans generic AAA/foundation/sovereign/WAL/CCT language and forces every section to be UNIQUE to the prompt's domain. 18-section AAA Master Framework template.
- **Reflections Viewer.** [`src/app/reflections/page.tsx`](src/app/reflections/page.tsx). Boot-time reader for `workspace-savant/REFLECTIONS.md` + `workspace-savant/SOUL.md` (Tauri-runtime SOUL.md fetch guarded by `if ("__TAURI_INTERNALS__" in window)`). Journal-style parser `split(/(^|\n)##\s+/)`. Streaming indicator + char count + Cancel button. `react-markdown@^10.1.0` + `remark-gfm@^4.0.1` for full CommonMark + GFM.
- **SSE Streaming.** OpenRouter chunked responses yield `preamble` / `chunk` / `complete` / `error` events. rAF-throttled state updates (50–200 chunks/sec) accumulate in a `useRef` and flush via `requestAnimationFrame`. AbortSignal-aware: the Cancel button stops the in-flight fetch + SSE parser cleanly.
- **ECHO Protocol Runtime.** 15 Laws + Perfection Loop FSM (RED → GREEN → AUDIT → SELF-CORRECT → COMPLETE) + FID lifecycle (Created → Analyzed → Fixed → Verified → Closed → Archived). All agent work obeys the ECHO discipline; see [`ECHO.md`](ECHO.md).
- **TypeScript Strict Mode.** `strict: true` required in [`tsconfig.json`](tsconfig.json). Named exports only, no defaults. `unknown` over `any`. 73 vitest unit tests across 7 test files in `src/lib/*.test.ts` + 15 privacy.rs + 4 forensic_capture.rs + 2 Playwright round-trips.
- **Quality Gates.** `cargo check --workspace` + `cargo build --workspace` + `tsc --noEmit` + `prettier --check` + `markdownlint-cli` all pass. Quality bar (300/50/100 file/function/line caps) in [`protocol.config.yaml`](protocol.config.yaml).

---

## Architecture

```text

[ Next.js 15 + React 19 + HeroUI v3 renderer (App Router, static export; LIVE) ]
                                                                                  |
                                                                                  v
                                                                          [ Tauri 2 Rust shell ]    savant_shell::run() at src-tauri/src/lib.rs
                                                                                  |
                                                              +-------------------+-------------------+
                                                              |                   |                   |
                                                              v                   v                   v
                                                  master-key vault       OpenRouter client      Reflections IPC (FID-017)
                                                  (auth.json, OS-sp)     (reqwest, Bearer)       5 commands wire the consciousness
                                                                                                  state machine to the renderer

  Rust cognitive core — RESTORED ON DISK, mechanically verified (cargo check --workspace 0/0)
  21 savant-orig crates + lib/cortexadb/ + workspace-savant/SOUL.md anchor
  Phase 2 wires: trigger bus, SQLite WAL, dual-loop engine, iceoryx2 zero-copy IPC
```

The renderer (Next.js App Router, static export) is the live surface. The Tauri 2 Rust shell exposes auth + inference + reflections through typed IPC commands (legacy Option B). The 21-crate cognitive core (FID-016 restore) builds clean but its trigger bus / SQLite WAL / dual-loop engine are still Phase 2 wiring — the crates are present on disk, the gates pass, but the runtime has not yet started exercising them. The v0.0.7+ pathway replaces this Tauri-only surface with a CLI-launched Next.js dashboard + the gateway IPC substrate per master-FID-035 §Layered Build Order.

---

## Quick Start

### Option A — Browser preview (no Tauri install; recommended for development)

```bash
git clone https://github.com/savant0x/Savant
cd Savant
npm install
npm run dev                                # → http://localhost:3000
```

Tauri IPC is mocked via `src/lib/mock-ipc.ts`. Master key vault + Soul Builder + Reflections Viewer all work end-to-end. Fast iteration; no Rust rebuild loop.

On first launch:

1. Settings page prompts for your OpenRouter master key (browser preview uses mock `localStorage` vault).
2. Derived session subkey auto-provisioned via OpenRouter `/v1/keys`.
3. `/manifest` builder + `/reflections` viewer become available.

### Option B — Tauri desktop (real Rust IPC; legacy surface)

```bash
git clone https://github.com/savant0x/Savant
cd Savant
cargo install tauri-cli --version "^2.0"
npm install
cargo tauri dev                            # Launches the desktop window (Next.js dev server on :3000)
```

On first launch:

1. MasterKeySetup screen prompts for OpenRouter API key → OS app-data vault.
2. InferenceSmokeTest screen: type, click Run. `POST https://openrouter.ai/api/v1/chat/completions` returns via HeroUI Card.
3. Reflections page streams from the Rust daemon (5 IPC commands wire the consciousness state machine).

> Web deployment is unsupported. Renderer is `output: "export"` in [`next.config.mjs`](next.config.mjs) (required by Tauri's `frontendDist: "../out"` in [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json)). The `/api/env` route compiles to a static JSON file. Supported paths are browser preview (Option A) and Tauri desktop (Option B). The Tauri app reads env vars server-side via Rust IPC in production.

### Option C — CLI-launched (v0.0.7+, planned per master-FID-035)

The strangler-fig migration replaces the Tauri host with a `savant` CLI binary that spawns the Next.js dashboard via `next start` + opens the browser via `webbrowser::open`. The CLI imports `savant_gateway` + `savant_runtime` directly (ZeroClaw pattern). See FID-030 for the implementation record.

---

## Project Structure

```text
Savant/
├── Cargo.toml              # 22-member cargo workspace (src-tauri + 21 savant-orig crates)
├── Cargo.lock              # Locked dependency graph (~566M / 121K LOC across the 22 members + lib/cortexadb/)
├── package.json            # Next.js 15 renderer config + npm scripts
├── next.config.mjs         # Next.js config (with ?raw for .md files)
├── vitest.config.ts        # Unit test config (happy-dom env)
├── playwright.config.ts    # E2E test config (chromium)
├── tsconfig.json           # TypeScript strict mode
├── LICENSE                 # Apache 2.0 license (full text)
├── NOTICE                  # Attribution chain (Tauri, React, HeroUI, Next.js, Rust, ...)
├── README.md               # This file
├── CHANGELOG.md            # Release changelog
├── VERSION                 # Canonical version (last-RELEASED, 0.0.6)
├── STARTER-PROMPT.md       # ECHO Protocol boot sequence
├── protocol.config.yaml    # ECHO project config (commands, quality bar, autonomy)
├── ECHO.md                 # The 15 Laws + Perfection Loop FSM + FID lifecycle
├── MIGRATION.md            # Breaking protocol/file structure transitions
├── coding-standards/       # Per-language rules (Rust, TypeScript, Python, Go, Java, C#, x402)
├── templates/              # FID + session summary templates
├── public/                 # Static assets (favicon, manifest, sw, icons)
├── scripts/                # release.py + refresh-readme.sh + commit-with-message.sh + tag-with-message.sh + lint-docs.sh + lint-defer.sh + release-check.sh + verify-fix.sh + bump-version.sh + archive-fids.sh + clean-bloat.sh + release-prep.sh + sync-agents.py
├── src/                    # Next.js 15 renderer
│   ├── app/                # App Router pages (home, chat, manifest, settings, reflections, health, ...)
│   ├── components/         # React components (dashboard-shell, rating-box, bulk-diff-viewer, soul-body-viewer, ...)
│   └── lib/                # Utilities + IPC
│       ├── mock-ipc.ts     # Phase 1 mock IPC (auto-disables when __TAURI_INTERNALS__ is set)
│       ├── soul.ts         # Build-time `?raw` re-export of workspace-savant/SOUL.md
│       ├── reflections/    # FID-017: lens array + reflection parser (port from crates/agent/src/pulse/prompts.rs)
│       ├── hooks/          # use-derived-rotation, use-lens-rotation, use-reflections, use-loaded-config
│       ├── manifest-mock.ts, swarm-diff.ts, prompt-generator.ts, name-generator.ts, ...
│       └── *.test.ts       # vitest unit tests (17 cases across the source-tree utilities)
├── src-tauri/              # Tauri 2 Rust shell (`[lib] name = "savant_shell"`) — legacy Option B
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs         # savant_shell::run() entry point
│       ├── lib.rs          # 8 IPC commands (setup_master_key, infer_openrouter, vault_list_profiles + FID-017: start/stop_consciousness, get_consciousness_state, trigger_reflection, initialize_app_state)
│       ├── security/       # master_key.rs (5-strategy cascade vault)
│       └── inference/      # openrouter.rs (reqwest chat-completions)
│   └── tests/              # 2 integration tests (master_key_test + inference_smoke_test)
├── crates/                 # 21 savant-orig crates (FID-016 restore, mechanically verified, reads clean)
│   ├── core/ gateway/ agent/ skills/ mcp/ channels/ canvas/ cognitive/ ipc/ memory/
│   └── dream/ panopticon/ obsidian/ integrations/ security/ sandbox/ echo/ browser/
│       toolforge/ generation/ schema/
├── lib/
│   └── cortexadb/          # 5.2M / 22,816 LOC — vector zero-copy substrate
├── dev/                    # Engineering operations
│   ├── fids/               # Runtime FIDs (gitignored, created on first run)
│   ├── fids/archive/       # Closed FIDs (auto-archived per ECHO §FID Auto-Archive)
│   ├── nova/               # Cross-agent audit channel (inbox + outbox per LESSON-008)
│   ├── session-summaries/  # ECHO Protocol audit trail
│   └── LEARNINGS.md        # Cross-session retained knowledge
├── e2e/                    # Playwright round-trips (auto-derived session key)
└── workspace-savant/       # Agent resident workspace (FID-004r2)
    ├── SOUL.md             # Canonical persona (read by chat + manifest)
    ├── AGENTS.md           # Operating instructions
    ├── LEARNINGS.md        # Agent-written at runtime
    └── EVOLUTION.jsonl     # Parser-managed at runtime
```

### Rust Module Map

| Source                                  |  Status (v0.0.6)        | Purpose                                                                            |
| :-------------------------------------- | :---------------------: | :--------------------------------------------------------------------------------- |
| `src-tauri/`                            |          LIVE           | Tauri shell (legacy Option B): vault + OpenRouter + Reflections IPC                  |
| `crates/gateway/`                       |  PARTIAL, v0.0.6+      | Gateway IPC substrate (33 `/v1/*` endpoints + 1 SSE + embedded-web feature flag) — Layer 0 of the strangler-fig |
| `crates/core`                           |  RESTORED, on disk      | lib basename `savant_core` (savant-orig identity); Phase 2 wires unified vault     |
| `crates/agent`                          |  RESTORED, on disk      | Agent + skills + learning parser; memory browser planned for v0.0.5+               |
| `crates/cognitive`                      |  RESTORED, on disk      | Dual-loop engine (fast + slow reflection); Phase 3 wiring                          |
| `crates/security`                       |  RESTORED, on disk      | Unified multi-profile vault (Phase 2 replacement for `src-tauri/src/security/`)   |
| `crates/ipc`                            |  RESTORED, on disk      | Atlas zero-copy IPC substrate (iceoryx2); Phase 2 wiring                           |
| `crates/memory`                         |  RESTORED, on disk      | CortexaDB + privacy.rs scanner; Phase 2 wiring                                     |
| `crates/echo`                           |  RESTORED, on disk      | ECHO Protocol core (renderer ECHO is Phase 1, daemon ECHO is later)                 |
| `crates/{11 more}`                      |  RESTORED, on disk      | Per FID-016; cargo workspace unified                                               |

---

## Development & Building

```bash
# Renderer (Phase 1)
npm run dev                      # Browser preview (mock IPC) at :3000
npm run build                    # Static export (Next.js)
cargo tauri dev                  # Tauri desktop (real Rust IPC, legacy Option B)
cargo tauri build                # Tauri release executable

# Tests
npm run test                     # vitest unit tests
npm run test:e2e                 # Playwright E2E tests
npm run test:all                 # Unit + E2E

# Workspace-wide Rust build (FID-016 restore verification)
cargo check --workspace          # 0 errors, 0 warnings
cargo build --workspace          # 0 errors, 0 warnings

# Quality gates
npx tsc --noEmit                 # TypeScript type-check
npx prettier --check .           # Prettier format check
npx markdownlint-cli '**/*.md'   # Markdown lint

# Release-time automation (LESSON-027/028/029/030/031 + LESSON-058)
pnpm lint:docs                   # LESSON-027 doc-drift invariant (5 anchors + 1 cascade-prose)
pnpm lint:defer                  # LESSON-038 no-auto-defer invariant
pnpm release:check 0.0.7         # LESSON-029 3-gate pre-flight (clean tree + 0 transient + remote tag)
pnpm release:prep 0.0.7          # FID-024 §Step A orchestrator (archive + bump + refresh + clean + verify + release)
pnpm release:readme 0.0.7        # scripts/refresh-readme.sh standalone (badges + What's New + table)
pnpm git:commit                  # LESSON-030 file-based commit helper
pnpm git:tag                     # LESSON-030 file-based tag helper
pnpm verify:fix                  # LESSON-031 dual-check re-grep pattern
```

---

## Roadmap

| Version | Phase | Status   | Focus                                                                                                              |
| :------ | :---: | :------- | :----------------------------------------------------------------------------------------------------------------- |
| v0.0.1  |   1   | SHIPPED  | Tauri 2 shell + master-key vault + OpenRouter smoke-test                                                           |
| v0.0.2  |   1   | SHIPPED  | Auto-derived session key (FID-0003) + two-tier credential + vitest/Playwright test framework                        |
| v0.0.3  |   1   | SHIPPED  | Soul Builder (FID-006 v3) + LLM streaming (FID-010) + swarm diff (FID-013) + Perfection Loop (FID-009)              |
| v0.0.4  |   1   | SHIPPED  | Rust core restored (FID-016) + Reflections Viewer (FID-017) + lib rename (FID-016r2) + License MIT→Apache 2.0       |
| v0.0.5  |   1   | SHIPPED  | Bundle identifier + Cargo crate/binary alignment to `savant` brand                                                |
| v0.0.6  |   1   | SHIPPED  | Pre-pivot baseline + git release tooling (FID-022 LESSON-027/028/029/030/031 scripts) + cascade-recovery cycle codification (LESSON-053 + LESSON-054) + FID-031 gateway expansion foundation (Layer 0) + README auto-update pipeline |
| v0.0.7  |   1+  | IN PROGRESS  | **Foundation landed (8512f78, 2026-07-15):** LESSON-059 codification + clean reinstall + FID-029 §Step 1 sibling-collection pivot. Next: Layer 1a §Step 2 (FID-029 renderer IPC wrappers) + Layer 2 (FID-030 CLI runtime host) + Layer 3 (FID-032 api-client swap) + Layer 4 (FID-033 Tauri repackaging) + Layer 5 (FID-034 kernel trait adoption). |
| v0.1.0  |   2   | PLANNED  | Trigger bus + hybrid tick + SQLite WAL + dual-loop init + Rust module split (trigger/, state/, cognitive/)         |
| v0.2.0  |   3   | PLANNED  | Tiered inference (fast + slow reflection) + observability + 16-provider chain                                      |
| v0.3.0  |   4   | PLANNED  | Mandatory Security Scanner + Two-Tier Agent System + Distributed Memory Substrate + Channels                      |
| v0.4.0  |   5   | PLANNED  | Full UI shell (multi-pane dashboard + agent observability) + MCP + Windows DPAPI + release signing                 |

Each phase lives as a FID under [`dev/fids/`](dev/fids/) as it ships.

---

## Documentation

- [`ECHO.md`](ECHO.md) — The 15 Laws + Perfection Loop FSM + FID lifecycle + circuit breakers. The protocol this project obeys.
- [`CHANGELOG.md`](CHANGELOG.md) — Release history and changes (reverse chronological).
- [`MIGRATION.md`](MIGRATION.md) — Breaking protocol/file structure transitions.
- [`protocol.config.yaml`](protocol.config.yaml) — Build commands, quality bar, autonomy level, paths, testing, FID config.
- [`dev/LEARNINGS.md`](dev/LEARNINGS.md) — Cross-session retained knowledge + codified lessons.
- [`dev/session-summaries/`](dev/session-summaries/) — ECHO Protocol audit trail per release/session.
- [`coding-standards/`](coding-standards/) — Per-language rules (Rust, TypeScript, Python, Go, Java, C#, x402).
- [`templates/`](templates/) — FID + session summary templates.
- [`LICENSE`](LICENSE) — Apache 2.0 license (full text).
- [`NOTICE`](NOTICE) — Attribution chain (Tauri, React, HeroUI, Next.js, Rust, build/test tooling).

---

## License

Apache 2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).

Forward-effectivity: v0.0.3 and earlier releases remain under their original MIT license; the MIT → Apache 2.0 change applies from v0.0.4 forward. Apache 2.0 grants an explicit patent license from contributors to users, with a retaliation clause. Trademarks are NOT granted — the "Savant" name is reserved for the official project per §6 of the license.

---

<div align="center">

_Savant is a sovereign substrate, in flight._

**Savant** &bull; 2026

</div>
