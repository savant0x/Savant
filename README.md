<!-- markdownlint-disable MD033 -->
<div align="center">

<img src="img/savant.png" alt="Savant Logo" width="180" />

**Sovereign agent substrate. Phase 1: Renderer-first.**

A desktop-resident proactive AI shell built on Next.js 15, Tauri 2, and Rust. Currently in a renderer-first MVP rebuild focusing on secure credential architecture, IPC interfaces, and LLM manifestation tooling.

</div>

---

## What's New in v0.0.3

**Soul Builder feature (FID-006 v3), 43-issue Perfection Loop, vault security hardening, 3-way swarm diff preview.**

- **Soul Builder** — LLM-driven persona generation with chunked SSE streaming for progressive rendering without UI freezes. (FID-006 v3 + FID-010)
- **ECHO Perfection Loop** — Implementation of a 43-issue FSM logger and rigid double-audit testing suite for extreme quality control. (FID-009)
- **3-Way Swarm Diff** — Preview agent configurations via color-coded diffing before deployment. (FID-013)
- **Vault Security Hardening** — Dev environment safeguard preventing accidental credential leaks into the Tauri static bundle. (FID-008)
- **Two-Tier Credential Architecture** — Master-key OS vault with auto-derived session subkeys and cross-tab `localStorage` sync. (FID-0003)

See [CHANGELOG.md](CHANGELOG.md) for the complete v0.0.3 entry.

**Zero Mock-IPC Drift:** the renderer is the live surface — `npm run dev` boots the full dashboard with master-key vault, derived session subkey, and Soul Builder end-to-end. Tauri shell swaps the mock IPC for real Rust IPC transparently.

**Tauri Version Aligned to Renderer:** the Tauri workspace was at `0.2.0` from a prior drift session; v0.0.3 cut brings it to `0.0.3` to match `package.json`. Per the release-only-versioning discipline codified in [dev/LEARNINGS.md](dev/LEARNINGS.md), version files rock only at release time.

**Apache 2.0 License Migration:** forward-effective from v0.0.4. Adds explicit patent grant + retaliation clause (per `workspace-savant/SOUL.md` AAA substrate's "Zero-Trust" + "Sovereign-Autonomy" laws). See [LICENSE](LICENSE) + [NOTICE](NOTICE).

---

<div align="center">

[![React](https://img.shields.io/badge/React-19-%23000000?style=flat-square&logo=react&logoColor=%2300fbff)](https://react.dev/)[![Next.js](https://img.shields.io/badge/Next.js-15-%23000000?style=flat-square&logo=nextdotjs&logoColor=%2300fbff)](https://nextjs.org/)[![Tauri](https://img.shields.io/badge/Tauri-2.x-%23000000?style=flat-square&logo=tauri&logoColor=%2300fbff)](https://tauri.app/)[![Rust](https://img.shields.io/badge/Rust-1.86+-%23000000?style=flat-square&logo=rust&logoColor=%2300fbff)](https://www.rust-lang.org/)[![TypeScript](https://img.shields.io/badge/TypeScript-5.7-%23000000?style=flat-square&logo=typescript&logoColor=%2300fbff)](https://www.typescriptlang.org/)[![HeroUI](https://img.shields.io/badge/HeroUI-v3_Alpha-%23000000?style=flat-square&logo=react&logoColor=%2300fbff)](https://heroui.com/)[![License](https://img.shields.io/badge/License-Apache_2.0-%23000000?style=flat-square&logo=github&logoColor=%2300fbff)](LICENSE)[![Status](https://img.shields.io/badge/Status-Phase_1:_In_Flight-%23000000?style=flat-square&color=yellow)](CHANGELOG.md)

</div>

---

## Current State & Live Features

**v0.0.3 ships a focused MVP surface.** The following features are live, tested, and audit-verified:

- **Tauri 2 Desktop Shell** — Rust daemon providing OS-level persistent vault and secure OpenRouter inference client. `cargo tauri dev` launches a native window with real Rust IPC; the mock IPC self-disables when `window.__TAURI_INTERNALS__` is set.
- **Mock-IPC Browser Preview** — Iterate at Phase-1 velocity at `http://localhost:3000` via `@tauri-apps/api/mocks` (installed in `src/lib/mock-ipc.ts`). Master-key vault, derived session subkey, and Soul Builder all work end-to-end without a Tauri host. Fast visual iteration; no Rust rebuild loop.
- **Soul Manifestation Engine** — LLM-powered builder for zero-hallucination agent configuration writing. The system prompt in `src/lib/soul-generation-system-prompt.ts` leads with a "CRITICAL DIRECTIVE: PROMPT-DRIVEN IDENTITY" section that BANS generic AAA/foundation/sovereign/WAL/CCT language and forces every section to be UNIQUE to the prompt's domain. 18-section AAA Master Framework template.
- **SSE Streaming** — OpenRouter chunked responses yield `preamble` / `chunk` / `complete` / `error` events. rAF-throttled state updates (50-200 chunks/sec) accumulate in a `useRef` and flush via `requestAnimationFrame` to avoid render thrashing. AbortSignal-aware: the Cancel button stops the in-flight fetch + SSE parser cleanly.
- **Two-Tier Credential Architecture** — Master key stored in OS app-data vault (`%APPDATA%/savant/auth.json` Windows, `~/.config/savant/auth.json` Unix). Save Master Key fires a chain: vault write → POST `/v1/keys` provision → `LS_DERIVED` write. The "Saved" indicator only flips after BOTH stages succeed. The derived subkey is what chat outbound traffic uses; the master never reaches HTTP.
- **OpenRouter Inference** — Single provider gateway configured via OS master-key. Two-tier credentials (master + derived subkey) auto-rotated ≥24h. `OPENROUTER_MASTER_KEY` env var takes priority when set (tier 1), vault entry is tier 2. Cross-tab `localStorage` sync via `storage` event listener keeps two-tab UX consistent.
- **ECHO Protocol Runtime** — 15 Laws + Perfection Loop FSM (RED → GREEN → AUDIT → SELF-CORRECT → COMPLETE) + FID lifecycle (Created → Analyzed → Fixed → Verified → Closed → Archived). All agent work obeys the ECHO discipline; see [ECHO.md](ECHO.md).
- **TypeScript Strict Mode** — `strict: true` required in `tsconfig.json`. Named exports only, no defaults. `unknown` over `any`. Prefer interface over type for object shapes. 17 unit tests across the `logger.ts` utility (`src/lib/logger.test.ts`) + 5 vitest + 2 Playwright round-trips.
- **Quality Gates** — `tsc --noEmit` + `prettier --check` + `markdownlint-cli` + `cargo check --workspace` all pass. Quality bar: 300 / 50 / 100 file/function/line caps. Full table in `protocol.config.yaml`.

> **Honest framing:** The Rust cognitive core (Trigger bus, SQLite WAL, dual-loop engine), Swarm Orchestration, 16-provider chain, Mandatory Security Scanner, Distributed Memory Substrate, Two-Tier Agent System, and Channels are **planned for v0.1.0+** — not live in v0.0.3. The v0.0.3 MVP is intentionally narrow: renderer-first, OpenRouter-only, Tauri-shell-with-auth-and-inference. The roadmap below shows the buildout sequence.

---

## Architecture

<div align="center">

<img src="img/architecture.png" alt="Savant Architecture v0.0.3" width="850" />

</div>

```text
[ Next.js 15 + React 19 + HeroUI v3 renderer (App Router, static export; LIVE v0.0.3) ] <-- IPC --> [ Tauri 2 Rust daemon (auth + inference; LIVE v0.0.3) ]
                                                          |
                                                          +-- master-key vault (auth.json, OS-specific path)
                                                          +-- OpenRouter client (reqwest, Bearer auth)
                                                          |
                                                          |   [ Rust cognitive core — NOT YET BUILT ]
                                                          +-- - - - Phase 2: trigger bus + hybrid tick
                                                          +-- - - - Phase 2: SQLite WAL durable state
                                                          +-- - - - Phase 3+: dual-loop cognitive engine
```

The renderer is the live surface and ships the Soul Builder, SSE streaming, and Swarm Deployment diffing. The Tauri 2 daemon provides the master-key vault + OpenRouter inference client as a thin IPC layer over the renderer. The Rust cognitive core (Trigger, State, Cognitive loops) is **not yet built** — the slots below are reserved for follow-on work, not live today.

---

## Quick Start

There are two ways to run Savant. Pick the one that matches what you're doing.

### Option A — Browser preview (no Tauri install required, fastest iteration)

```bash
git clone https://github.com/savant0x/Savant
cd Savant
npm install                                # Next.js 15 + React 19 + HeroUI v3 alpha
npm run dev                                # → http://localhost:3000
```

Open `http://localhost:3000` in any browser. Tauri IPC is mocked via `@tauri-apps/api/mocks` (installed in `src/lib/mock-ipc.ts`), so the dashboard renders and `MasterKeySetup` + `SoulBuilder` work end-to-end without a Tauri host. Fast visual iteration on the UI; no Rust rebuild loop.

On first launch:

```text
1. Settings page prompts for your OpenRouter master key
2. Stored in mock localStorage vault (browser preview only)
3. Derived session subkey auto-provisioned via OpenRouter /v1/keys
4. Soul Builder becomes available at /manifest
```

### Option B — Tauri desktop (real Rust IPC, native window)

```bash
git clone https://github.com/savant0x/Savant
cd Savant
cargo install tauri-cli --version "^2.0"   # v2.10.1 verified on Windows 11 dev box
npm install                                # Next.js 15 + React 19 + HeroUI v3 alpha
cargo tauri dev                            # launches the desktop window (Next.js dev server on :3000)
```

On first launch:

```text
1. MasterKeySetup screen prompts for your OpenRouter API key
2. Stored in OS app-data vault:
     Windows: %APPDATA%/savant/auth.json
     Unix:    ~/.config/savant/auth.json
3. InferenceSmokeTest screen has a textarea — type anything, click Run
4. Response from POST https://openrouter.ai/api/v1/chat/completions
   appears in a HeroUI Card below the input
```

> **Web deployment is not supported.** Savant's renderer is built with `output: "export"` in `next.config.mjs` (required by the Tauri static export at `frontendDist: "../out"` in `tauri.conf.json`). The `/api/env` route is compiled to a static JSON file at build time and cannot read server env vars at runtime in a static export. The two supported paths are the browser preview (Option A, dev server with mocked IPC) and the Tauri desktop (Option B, real Rust IPC). The Tauri app reads the env var server-side via Rust IPC in production, so the env var tier remains functional.

---

## Project Structure

```text
Savant/
├── Cargo.toml              # Workspace root (single member: src-tauri)
├── package.json            # Next.js 15 renderer config + npm scripts
├── next.config.mjs         # Next.js config (with ?raw for .md files)
├── vitest.config.ts        # Unit test config (happy-dom env)
├── playwright.config.ts    # E2E test config (chromium)
├── tsconfig.json           # TypeScript strict mode
├── LICENSE                 # Apache 2.0 license (full text)
├── NOTICE                  # Attribution chain (Tauri, React, HeroUI, etc.)
├── README.md               # This file
├── CHANGELOG.md            # Release changelog
├── VERSION                 # Canonical version (last-RELEASED)
├── STARTER-PROMPT.md       # ECHO Protocol boot sequence
├── protocol.config.yaml    # ECHO project config (commands, quality bar, autonomy)
├── ECHO.md                 # The 15 Laws + Perfection Loop FSM + FID lifecycle
├── MIGRATION.md            # Breaking protocol/file structure transitions
├── coding-standards/       # Per-language rules (Rust, TypeScript, Python, Go, Java, C#, x402)
├── templates/              # FID + session summary templates
├── public/                 # Static assets
│   └── favicon/            # Favicon files (16x16, 32x32, apple-touch, android-chrome)
├── scripts/                # release.py + sync-agents.py
├── src/                    # Next.js 15 + React 19 + HeroUI v3 alpha renderer
│   ├── app/                # App Router pages (home, chat, manifest, settings, etc.)
│   ├── components/         # React components (dashboard-shell, rating-box, etc.)
│   └── lib/                # Utilities + IPC (logger, soul, manifest-mock, mock-ipc, etc.)
├── src-tauri/              # Tauri 2 Rust daemon (single crate, will split in Phase 2+)
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs
│       ├── security/       # master_key vault (5-strategy cascade)
│       └── inference/      # openrouter client (reqwest chat-completions)
├── dev/                    # Engineering operations
│   ├── fids/               # Runtime FIDs (gitignored, created on first run)
│   ├── fids/archive/       # Closed FIDs (auto-archived per ECHO §FID Auto-Archive)
│   ├── session-summaries/  # ECHO Protocol audit trail
│   └── LEARNINGS.md        # Cross-session retained knowledge
├── tests/                  # Rust integration tests
├── e2e/                    # Playwright E2E tests
└── workspace-savant/       # Agent resident workspace (FID-004r2)
    ├── SOUL.md             # Canonical persona
    ├── AGENTS.md           # Operating instructions + private diary spec
    ├── LEARNINGS.md        # Agent-written at runtime
    └── EVOLUTION.jsonl     # Parser-managed at runtime
```

### Rust Module Map

| Module                     |      Status      | Purpose                                                                                                             |
| :------------------------- | :--------------: | :------------------------------------------------------------------------------------------------------------------ |
| `src-tauri/src/security/`  |  LIVE (v0.0.3)   | Generalized multi-profile `Vault` (5-strategy cascade: env vars → cwd `.env` → exe `.env` → JSON vault → UI prompt) |
| `src-tauri/src/inference/` |  LIVE (v0.0.3)   | `openrouter` provider client (reqwest chat-completions, reads `openrouter-default` profile)                         |
| `src-tauri/src/trigger/`   | PLANNED (v0.1.0) | Trigger bus + hybrid tick scheduler                                                                                 |
| `src-tauri/src/state/`     | PLANNED (v0.1.0) | SQLite WAL durable state                                                                                            |
| `src-tauri/src/cognitive/` | PLANNED (v0.3.0) | Dual-loop cognitive engine (fast loop + slow reflection)                                                            |

---

## Development & Building

```bash
npm run dev                # Browser preview (mock IPC) at :3000
cargo tauri dev            # Tauri desktop (real Rust IPC)
npm run build              # Static export (Next.js)
cargo tauri build          # Tauri release executable

npm run test               # vitest unit tests
npm run test:e2e           # Playwright E2E tests
npm run test:all           # Unit + E2E

npx tsc --noEmit           # TypeScript type-check
npx prettier --check .     # Format check
npx markdownlint-cli '**/*.md'  # Markdown lint
cargo check --workspace    # Rust type-check
```

---

## Roadmap

| Version | Phase | Status  | Focus                                                                                                                                                          |
| :------ | :---: | :------ | :------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| v0.0.1  |   1   | SHIPPED | Tauri 2 shell + master-key vault + OpenRouter smoke-test                                                                                                       |
| v0.0.2  |   1   | SHIPPED | Auto-derived session key (FID-0003) + two-tier credential architecture + vitest/Playwright test framework                                                      |
| v0.0.3  |   1   | **NOW** | Soul Builder (FID-006 v3) + LLM streaming (FID-010) + swarm diff (FID-013) + Perfection Loop (FID-009) + env key security + dev server fixes                   |
| v0.0.4  |   1   | NEXT    | First Apache 2.0 release; (in flight) Quality Loop + early swarm plumbing                                                                                      |
| v0.0.10 |   1   | PLANNED | Phase 1 stabilization (before minor bump)                                                                                                                      |
| v0.1.0  |   2   | PLANNED | Trigger bus + hybrid tick + SQLite WAL durable state + dual-loop init + Rust module split (trigger/, state/, cognitive/)                                       |
| v0.1.10 |   2   | PLANNED | Phase 2 stabilization (before minor bump)                                                                                                                      |
| v0.2.0  |   3   | PLANNED | Tiered inference (fast loop + slow reflection) + observability + 16-provider chain                                                                             |
| v0.3.0  |   4   | PLANNED | Mandatory Security Scanner + Two-Tier Agent System + Distributed Memory Substrate + Glass House (Obsidian sync) + Channels (Discord, Telegram, WhatsApp, etc.) |
| v0.4.0  |   5   | PLANNED | Full UI shell (multi-pane dashboard + agent observability) + MCP integration + Windows DPAPI hardening + release signing + auto-update                         |

Each phase lives as a FID under `dev/fids/` as it ships.

---

## Documentation

- [ECHO.md](ECHO.md) — The 15 Laws + Perfection Loop FSM + FID lifecycle + circuit breakers. The protocol this project obeys.
- [CHANGELOG.md](CHANGELOG.md) — Release history and changes (reverse chronological).
- [MIGRATION.md](MIGRATION.md) — Breaking protocol/file structure transitions.
- [protocol.config.yaml](protocol.config.yaml) — Build commands, quality bar, autonomy level, paths, testing, FID config, perfection-loop parameters.
- [dev/LEARNINGS.md](dev/LEARNINGS.md) — Cross-session retained knowledge + codified lessons.
- [dev/session-summaries/](dev/session-summaries/) — ECHO Protocol audit trail per release/session.
- [coding-standards/](coding-standards/) — Per-language rules (Rust, TypeScript, Python, Go, Java, C#, x402).
- [templates/](templates/) — FID + session summary templates.
- [LICENSE](LICENSE) — Apache 2.0 license (full text).
- [NOTICE](NOTICE) — Attribution chain (Tauri, React, HeroUI, Next.js, Rust, build/test tooling).

---

## License

Apache 2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).

Apache 2.0 grants an explicit patent license from contributors to
users, with a retaliation clause (if you sue the project for
patent infringement, your license terminates). Trademarks are
NOT granted — the "Savant" name is reserved for the official
project per §6 of the license. See the [LICENSE](LICENSE) for
the full text and the [NOTICE](NOTICE) for the attribution chain.

**Forward-effectivity:** v0.0.3 and earlier releases remain under
their original MIT license. The MIT → Apache 2.0 change applies
forward-effective from the next release (v0.0.4).

---

<div align="center">

_Savant is a sovereign substrate, in flight._

**Savant** &bull; 2026

</div>
