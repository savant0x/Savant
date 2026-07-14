# Changelog

All notable changes to this project are documented here automatically by the
agent when a FID reaches **Closed** status. Entries are added in reverse
chronological order (newest first).

Format: Each entry includes the version, date, and changes.

**Version source of truth:** `package.json` is canonical (last-RELEASED
version). All project-meta files (`VERSION`, `protocol.config.yaml`
`project.version`, `README.md` headline + §Versioning rule, and the
most recent RELEASED entry in this `CHANGELOG.md`) MUST mirror
`package.json`. **Versions rock only at release time** — never bump
these files speculatively mid-development, regardless of how much code
work has accumulated. Work-in-progress lives under `## [Unreleased]`
below and gets tagged with a `## v0.0.X — YYYY-MM-DD` header at release
time. The `protocol.version` axis (`protocol.config.yaml`) is
independent (ECHO Protocol release cycle, separate from the Savant
project).

**Versioning rule:** This project uses the "10 patch releases per minor
number" rule. See `coding-standards/release-workflow.md` for the full spec.

**Notice:** The previous housekeeping pass (2026-07-12) bumped project
meta-files `0.0.1 → 0.1.4` _speculatively_ without a corresponding
release push. That was against the release-only discipline and has been
reverted; the work it captured now lives under `## [Unreleased]` below
and gets tagged on the next release. Cross-ref:
`dev/LEARNINGS.md` "Versions rock ONLY at release time" entry.

---

## v0.0.4 — 2026-07-13

Work-in-progress since v0.0.3. Captures FID-016 (Rust core restored —
21 crates + `lib/cortexadb/` + 4 root configs from `Savant-backup/`)
+ FID-016r2 (surgical rename `savant_core` → `savant_shell` in the
`src-tauri/` `[lib]` block to close the 3 cargo-build filename-collision
warnings; SUB-FID lives inline in the FID-016 row above per the
SUB-FID convention — no separate row needed) + FID-017 (Reflections
Viewer MVP — 5 Tauri commands + `AppState` + LENSES array port +
2 new React hooks + Markdown renderer swap from hand-rolled to
`react-markdown@^10.1.0` + `remark-gfm@^4.0.1`) + the License MIT →
Apache 2.0 forward-effectivity switch (existing v0.0.3 and earlier
remain MIT — preserved under their original license per the
forward-effectivity clause). Tagged `## v0.0.4 — 2026-07-13` at the
release cut per the release-only-versioning discipline; work-in-progress
against v0.0.5 lives under `## [Unreleased]` below. Verified clean:
`cargo build --workspace` (2:05, 0/0); `cargo check --workspace`
(2:18, 0/0); `npx tsc --noEmit` (clean); `npm run build` (17/17
routes); `npx prettier --check` (clean on the close-out files —
the 217 pre-existing repo files are out-of-scope per FID-009/FID-017
historical pattern). Plus the brick-level FID-016r2 artifact inspection
confirmed `savant_shell.{dll,lib,pdb,d,dll.exp,dll.lib}` + 
`savant_core.{dll,lib,pdb,d,dll.exp,dll.lib}` + `savant-core.exe` =
13 distinct artifact basenames, 0 collisions.

**TLDR:** Rust core restored under the renderer; lib basename collision
closed via surgical 4-file rename; reflections viewer with 5 IPC commands
+ lens rotation + react-markdown swap; license forward-switched to
Apache 2.0 with `[Unreleased]` re-opened for v0.0.5 work. FID-016r2 is
a SUB-FID of FID-016 (audit-driven restore of incomplete rename
provenance — closes the LESSON-025 forward-effective cross-link);

### Added

- **Rust core restored (FID-016)** — `Savant/` now contains the full savant-orig Rust workspace as a 22-member cargo workspace. Copied 21 crates (`core, gateway, agent, skills, mcp, channels, canvas, cognitive, ipc, memory, dream, panopticon, obsidian, integrations, security, sandbox, echo, browser, toolforge, generation, schema`; ~566M / 121K LOC) + `lib/cortexadb/` (5.2M / 22,816 LOC) + 4 root configs (Cargo.lock, clippy.toml, rustfmt.toml, deny.toml) from `Savant-backup/`. New `Savant/Cargo.toml` members: `src-tauri` + the 21 restored crates. `[workspace.dependencies]` is `Savant-backup`'s verbatim (minus 4 `savant_cli_*` paths; the `crates/cli/*` subtree and `crates/desktop/src-tauri` are excluded as the terminal CLI + old Tauri host are replaced by the current `Savant/src-tauri/` + the Next.js dashboard). Verified clean: `cargo check --workspace` (3:23, 0 errors, 0 warnings) + `cargo build --workspace` (6:32, 0 errors, 3 filename-collision warnings tracked as FID-016r2). 0 hard stubs across the 21 crates — the savant-orig "no AI slop" discipline is mechanically verified. Cross-ref: [`dev/fids/FID-2026-07-13-016-restore-rust-core.md`]. **This is the foundation for FID-017+ per-subsystem wiring** (inner monologue MVP, memory browser, skills marketplace, etc.).

- **Reflections Viewer MVP (FID-017)** — [`src-tauri/src/lib.rs`] adds 5 Tauri commands
  (`start_consciousness` / `stop_consciousness` / `get_consciousness_state` /
  `trigger_reflection` / `initialize_app_state`) plus the `AppState` struct
  (`workspace_path` + `state_handle Arc<AtomicU8>` + `lens_index Mutex<usize>` +
  daemon lifecycle `JoinHandle + CancellationToken` + `daemon_state`
  consciousness mirror). The 19-entry `LENSES` array is ported verbatim from
  [`crates/agent/src/pulse/prompts.rs:147`] to [`src/lib/reflections/lenses.ts`]
  (no design changes to the 2:1 emergent/operational weighting per
  LESSON-018 source-faithful rebuild). Two new React hooks:
  [`src/lib/hooks/use-lens-rotation.ts`] (pure selector — given an index,
  derives `{ name, prompt, type: EMERGENT|OPERATIONAL|UNKNOWN, index,
  nextName, prevName, rotationPosition, rotationTotal }` from the LENSES
  array — NOT a state machine; the index lives in Tauri state) and
  [`src/lib/hooks/use-reflections.ts`] (boot-time reader for
  `workspace-savant/REFLECTIONS.md` + `workspace-savant/SOUL.md` with the
  Tauri-runtime SOUL.md fetch guarded by
  `if ("__TAURI_INTERNALS__" in window)` to prevent 404 spam in browser
  preview; journal-style parser `split(/(^|\n)##\s+/)`; legacy-key
  migration block preserves user data through the rename
  `savant.monologue.reflections` → `savant.reflections.entries`). New page
  at [`src/app/reflections/page.tsx`]: control bar + streaming indicator
  **inline at the top** + journal timeline — single unified stream of
  consciousness. REFLECTIONS.md format is `## [YYYY-MM-DD HH:MM:SS UTC]
  [BODY]` with NO per-entry `[LENS]` tag — the lens rotates the LLM
  prompt angle only; the output is ONE continuous journal of
  consciousness (per Spencer 2026-07-13: *"all lenses are supposed to
  be a single stream, not separated by lenses but all joined together"*).
  Date headers via new `formatFullTimestamp` helper in
  [`src/lib/format-relative-time.ts`] (`YYYY-MM-DD HH:MM:SS UTC` format);
  semantic `<time dateTime={r.ts}>` markup for entries. Flat-bordered
  card design (`rounded-none border border-default/30 px-5 py-4`)
  replacing earlier over-rounded `rounded-lg` HeroUI surfaces per design
  feedback. **`mock-ipc.ts` `trigger_reflection`** case integration:
  real HTTP POST to `https://openrouter.ai/api/v1/chat/completions` when
  an OpenRouter master key is captured in `mockMasters["openrouter"]`;
  surfaces 401 with the active key source (`env` vs `vault` vs `none`)
  + key length so the user can immediately tell WHICH tier is the
  problem; if both tiers are empty, surfaces a clear "No model
  configured — set your model in Settings → OpenRouter before triggering
  reflections" error. No random default model — Settings page is the
  single point of model choice. **§Perfection Loop 5 — Markdown renderer
  swap**: the hand-rolled `MarkdownLite` parser (in
  [`src/lib/markdown-lite.tsx`], ~280 lines covering only h1-h3 /
  `**bold**` / `*italic*` / `> blockquote` / `---` hr / HTML entities) was
  replaced with `react-markdown@^10.1.0` + `remark-gfm@^4.0.1` for full
  CommonMark + GFM coverage (lists with nesting; fenced code blocks;
  tables; task lists; strikethrough; autolinks; all heading levels; hard
  line breaks; escape sequences). 97 transitive packages added.
  [`src/lib/markdown-lite.tsx`] deleted. Per Spencer *"ALL of them,
  that's the entire point of the parser."* Custom `a` component opens
  external `http(s)` links in new tabs with `rel="noopener noreferrer"`;
  internal anchors stay default behavior. XSS-safe by construction (no
  `dangerouslySetInnerHTML`; ref-based AST traversal → React elements;
  React's native escaping in string children).  Cross-ref:
  [`dev/fids/archive/FID-2026-07-13-017-reflections-viewer-mvp.md`].

- **Tauri vault: Phase 5 DPAPI at-rest encryption + atomic writes** [`src-tauri/src/security/master_key.rs`]: closes the laptop-theft risk surface on the Windows Tauri vault. **DPAPI at-rest encryption** — `CryptProtectData` / `CryptUnprotectData` with user scope (`CRYPTPROTECT_LOCAL_MACHINE` flag = 0) wraps the entire vault blob. The new `SAVANT_VAULT_V2\n` magic header (16 bytes, `pub const MAGIC_HEADER`) prefixes the encrypted bytes; legacy v1 plain-JSON files (no magic header) are auto-detected on first read and re-written as v2 (lazy v1→v2 migration; idempotent on subsequent loads, verified by the `v2_load_is_idempotent_no_rewrite` mtime + size test). Standard Windows password changes do NOT invalidate the DPAPI master key (Windows re-wraps on logon); a system-level admin force-reset can. Unix is a passthrough (libsecret parity is a future FID). **Atomic `write_vault_file`** — writes to `<path>.tmp` first, then `fs::rename` over the original; a failed write (disk full, perms, ENOSPC) leaves the original vault untouched (verified by the `failed_write_does_not_corrupt_existing_vault` test, which forces a failure by pre-creating a directory at the `.tmp` path and verifies the original's size + mtime + content are unchanged). 3 new `VaultError` variants (`EncryptionFailed(String)` / `DecryptionFailed(String)` / `CorruptedVault`) are distinct from `Io` so a DPAPI failure cannot be masked as "file not found" and trigger a silent `Vault::default()` overwrite. 2 new public test helpers (`load_vault_from_path`, `write_vault_to_path`; plus `MAGIC_HEADER` and `tmp_path_for` as `pub`) let integration tests exercise the migration + atomicity without touching the user's real OS vault. 9 new integration tests (6 DPAPI + 3 atomicity): DPAPI roundtrip, v2 magic header detection, v2 roundtrip, v1→v2 migration, v2-load idempotent, header-only `CorruptedVault`, `tmp_path_for` path-scheme, no-leftover-tempfile, failed-write-doesn't-corrupt. Windows-only dep: `windows = { workspace = true, features = ["Win32_Foundation", "Win32_Security_Cryptography"] }` under `[target.'cfg(target_os = "windows")'.dependencies]` in [`src-tauri/Cargo.toml`]. `cargo check -p savant-core` clean (11:22 cold build, 0 warnings). `code-reviewer-minimax-m3` PASS on the DPAPI pass and the atomic-write followup. **Replaces the prior `Phase 5` marker comment at [`src-tauri/src/security/master_key.rs:201`]** ("Windows: default ACL for Phase 1. DPAPI integration is Phase 5.").

### Changed

- **License: MIT → Apache 2.0** (forward-effective from v0.0.4 release).
  Existing v0.0.3 and earlier releases remain under their original MIT
  license. Adds explicit patent grant + retaliation clause (per
  `workspace-savant/SOUL.md` AAA substrate's "Zero-Trust" +
  "Sovereign-Autonomy" laws). Aligns with Tauri 2's dual-license
  (MIT/Apache-2.0). `NOTICE` file added with attribution chain +
  2026-07-13 boilerplate separation note. ECHO `protocol.config.yaml`
  `project.license` field added for metadata symmetry.

- **Vault extracted to `savant-vault` workspace crate (FID-019)** — Vault
  extracted from `src-tauri/src/security/master_key.rs` to a new
  `savant-vault` workspace crate. IPC surface (`setup_master_key`,
  `infer_openrouter`, `vault_list_profiles`) unchanged; consumers of
  `savant_shell::security::master_key` must migrate to
  `savant_vault::master_key`.
- **Cascade-ordering rationale consolidated (FID-021, doc-only)** —
  moved the dotted cascade-ordering rationale (5-strategy vault cascade:
  env vars → cwd `.env` → exe-dir `.env` → JSON vault → UI prompt, plus
  the cwd-FIRST `.env` precedence rule from FID-020/FID-020r2) into a
  single canonical paragraph at
  [`crates/vault/src/master_key.rs`](crates/vault/src/master_key.rs)
  (cascade docstring referencing the canonical cascade-ordering phrase section
  the 5-strategy enumeration). All 5 prior duplicates replaced with
  one-line forward-pointers referencing the canonical by exact phrase
  — enables a future doc-drift linter to detect drift via
  (the canonical cascade-ordering phrase is the drift-detection anchor;
  expects 5 anchors across the codebase after consolidation: 1 canonical +
  4 forward-pointers).
  Drift closed: 4 cascade-prose duplicates removed (`src-tauri/src/lib.rs::run()`
  doc comment, `src-tauri/src/lib.rs::load_env_from_exe_dir` doc comment,
  `src-tauri/Cargo.toml` `dotenvy` dep comment, the drifted [Unreleased]
  CHANGELOG entry's FID-020r2 `### Fixed` paragraph). 4 file edits,
  0 lines of code, 0 test changes. `cargo check --workspace --tests`
  clean; `cargo test --test vault_dotenv_strategy_test` 4/4 PASS;
  substring invariant: lib.rs=2 + Cargo.toml=1 + CHANGELOG=1 +
  master_key.rs=1 = 5 anchors (4 forward-pointers + 1 canonical). Cross-ref:
  [`dev/fids/archive/FID-2026-07-13-021-cascade-doc-consolidation.md`].

### Fixed

- **Vault strategies 2/3 (`.env` file loading) wired at startup** (FID-020 + FID-020r2)
  [`src-tauri/src/lib.rs:savant_shell::run()`]: the vault's 5-strategy
  cascade listed strategies 2 (cwd `.env`) and 3 (exe-dir `.env`) in
  its docstring [`crates/vault/src/master_key.rs:25-26`], but no code
  ever called `dotenvy::dotenv()` to actually load those files —  so those two strategies have been silently  non-functional since at least the v0.0.4 release. **FID-020 wired
  strategy 2** (cwd `dotenvy::dotenv().ok();` at startup).
  **FID-020r2 wired strategy 3** (exe-dir `.env`) via the new
  [`savant_shell::load_env_from_exe_dir`] helper called from
  [`run()`] with `std::env::current_exe()`. **See
  [`savant_vault::master_key`] (cascade docstring "Precedence & `.env` loading" paragraph) for the cwd-FIRST `.env` precedence
  rationale.** The smoking gun: `dotenvy` was declared in
  `src-tauri/Cargo.toml` for the entire FID-019 sequence, but a
  workspace-wide grep for `use dotenvy` / `dotenvy::` returned zero
  matches (confirmed in the FID-019r2 dead-dep audit). The fix adds
  `dotenvy::dotenv().ok();` to the top of [`savant_shell::run()`] —
  the `.ok()` ignores the `IoError::NotFound` case (no `.env` file is
  the common dev / packaged-prod scenario — the OS env or the vault
  file covers it). Placed **before** `tracing_subscriber::fmt().init()`
  so that `RUST_LOG` set in `.env` is respected by the subscriber.
  The `dotenvy` workspace dep is re-added to `src-tauri/Cargo.toml`
  (was removed in FID-019r2 as "dead" before we realized the call
  site was missing). New integration test
  [`src-tauri/tests/vault_dotenv_strategy_test.rs`] creates a unique
  temp `.env` per process, calls `dotenvy::from_path().ok()` for
  parallel-test safety (avoids mutating process cwd, which would race
  with other tests), and asserts
  `savant_vault::master_key::resolve_secret("env:<TEST_KEY>")` returns
  the value. A 2nd test in the same file guards against regression:
  with no `.env` loaded, `resolve_secret` still errors with
  `InvalidKeyFormat` on an unset env var. `cargo check --workspace
  --tests` clean.

## [Unreleased]

Work-in-progress against v0.0.5. FID-021 (cascade-doc-consolidation)
close-out captures the v0.0.4 doc-cleanup work; subsequent v0.0.5 work
will land here.

### Added

### Changed

### Fixed

### Removed

## v0.0.3 — 2026-07-13

Work-in-progress since v0.0.2. Captures the in-repo
`fame0528`/`savant-protocol` cleanup performed on 2026-07-12
and the subsequent quality fixes (FID-006 v3, FID-007, FID-008,
FID-009) on 2026-07-13.

**TLDR:** Soul Builder feature (FID-006 v3) with LLM-driven
generation + SSE streaming (FID-010), 3-way swarm diff preview
before deploy (FID-013), 43-issue Perfection Loop (logger +
redact() + 17 tests), env key security fix (dev guard prevents
leak in Tauri bundle), dev server error fixes (favicon conflict +
/api/env static export), Tauri version aligned to renderer
(0.2.0 → 0.0.3).

### Fixed (FID-009 — Manifest page quality fix, 2026-07-13)

- **DEPTH locked at 50%** [`src/lib/manifest-mock.ts:381`]: removed the
  hardcoded `depth_score: 0.5` override on the template fallback. The
  template's real depth is ~0.85-0.95 (256 lines / 18 sections / 1000+
  words), not 0.5. The override was a temporary fix that became a
  permanent bug.
- **SECTIONS shows 26 but body has 18** [`src/lib/manifest-mock.ts:65,47`]:
  changed the section regex from `/##/g` to `/^##\s+/gm` to ONLY count
  top-level section headers (`## N. Title`) and NOT sub-section
  headers (`### The Foundation`). The template has 18 `##` + 8 `###` =
  26 total `##` matches with the old regex.
- **Note truncation "generatio"** [`src/app/manifest/page.tsx:583`]:
  extended the `builtSoul.note.slice(0, 120)` to `slice(0, 200)` to
  avoid the previous "generatio" truncation on the template-fallback
  note (143 chars). Full text is now in the `title` attribute for
  hover. Same treatment for `builtSoul.error`.

### Added (FID-012 — Per-section rating breakdown, 2026-07-13)

- **`computeSectionMetrics(content)`** [`src/lib/manifest-mock.ts`]: new
  utility that splits a SOUL.md body by its top-level `## ...` headers
  and computes per-section metrics (lines, words, density,
  completeness). Loose header regex (`/^(##\s+.+)$/gm`) to match LLM
  output variations like `## 1) Title`, `## 1 Title`,
  `## Section 1: Title`. Returns an empty array for empty content.
- **`SectionRatingCard` component** [`src/components/section-rating-card.tsx`]:
  new compact horizontal card (section number + truncated title +
  Lines/Words pills + density + completeness dot). Color thresholds
  for density: green >= 10, yellow >= 5, red < 5. Reusable per ECHO
  Law 13 (utility-first).
- **SECTIONS BREAKDOWN collapsible** [`src/app/manifest/page.tsx:Section 2`]:
  new `<details>` block below the SoulBodyViewer showing one
  `SectionRatingCard` per `## ...` section. Shows section count +
  completeness count in the summary. Collapsed by default (18 cards
  would dominate the page).

### Added (FID-013 — Swarm deployment preview diff, 2026-07-13)

- **`previewSwarmDiff(baseline, proposed)`** [`src/lib/swarm-diff.ts`]:
  new utility that computes a 3-way diff (added/modified/removed/
  unchanged) between two `AgentManifestPlan[]` arrays, matched by
  `name`. Uses a sync FNV-1a 32-bit hash (`computeSwarmHashSync`) for
  the per-agent equality check (avoids async crypto cost in the
  render path). Async SHA-256 variant (`computeSwarmHash`) exposed
  separately for future cryptographic-grade integrity flows.
- **`BulkDiffViewer` component** [`src/components/bulk-diff-viewer.tsx`]:
  new color-coded display: ADDED (green + plus icon), MODIFIED
  (yellow + pen icon), REMOVED (red + minus icon), UNCHANGED
  (muted, collapsed by default). Each item shows: name + content
  snippet (first 100 chars of soul). MODIFIED items show
  `before → after` hint.
- **`get_swarm_baseline` IPC command** [`src/lib/mock-ipc.ts`]: new
  case that reads the last successfully-deployed swarm from
  `localStorage["savant.bulk.baseline"]`. Returns `[]` on a fresh
  install (no baseline yet) or on missing/corrupt LS entry.
- **`bulk_manifest` baseline persistence** [`src/lib/mock-ipc.ts:case
"bulk_manifest"`]: on successful deploy, writes the payload to
  `LS_SWARM_BASELINE` so the next `get_swarm_baseline` returns it.
  Documented as non-transactional (quota/private-mode failure leaves
  baseline stale; low risk in practice).
- **Two-step deploy flow** [`src/app/manifest/page.tsx:onBulkDeploy`]:
  Step 1 (idle) parses JSON, fetches baseline, computes diff, shows
  preview. Step 2 (preview) dispatches `bulk_manifest`, updates
  baseline on success. New `onBulkCancel` clears the diff state +
  `bulkError` + stale `bulkResult`. JSON textarea is `readOnly` during
  preview/deploy to prevent the diff from going stale.

### Added (FID-014 — Regenerate button, 2026-07-13)

- **Regenerate button** [`src/app/manifest/page.tsx:Section 2 actions`]:
  new button next to Copy + Revert. On click, restores the
  persisted prompt + name to the textarea, increments a
  `regenCount` state, and re-runs the LLM with the persisted
  prompt + a `[variant #N]` suffix. Shows `Regen #N` when
  regenCount > 0, plain "Regenerate" otherwise.
- **Capture-then-reset pattern** [`src/app/manifest/page.tsx:onManifestSubmit`]:
  the counter is read into a local `submitRegenCount` variable at
  the top of the function, then immediately reset to 0 in React
  state. The suffix uses the captured value, NOT the React state.
  Guarantees the suffix is applied for EXACTLY the submit triggered
  by `onRegenerate` — the next MANIFEST SOUL click sees
  `regenCount=0` and gets no suffix.
- **Race condition closed** [`src/app/manifest/page.tsx:submitDisabled
  - Regenerate button`]: both the MANIFEST SOUL + Regenerate buttons
are gated on`regenPending`. The Regenerate callback's guard is
also extended:`if (!builtSoul || submitting || regenPending)`.
    Closes the double-click race during the regenPending window.
- **`regenPending` useEffect with ref** [`src/app/manifest/page.tsx`]:
  the effect deps are INTENTIONALLY only `[regenPending, submitting]`
  — `onManifestSubmit` is captured via a ref so the effect doesn't
  re-run on every state tick.

### Added (FID-010 — LLM response streaming, 2026-07-13)

- **Soul generation streaming via OpenRouter SSE** [`src/lib/manifest-mock.ts:generateSoulStream`]:
  replaces the blocking `manifest_soul` (10-30s UI freeze) with a
  token-by-token async generator. Yields `preamble` / `chunk` /
  `complete` / `error` events. Pre-yields the static `# SOUL.md`
  header before the LLM call so the user sees the file shape
  immediately even on a slow TTFB. AbortSignal-aware: the generator
  stops yielding when the renderer's Cancel button fires.
- **Buffered SSE parser** [`src/lib/manifest-mock.ts:parseSSEStream`]:
  handles TCP fragmentation correctly — a single TCP packet can
  split mid-JSON, so the parser maintains a string buffer and
  only processes events that have a complete `\n\n` boundary.
  Uses `TextDecoder({ stream: true })` for multi-byte UTF-8 splits.
- **Channel-shaped IPC contract** [`src/lib/ipc.ts:createManifestStreamChannel` +
  `manifestSoulStream`]: duck-typed Phase-1 mock of Tauri v2's
  `Channel<ManifestStreamEvent>`. The renderer creates a channel,
  subscribes via `onmessage`, passes it to `invoke("manifest_soul_stream", {...})`.
  Returns a `{ cancel, done }` handle. **Phase 2 Tauri migration
  is a drop-in replacement** — swap the channel factory for
  `new Channel<ManifestStreamEvent>()` and the renderer code stays
  unchanged.
- **Mock IPC streaming command** [`src/lib/mock-ipc.ts:manifest_soul_stream`]:
  background loop consumes `generateSoulStream()` and forwards
  each event via `channel.send()`. Maintains an `activeStreams`
  Map<streamId, AbortController> for explicit ID-based cancellation
  via the new `cancel_manifest_stream` command (safety net for
  cross-scope cancellation).
- **Live preview card** [`src/app/manifest/page.tsx:Section 1.5`]:
  visible only while a stream is in flight. Shows pulsing dot +
  "Streaming Soul Manifestation" label + char count + elapsed
  seconds + Cancel button. Live preview in a scrollable mono-font
  box with a blinking cursor.
- **rAF-throttled state updates** [`src/app/manifest/page.tsx:scheduleStreamFlush`]:
  OpenRouter can emit 50-200 chunks/sec; we accumulate in a
  `useRef<string>` and flush to React state via
  `requestAnimationFrame` to avoid render thrashing.
- **Elapsed-time ticker** [`src/app/manifest/page.tsx:streamStartTs` +
  `streamElapsedSec`]: `setInterval(100ms)` updates the elapsed
  seconds counter while a stream is in flight. Stops automatically
  on stream end.
- **Auto-cancel on resubmit** [`src/app/manifest/page.tsx:onManifestSubmit`]:
  if a previous stream is in flight when the user clicks MANIFEST
  SOUL again, the old stream is cancelled first. Prevents chunk
  interleaving across concurrent streams.
- **Cleanup on unmount** [`src/app/manifest/page.tsx` cleanup
  useEffect]: cancels any in-flight stream + clears the rAF tick
  on component unmount. Prevents memory leaks if the user
  navigates away mid-stream.

### Changed (FID-010 — LLM response streaming, 2026-07-13)

- **`src/app/manifest/page.tsx:onManifestSubmit`** — rewrote from
  single-Promise to channel-based streaming. The `await handle.done`
  awaits the background loop's unwind. The Draft Buffer is
  populated from the `complete` event's authoritative `result.content`
  (not the lagging `streamBuffer`, which may have been batched by
  rAF).
- **`src/lib/manifest-mock.ts`** — the `ManifestResult` import is
  now shared with the new `ManifestStreamEvent` type. The existing
  `generateSoul()` non-streaming function is preserved (used by
  the `manifest_soul` IPC for fallback paths + by future tests).

### Fixed (FID-010 — LLM response streaming, 2026-07-13)

- **UI freeze during soul generation** (10-30s blocking): replaced
  with progressive rendering. The user now sees the preamble
  immediately + each chunk as it arrives.
- **No cancel option**: added a Cancel button that aborts the
  in-flight fetch + SSE parser via the channel's `cancel()`.

### Added (FID-009 — Manifest page quality fix, 2026-07-13)

- **`src/lib/name-generator.ts`** (NEW): themed name generator with
  100 unique names across 5 themes (mythological, sci-fi, tech, nature,
  abstract). Exports `getRandomName()` (random theme + random name
  with closure-scoped `lastPick` to avoid immediate repeats),
  `getRandomNameByTheme(theme)`, `NAMES_BY_THEME` (theme → pool
  mapping for future theme-filter UI), and `TOTAL_NAME_COUNT` /
  `TOTAL_THEME_COUNT` constants for UI display.
- **`src/lib/prompt-generator.ts`** (NEW): curated example prompts
  with 20 prompts across different domains (hustler, security,
  poet, strategist, zen, ceo, negotiator, quantum, chef, crisis,
  philosopher, ai-safety, marine-bio, lawyer, chess, er-doctor,
  jazz, vc, astrophysicist, product-designer). Exports
  `getRandomPrompt()` (with `lastPromptPick` to avoid immediate
  repeats) and `getRandomPromptByDomain(domain)`.
- **`src/components/soul-body-viewer.tsx`** (NEW): custom Markdown
  renderer (~250 lines, no external dependencies — no
  `react-markdown` 50KB+). Splits content into blocks (by blank
  lines), parses each block by type, renders with Tailwind classes.
  Supports: `##` (h2 with accent), `###` (h3), `**bold**`, `` `code` ``,
  `-`/`*` lists, `1.` lists, `|` tables, `>` blockquotes, `---` (hr),
  indented code blocks, paragraphs. Replaces the previous plain
  `<pre>` block.
- **Random prompt generator** [`src/app/manifest/page.tsx`]: die icon
  overlaid inside the prompt textarea (absolute bottom-right). Click
  picks a random prompt from the 20 curated examples. Same
  "lastPick avoids immediate repeat" pattern as the name generator.
- **Die icon overlay inside name input** [`src/app/manifest/page.tsx`]:
  replaced the side-by-side flex layout with an absolute-positioned
  icon inside the input (right edge, `pr-10` on input). Same
  treatment for the prompt textarea (bottom-right, `pb-10` on textarea).
- **Professional placeholder text** [`src/app/manifest/page.tsx`]:
  replaced the hustler example placeholder with a professional
  description ("Describe the entity's core directive — its purpose,
  values, and operating philosophy…"). The placeholder now explains
  what the system prompt does (forces every section to be UNIQUE).

### Changed (FID-009 — Manifest page quality fix, 2026-07-13)

- **`src/components/rating-box.tsx`** (REWRITE): enterprise-grade
  redesign with larger value typography (text-xl → text-2xl on
  hover), hover state (border becomes accent + scale-up 1.02x +
  accent shadow), thicker progress bar (h-1 → h-1.5), gradient
  background, accent bar on left edge, optional `icon` + `sublabel`
  props, smoother transitions (300ms on hover, 700ms on progress bar).
  Backwards-compatible: all existing usages work without changes.

### Added

- **Agent-workspace convention (`workspace-savant/`)** — FID-004r2:
  the agent-workspace layout anchored strictly in savant-orig
  filesystem evidence. Sole human-scaffolded file:
  `workspace-savant/SOUL.md` (YAML frontmatter + persona body
  transcribed from [`src/app/chat/page.tsx:49-53`] SAVANT_SOUL
  const). **Three elements are runtime outputs and are not
  pre-scaffolded** (per Spencer's LEARNINGS-ownership correction on
  2026-07-13):
  - `skills/` — auto-scaffolded at agent boot per
    [`crates/agent/src/manager.rs:32`] (`tokio::fs::create_dir_all`).
  - `LEARNINGS.md` — agent-written at runtime per
    [`crates/agent/src/learning/parser.rs:25, 42`] (parser reads
    if-present; returns `Ok(0)` when absent).
  - `LEARNINGS.jsonl` — parser-managed at runtime per
    [`crates/agent/src/learning/parser.rs:26, 69-77`] (`OpenOptions
::new().create(true).append(true)`).
    Cross-ref: [`dev/fids/FID-2026-07-12-004r2-workspace-savant.md`]. Original FID-004 (r0, proposed invented layout: `README.md`,
    `manifest.toml`, `soul/savant.soul.yaml`, `evolution.log`) was
    audit-rejected on 2026-07-13 for producing claims not present in the
    savant-orig filesystem and is archived at
    [`dev/fids/archive/FID-2026-07-12-004-workspace-savant-system.md`]
    with the full divergent-table annotation.

- **Soul-manifest feature + workspace-savant scaffold (FID-006 v2):**
  source-faithful rebuild of the savant-orig persona system per
  `LESSON-018` (no deviation, no invented schemas). **Architecture
  (Option B+D):** `workspace-savant/SOUL.md` is the canonical
  disk-resident persona source (mirroring
  [`crates/core/src/fs/registry.rs:192`] disk loader + [`registry.rs:320-326`]
  default-write); `src/lib/soul.ts` is the build-time re-export via
  Next.js `?raw` import (production-safe under [`next.config.mjs:4`]
  `output: "export"`); both `src/app/chat/page.tsx` and
  `src/app/manifest/page.tsx` consume from `@/lib/soul` (ECHO Law 13).
  The drift class that produced FID-004r2/005r2/006-v1
  (YAML-frontmatter hallucination, `/api/soul` runtime route blocked
  by `output: "export"`) is fully closed. **10 file changes (7 NEW +
  1 EDIT build-config + 1 EDIT chat + 1 REWRITE manifest):**
  - `workspace-savant/SOUL.md` (NEW) — literal `registry.rs:320-326`
    default (`# Soul Configuration\n\n**Name:** Savant\n\n## Terminal
Mantra\n\nYou are a Savant autonomous agent. Operate with precision,
security, and autonomy.`).
  - `workspace-savant/AGENTS.md` (NEW) — literal `registry.rs:327-378`
    default (full operating instructions + private diary system spec).
  - `workspace-savant/LEARNINGS.md` (NEW) — literal `registry.rs:379-386`
    default.
  - `workspace-savant/EVOLUTION.jsonl` (NEW, empty) — `registry.rs:213-219`
    seeds empty.
  - `workspace-savant/skills/.gitkeep` (NEW, empty) — `manager.rs:33-34`
    `tokio::fs::create_dir_all(skills_dir)`.
  - `src/lib/soul.ts` (NEW) — `?raw` re-export of SOUL.md +
    `parse_soul_examples` 1:1 port of
    [`crates/agent/src/soul_examples.rs:31`] + 6 typed exports
    (`SOUL_PROMPT`, `SOUL_BASELINE_HASH`, `OCEAN_DEFAULT_TRAITS`,
    `SOUL_EXAMPLES`, `parse_soul_examples`, `SoulExample` interface).
  - `src/types/raw.d.ts` (NEW) — global `declare module "*?raw"`
    ambient declaration (resolved by tsconfig `src/**/*.ts` include).
  - `next.config.mjs` (EDIT) — added webpack rule
    `{ test: /\.md$/, type: "asset/source" }` to enable `?raw` for
    `.md` files.
  - `src/app/chat/page.tsx` (EDIT) — replaced 8-line `const SAVANT_SOUL`
    with `import { SOUL_PROMPT } from "@/lib/soul";` (the const was
    a stop-gap that this FID retired).
  - `src/app/manifest/page.tsx` (REWRITE) — 3 cards wired to
    `@/lib/soul`: **Identity & Vibe** (renders SOUL_PROMPT body),
    **Evolution State** (OCEAN traits + SOUL_BASELINE_HASH),
    **Operating Directives** (file map). Cards map 1:1 to
    `ContextAssembler.assemble_system_prompt` sections 1-3 of the
    persona layer.
  - **NOT pre-scaffolded** (per savant-orig: `USER.md`/`IDENTITY.md`/
    `LEARNINGS.jsonl` are not auto-written blank; `LEARNINGS.md`/
    `LEARNINGS.jsonl` are runtime writes per `learning/parser.rs:40-42,
70-73`).
  - **AUDIT (all 4 gates PASS):** `npx tsc --noEmit` exit 0 (no
    errors, TS2664 `?raw` augmentation gone); `npm run build` success
    (Next.js 15.5.19, 16/16 static pages generated, 2/2 exported, route
    `/manifest` 1.78 kB, `/chat` 3.57 kB); `npx prettier --check`
    clean on the 3 FID-006 v2 files (the 22 pre-existing repo files
    flagged are NOT in this FID's scope); FID-151 grep gate
    `grep -rn 'from "@/lib/soul"' src/` returns exactly 2 consumers
    (chat + manifest), 6 exports from `@/lib/soul`. `code-reviewer-minimax-m3`:
    APPROVE (all 6 criteria PASS).
  - Cross-ref: [`dev/fids/archive/FID-2026-07-13-006-v2-soul-manifest-and-workspace-scaffold.md`].
  - **Superseded by FID-006 v3 (2026-07-13):** the 3-card _viewer_ was
    a drift — the actual feature is a _builder_ with form input. v3
    replaces the manifest page with a 4-section builder (input box +
    status panel + BulkManifest + Active Baseline). **The v2
    file-resident persona infrastructure is kept unchanged:**
    `src/lib/soul.ts` (build-time `?raw` re-export), `src/types/raw.d.ts`
    (global `?raw` ambient), `next.config.mjs` webpack `asset/source`
    rule, `workspace-savant/{SOUL,AGENTS,LEARNINGS}.md` + `EVOLUTION.jsonl`
    - `skills/`, and `src/app/chat/page.tsx`'s `import { SOUL_PROMPT }
from "@/lib/soul"` (replacing the const-string stop-gap). Only the
      `/manifest` viewer is replaced. See
      [`dev/fids/FID-2026-07-13-006-v3-soul-builder.md`].

- **Soul Builder feature (FID-006 v3):** interactive authoring flow
  mirroring savant-orig's `ControlFrame::SoulManifest` +
  `ControlFrame::BulkManifest` + `generate_template_soul` chain
  ([`crates/core/src/types/mod.rs:75-86, 189-194, 1153-1183`],
  [`crates/gateway/src/handlers/mod.rs:645-665, 2031`]). Supersedes v2's
  read-only 3-card viewer with a **4-section builder**:
  - `src/types/control-frames.ts` (NEW) — TypeScript mirrors of
    `BootstrapTier` (4 variants, snake_case), `BootstrapStatus`
    (3 variants, `Ready`/`Pending`/`Degraded`, snake_case with
    `#[default] Ready`), `AgentManifestPlan` (3 fields:
    `name`/`soul`/`identity?`), `SoulManifestPayload`,
    `BulkManifestPayload`, `BulkManifestResult`. All names are exact
    pasteback mirrors — no UX rename.
  - `src/lib/manifest-mock.ts` (NEW) — `generate_template_soul(prompt,
name, birthDate, _tier?)` pure function mirror of
    `handlers/mod.rs:2031`; Phase 1 produces a faithful structural
    subset (header + sections 1-3: Systemic Core & Origin,
    Psychological Matrix AIEOS, Architectural Lineage) + a marker for
    sections 4-18. `today_iso()` helper for `birth_date`.
  - `src/lib/mock-ipc.ts` (EDIT) — two new cases: `manifest_soul`
    (returns the templated string + persists to module-scoped
    `builtSouls` map, cap 20) and `bulk_manifest` (enforces SEC #8
    limit of 10 agents per `handlers/mod.rs:647-655`; returns
    `{ status: "REJECTED", reason: "SEC_8_LIMIT_EXCEEDED" }` on
    overflow).
  - `src/lib/ipc.ts` (EDIT) — two new typed wrappers
    `manifestSoul(payload: SoulManifestPayload): Promise<string>` +
    `bulkManifest(payload: BulkManifestPayload): Promise<BulkManifestResult>`.
  - `src/app/manifest/page.tsx` (REWRITE) — 4 sections in vertical
    order: **(1) Soul Manifestation Engine** (builder form: name
    text input + prompt textarea + BootstrapTier select with
    4 options + Manifest Soul submit button + status badge
    Pending→Ready with 800ms setTimeout for the simulated async
    gateway round-trip); **(2) Draft Buffer** (reads
    `localStorage["LS_SOUL_BUILT"]` with cross-tab `storage` event
    sync, shows the most recent built soul with timestamp + tier +
    body preview, Copy to Clipboard + Revert buttons); **(3) Swarm
    Deployment** (JSON textarea for `AgentManifestPlan[]` + Deploy
    Swarm button + success metric display); **(4) Active Baseline**
    (the v2 3 cards: Identity & Vibe / Evolution State / Operating
    Directives wired to `@/lib/soul` — the read-only canonical
    baseline).
  - **Persistence strategy:** `Option A` — `localStorage["LS_SOUL_BUILT"]`
    for the builder's output; build-time `?raw` `workspace-savant/SOUL.md`
    remains the chat baseline (LESSON-018 invariant preserved). The
    browser cannot write to `workspace-savant/SOUL.md` in Phase 1
    (no Rust daemon; Next.js 15 `output: "export"` is a static bundle).
    The chat's `src/app/chat/page.tsx` is unchanged — its
    `SOUL_PROMPT` continues to read the build-time canonical persona.
  - **AUDIT (gated on Spencer ratify):** tsc + build (output:export)
    - prettier + grep gates (FID-151 still 2 consumers +
      `invoke("manifest_soul"` + `invoke("bulk_manifest"` callers)
    - `code-reviewer-minimax-m3` APPROVE. See FID body §Verification
    - §Perfection Loop for gate definitions.
  - Cross-ref: [`dev/fids/FID-2026-07-13-006-v3-soul-builder.md`].

### Fixed

- **Manifest page runtime TypeError: `Cannot read properties of undefined (reading 'lines')`.**
  The `LS_SOUL_BUILT` localStorage entries written BEFORE the FID-006 v3
  reopen don't have the `metrics` field (which was added in the v3
  reopen). When the page hydrates from localStorage, the AAA_RATING
  badge tries to access `builtSoul.metrics.lines` and crashes the
  entire page. The fix: migrate stale entries on read in the
  `useEffect` hydration (`status` defaults to `"template"`,
  `metrics` defaults to `{ lines: 0, sections: 0, depth_score: 0.5 }`)
  - add defensive optional chaining (`builtSoul.metrics?.lines ?? 0`)
    on all 3 AAA_RATING reads (className, title, percentage). Affects 1
    file: [`src/app/manifest/page.tsx`]. No data loss — the user's
    pre-reopen soul body is preserved; only the missing fields are
    filled with the legacy template defaults.
- **Manifest page Copy button: silent failure in non-secure contexts.** The
  previous implementation used only `navigator.clipboard.writeText()`,
  which fails silently in the Tauri webview (no clipboard plugin
  enabled), in unfocused documents, and in non-HTTPS contexts. The
  fix tries the modern Clipboard API first, then falls back to a
  hidden `<textarea>` + `document.execCommand("copy")` strategy that
  works universally. Also adds a 2-second "Copied!" success indicator
  (icon `fa-copy` → `fa-check`, text → "Copied!", border/bg → success
  color) so the user has visible confirmation. Affects 1 file:
  [`src/app/manifest/page.tsx`]. The `onCopy` callback now tries both
  strategies and only surfaces the error state when both fail.
- **HTTP-Referer header bug** [src-tauri/src/inference/openrouter.rs:79]:
  the `HTTP-Referer` was hardcoded to `https://github.com/fame0528/Savant`
  (boilerplate-era leftover from when the project booted off the
  savant-protocol upstream). Should point at the actual project repo
  `https://github.com/savant0x/Savant` per OpenRouter's API guidance
  ("HTTP-Referer should identify the calling application"). The fix is
  correctness + brand identity, not a security boundary.

### Changed

- **Boilerplate reference cleanup:** 4 in-repo source-file `fame0528` /
  `savant-protocol` references cleaned up:
  - [`MIGRATION.md:11`] example `git clone` URL updated
    `fame0528/savant-protocol` → `savant0x/Savant` (the project we
    ship from today).
  - [`scripts/release.py:211`] GitHub API `User-Agent` updated
    `savant-protocol-release-script` → `savant-release-script` (script
    identity accuracy).
  - [`scripts/sync-agents.py:16-17`] docstring
    `savant-protocol/ source` → `Savant project's source` (project
    identity wording).
  - [`scripts/sync-agents.py:239`] `sync.yaml` default header
    `# savant-protocol sync targets` → `# Savant project sync targets`
    (project identity wording).
- `CHANGELOG.md:66` (v0.0.2 entry) and `scripts/release.py:33` (docstring
  NOTE section) are **kept as historical documentation** of the
  boilerplate→project separation discipline correction. Trimming would
  lose the audit trail.
- **Coding standards: `max_file_lines` TS override bumped 400 → 1,000.** Per
  Spencer's ratification on 2026-07-13 ("bump the limit from 400 to 1,000 -
  because there are other files like page.tsx that also break 400 lines. I
  think 1,000 is healthy as long as we continue to build utility first").
  The bump aligns with the ECHO "build utility first" philosophy: page-level
  components with form state + multiple sections + IPC + persistence are
  legitimately utility-dense and 1,000 is the healthy ceiling. The v3
  `/manifest` page at ~470 lines (FID-006 v3) and the 747-line
  `src/app/settings/page.tsx` (FID-0003) are now both under the new ceiling.
  FID-0007 (`manifest-page-split`) is superseded.
  [`coding-standards/typescript.md`].
- **Input border styling: explicit white/black per theme.** All form inputs
  (text inputs, textareas, selects) across `/chat`, `/manifest`, and
  `/settings` now have a theme-aware slight border: `rgba(255, 255, 255, 0.4)`
  in dark mode, `rgba(0, 0, 0, 0.4)` in light mode. Implemented via a new
  `--input-border-color` CSS variable in `globals.css` (per-theme) +
  `border-[color:var(--input-border-color)]` Tailwind arbitrary value on
  each input. Replaces the previous `border border-default/40` (HeroUI
  semantic auto-adapting) and `border border-white/20` (settings dark-only)
  patterns. Per Spencer's directive on 2026-07-13 ("all input boxes need to
  have a slight border, white for dark pages, black for light pages").
  Affects 8 form elements: chat textarea (1) + manifest name input + tier
  select + prompt textarea + bulk-JSON textarea (4) + settings master-key
  input + 2 provider selects (3). [`src/app/globals.css`,
  `src/app/chat/page.tsx`, `src/app/manifest/page.tsx`,
  `src/app/settings/page.tsx`].
- **FID-006 v3 v2 reopen — prompt-driven identity + UI redesign.** Per
  Spencer's 8-point feedback on the v1 LLM-driven implementation
  (tested with "a hustler by nature, with the ability to turn $0
  into $100" — got 90% filler content that ignored the prompt, plus
  the char counter was in the wrong section, Section 4 was showing
  dummy data, the die icon for name generation was missing, only one
  rating box existed, the tier dropdown had a white background, the
  Draft Buffer design was "terrible", and Section 3 was wasting
  space). The fix: the system prompt in
  [`src/lib/soul-generation-system-prompt.ts`] now leads with a
  "CRITICAL DIRECTIVE: PROMPT-DRIVEN IDENTITY" section that BANS
  generic AAA/foundation/sovereign/WAL/CCT language and forces every
  section to be UNIQUE to the prompt's domain (with "hustler"
  examples throughout as the canonical anti-pattern). The manifest
  page is now 3 sections: **(1)** Soul Builder form (with new
  `fa-dice` die icon that generates random names from a 20-name
  pool: Prometheus, Athena, Nova, Orion, etc.; tier `<select>`
  given `colorScheme: "dark"` + `bg-[color:var(--surface)]` to fix
  the white dropdown); **(2)** Draft Buffer (enterprise-grade
  metadata grid: 4 cards NAME/TIER/STATUS/GENERATED + rating grid:
  5 cards LINES/SECTIONS/DEPTH/HASH/INFRA + character counter
  moved here from Section 1 + soul body + Copy + Revert actions);
  **(3)** Swarm Deployment (now a `<details>` element, collapsed
  by default to save space). **Section 4 removed** — was showing
  the chat's canonical persona ("Savant") instead of the just-built
  soul, which Spencer correctly called "CLEARLY not wired". The
  canonical persona is shown in the chat page header, not needed
  here. A new reusable `RatingBox` component at
  [`src/components/rating-box.tsx`] (label + value + optional 0-1
  bar + color variant) per ECHO Law 13 utility-first. 4 file
  changes: [`src/lib/soul-generation-system-prompt.ts`] (rewrite) +
  [`src/components/rating-box.tsx`] (NEW) +
  [`src/app/manifest/page.tsx`] (rewrite) + this entry.
- **LLM-driven soul manifestation (FID-006 v3 reopened).** Per Spencer's
  test rejection on 2026-07-13 ("missing the mark by A LOT" — tested
  with prompt "a proactive hustler, with the ability to turn $0 into
  $100 with sheer will." and got a generic INTJ/architect template that
  ignored the hustler intent and only had 3 of 18 sections), the
  static LLM-less template was replaced with a real OpenRouter
  `POST /v1/chat/completions` call using the 18-section AAA Master
  Framework Prompt from
  [`crates/gateway/src/handlers/mod.rs:1751-1831`]. LLM params match
  savant-orig: `max_tokens: 16384`, `temperature: 0.78`, headers
  `HTTP-Referer: https://github.com/Savant-AI/Savant` +
  `X-Title: Savant Soul Manifestation Engine`. Tier-specific YAML
  grounding (PureGeneration skips; Grounded/Scaffolded/Aspirational
  inject a static `ACTUAL_SYSTEM_STATE` block — the renderer has no
  `gather_system_context` in Phase 1) per [`mod.rs:1834-1852`].
  Metrics calculation (`lines`, `sections`, `depth_score`) ports
  `calculate_semantic_depth` from [`mod.rs:2014-2026`]. The static
  18-section template fallback (for no-key mode) is also expanded to
  all 18 sections (was 1-3 + marker) so the offline path matches the
  online path's structural shape. `soul_blake3` uses SHA-256 in browser
  (BLAKE3 not in SubtleCrypto; field name kept for IPC contract
  parity). New UI: character counter below the prompt textarea,
  `AAA_RATING` badge in the Draft Buffer (depth_score percentage with
  status-color coded for complete/template/error),
  `MANIFESTATION SEQUENCE IN PROGRESS…` submit button text. Without an
  OpenRouter master key in `mockMasters["openrouter"]`, the mock
  returns the static template with `status: "template"` and a
  `note: "..."`. 6 file changes:
  [`src/lib/soul-generation-system-prompt.ts`] (NEW) +
  [`src/lib/manifest-mock.ts`] (REWRITE — added `generateSoul()` +
  expanded `generateTemplateSoul()` to all 18 sections) +
  [`src/lib/mock-ipc.ts`] + [`src/lib/ipc.ts`] +
  [`src/types/control-frames.ts`] (added `ManifestResult` interface +
  `model?` field on `SoulManifestPayload`) +
  [`src/app/manifest/page.tsx`].

### Removed

- **Drift-cleanup (FID-2026-07-12-004-system, FID-004r2, FID-005-feature, FID-005r2):** all 4 active FIDs created during the 2026-07-12/13 drift session (paraphrased citations of savant-orig filesystem evidence without re-verification pasteback) archived to `dev/fids/archive/` with `[DRIFT-REJECTED]` filename marker. The corrupt duplicate of `FID-004-system` in archive (64-line stub) was overwritten with the full 364-line active version; the hallucinated `workspace-savant/{README.md, manifest.toml, soul/savant.soul.yaml, evolution.log}` layout proposed in FID-004 r0 is explicitly **not implemented**. `workspace-savant/SOUL.md` remains on disk as a real artifact, but its previously-claimed provenance (line citations into `crates/agent/src/manager.rs:32`, `learning/parser.rs:25,42,26,69-77`, `soul_examples.rs:3`, `context.rs`, `workspace_guard.rs:6-50`) is unverified pending a freshly authored FID grounded in actually-read savant-orig evidence per the LESSON-016 Draft-and-Prove Rule captured in [`dev/LEARNINGS.md`]. The soul-manifest feature will be authored A-to-Z in a fresh FID with verified-line pastebacks only — not an `FID-005r3` resurrection.

---

### Added (FID-009 — Perfection Loop, 2026-07-13)

- **Test suite, 5 new files, 68/68 passing** [src/lib/name-generator.test.ts,
  src/lib/prompt-generator.test.ts, src/lib/manifest-mock.test.ts,
  src/lib/swarm-diff.test.ts, src/components/soul-body-viewer.test.tsx]:
  full unit test coverage for the pure functions across FID-006 v3 /
  007 / 009 / 010 / 012 / 013 / 014. Coverage map: name-generator
  (5 themes, no-repeat lastPick) + prompt-generator (20 domains,
  no-repeat lastPromptPick) + manifest-mock (computeSectionMetrics
  edge cases + calculateSemanticDepth variance-based 0.5-lock test
  - generateTemplateSoul 18-section shape + parseSSEStream TCP
    fragmentation + [DONE] sentinel + malformed chunks + abort) +
    swarm-diff (computeSwarmHashSync stability + previewSwarmDiff
    4-bucket classification) + soul-body-viewer (h2/h3 + **bold** +
    `code` + lists + tables + blockquotes + hr). FID-014
    (regenerate button) intentionally has no unit tests — the
    `regenPending` useEffect + capture-then-reset pattern lives in
    page.tsx and would require React Testing Library; verified
    through 5 code-reviewer passes + manual smoke test.
- **vitest setup file** [vitest.setup.ts]: polyfills
  `globalThis.crypto` from `node:crypto` `webcrypto` if
  `crypto.subtle` is missing (happy-dom's native implementation
  is incomplete). Required by `calculateSoulHash` and
  `computeSwarmHash` SHA-256 calls. Idempotent — only installs if
  `crypto.subtle` is missing. Also documents the FID-015 follow-up
  (`// TODO(fid-015): polyfill localStorage`) so the next run isn't
  silently green-with-one-excluded for the pre-existing
  `src/lib/ipc.test.ts` failure.
- **`esbuild.jsx: "automatic"`** in [vitest.config.ts]: matches the
  production Next.js build's JSX transform so `.tsx` source files
  (which don't `import React`) work in the test runner without
  modification. Avoids polluting source files with dead `import
React` lines.
- **`parseSSEStream` exported for testability** [src/lib/manifest-mock.ts]:
  was a module-private function; now exported so the test file can
  exercise the TCP-fragmentation path. No behavior change for
  production callers.
- **Nested-list depth field** [src/components/soul-body-viewer.tsx]:
  `parseList()` now returns a `depth` field for nested list items
  (0 = top-level, 1 = 2-space indent, 2 = 4-space indent) so the
  test can assert the parser handles GitHub-style nested lists.
  The renderer continues to use 0/1 (no 3-level deep lists in the
  canonical SOUL.md).
- **Markdown parser hardening** [src/components/soul-body-viewer.tsx]:
  documents that an unclosed `**` (e.g., `**foo` without a
  matching `**foo**`) is rendered as plain text per the GitHub
  Flavored Markdown spec, not as a parser error. Added the
  `renderInline` helper to keep the `**bold**` + `` `code` ``
  parsing logic in one place (was duplicated across the table +
  paragraph + list-item renderers).
- **HMR reset documented** [src/app/manifest/page.tsx]: the
  `lastPick` and `lastPromptPick` module-scoped variables may
  reset to `null` on hot module replacement. This is intentional
  and does NOT persist to sessionStorage (would leak across
  sessions; out of scope for FID-009). The doc comment makes this
  explicit for future maintainers.

### Added (FID-015 — happy-dom localStorage polyfill, 2026-07-13)

- **localStorage polyfill implemented** [vitest.setup.ts]:
  Map-backed shim matching the `Storage` interface (getItem /
  setItem / removeItem / clear / key / length) now satisfies
  `src/lib/mock-ipc.ts:129:35` `hydrateMasters` and all 5
  pre-existing `src/lib/ipc.test.ts` cases. The polyfill guard
  checks `typeof getItem !== "function"` (catches both undefined
  and broken-object cases — happy-dom in some configurations
  provides a localStorage OBJECT that exists but lacks methods).
  Installed via `Object.defineProperty` to bypass non-writable
  property protection; set on both `globalThis` and `window`
  when they differ. Includes a `beforeEach` hook
  (`globalThis.localStorage?.clear()`) for state isolation
  between tests. The `--exclude='**/ipc.test.ts'` flag is no
  longer needed — the test suite is now **actually** green at
  73/73 (was 68/68 with one test silently excluded). See
  [`dev/fids/archive/FID-2026-07-13-015-happy-dom-localstorage.md`].

---

## v0.0.2 — 2026-07-12

Patch bump. FID-0003 (auto-derived session key) shipped end-to-end —
chat HTTP 401 resolved via two-tier credential architecture. Settings +
chat rewrites per the orig Savant `OpenRouterMgmt::create_key` ref.
Test framework installed (vitest + Playwright + happy-dom). FID
lifecycle housekeeping pass codified with release-only-versioning
discipline.

### Added

- **FID Auto-Archive (FID-0001, FID-0002):** both Phase 1 FIDs moved
  from `dev/fids/` → `dev/fids/archive/` per ECHO §FID Auto-Archive
  (FID-0001 `ui-first-phase`, FID-0002 `initial-release`). FID-0002's
  effectful ops (git remote + `gh repo create` + `git push -u origin
main` + initial tag) were retroactively reconciled — see FID bodies.
- **`.savant` agent home pointer:** stale `Savant-backup` → current
  `C:\Users\spenc\dev\Savant\` (mirrors `.vera` pattern).
- **`dev/LEARNINGS.md` session entry:** "Housekeeping Pass + FID-0003
  Loop 0 Doc Convergence Note" — codifies status-name hygiene, mock
  IPC realness, tier-invariance capture, pre-impl probe gates, and
  the release-only-versioning discipline (post speculative 0.0.1 →
  0.1.4 rollback on 2026-07-12).
- **`dev/session-summaries/2026-07-12-housekeeping-pass.md`:**
  multi-iteration FID-0003 Loop 0 audit trail.
- **FID-0003 (`auto-derived-session-key`) — IMPLEMENTATION SHIPPED:**
  8 file changes + 5 vitest tests + 2 Playwright round-trip tests
  per §Steps + §Quality Setup of the FID body. Converged through 10
  iters of Loop 0 (FID-doc) + 3 turns of Loop 1 (RED → GREEN → AUDIT
  on code). Status: `verified` → **`closed`** at this release cut +
  auto-archived to `dev/fids/archive/0003-auto-derived-session-key.md`
  per ECHO §FID Auto-Archive.
- **`scripts/release.py` boilerplate fix:** `REPO_SLUG` default
  boilerplate artifact `fame0528/savant-protocol` → **`savant0x/Savant`**
  per project separation (boilerplate→Savant cutover); `--repo` flag
  added for fork workflows. `scripts/sync-agents.py` docstring
  banner aligned to project source path. Both scripts documented.
- **`scripts/` enhancements:** both Python scripts' docstrings + module
  comments updated. `scripts/release.py` now exports `--repo` for
  repo-agnostic GitHub Release API calls (default `savant0x/Savant`).
- **Test framework installed (devDependencies):** `vitest@^2.1.0` +
  `@playwright/test@^1.49.0` + `happy-dom@^15.11.0`. New npm scripts
  `test` / `test:watch` / `test:e2e` / `test:all`. Config files:
  `vitest.config.ts` (happy-dom env, src/**/_.test._, @/* alias) +
  `playwright.config.ts` (testDir ./e2e, chromium, webServer npm run
  dev, baseURL :3000).
- **New source files:** `src/lib/ids.ts` (`randomHex(n)` utility via
  `crypto.getRandomValues` for OpenRouter `agent_name` uniqueness);
  `src/lib/hooks/use-derived-rotation.ts` (NEW useDerivedRotation hook
  per OQ-4 daily-cron — mount-time scan + minute-tick interval); `src/lib/ipc.test.ts`
  (5 vitest cases — 4 provisionSessionKey parser + 1 clearSessionKey
  hash regression); `e2e/auto-derived.spec.ts` (2 Playwright round-trip
  tests, env-gated on `SAVANT_TEST_MASTER`).
- **Source file modifications:** `src/lib/ipc.ts` (+ `SessionKey` type
  - `normalizeProvisionResponse` + `provisionSessionKey` +
    `clearSessionKey` IPC bridge); `src/lib/mock-ipc.ts` (real
    `provision_session_key` POST `/v1/keys` + `clear_session_key`
    DELETE `/v1/keys/{hash}` cases; module-scoped `mockMasters` for
    cross-call continuity); `src/lib/hooks/use-loaded-config.ts`
    (`LS_DERIVED` constant + `parseDerivedSession` reader); `src/app/settings/page.tsx`
    (full rewrite — dual-stage handleSaveKey + Session Key card a11y +
    Rotate + Disconnect + cross-tab `storage` listener; **deviation:
    747 lines, +87% over TS override `max_file_lines=400`** — tracked
    as FID-0004 follow-on split); `src/app/chat/page.tsx` (full rewrite
    — reads `LS_DERIVED` with try/catch wrap, blocking `<dialog>` modal
    per OQ-3, `Authorization: Bearer ${derived.key}` swap, Retry
    handler).
- **Documentation polish (Law 11):** Prettier formatting cleaned on
  CHANGELOG.md, ECHO.md, MIGRATION.md, next-env.d.ts,
  protocol.config.yaml, README.md.

### Fixed

- **FID-2026-0711-003 (`auto-derived-session-key`):** chat HTTP 401
  `User not found` resolved. Master key no longer held in browser
  `localStorage` (eliminated single-tier collapse that the orig
  Savant two-tier architecture prevented). Chat reads derived
  `SessionKey` from `LS_DERIVED` with try/catch `JSON.parse` wrapping +
  blocking `<dialog>` modal on empty/invalid state. OpenRouter
  `/v1/keys` provisioned per Save Master Key with
  `agent_name="savant-${randomHex(8)}"`. Reference impl in
  `C:\Users\spenc\dev\Savant-backup\crates\agent\src\providers\mgmt.rs`
  (`OpenRouterMgmt::create_key`).

### Changed

- **Version-rocking discipline codified:** `package.json` is canonical
  source of truth; meta-files never bump speculatively mid-development;
  work-in-progress lives under `## [Unreleased]` in `CHANGELOG.md`
  awaiting next-release tagging. The previous speculative
  project-version bump (`0.0.1 → 0.1.4` across 4 anchors) was
  reverted on 2026-07-12; meta-files now match `package.json=0.0.2`
  (single patch-digit bump per release-only discipline).
- **`scripts/release.py` behavior:** keeps `git` push (uses local
  remote `origin`), `--repo` override applies ONLY to the GitHub
  Release API call (Release notes / fetch-existing-release checks).

## v0.0.1 — 2026-07-11

First release of Savant — the proactive AI agent desktop shell, built on
Tauri 2 + React 19 + HeroUI v3 alpha. Phase 1 of a multi-phase build.

### Added

- **Tauri 2 + Next.js 15 + React 19 + HeroUI v3 alpha scaffolding** — desktop shell, renderer (App Router + static export), IPC command surface.
- **`src-tauri/src/security/master_key.rs`** — generalized multi-profile `Vault` (5-strategy cascade: env vars → cwd `.env` → exe `.env` → JSON vault file → UI prompt).
- **Vault storage paths:** Windows `%APPDATA%/savant/auth.json`; Unix `~/.config/savant/auth.json`. Unix perms enforced 0o600 (Windows DPAPI deferred to Phase 5).
- **`src-tauri/src/inference/openrouter.rs`** — reqwest-based chat-completions client; reads `openrouter-default` profile from vault; returns `Result<String, InferenceError>`.
- **Three Tauri IPC commands:** `setup_master_key`, `infer_openrouter`, `vault_list_profiles`.
- **HeroUI v3 alpha integration** — CSS-first, no Provider wrapper required, no Tailwind plugin required. Interactive components use `'use client'` directives for Next.js App Router compatibility.
- **Smoke-test screen** — first end-to-end test of the build pipeline; proves UI → IPC → daemon → OpenRouter → response → UI round-trip.
- **FID-2026-07-11-001** — Phase 1 FID (`dev/fids/0001-ui-first-phase.md`).
- **FID-2026-07-11-002** — initial release FID (`dev/fids/0002-initial-release.md`).

### Changed

- **`protocol.config.yaml`** — `language: "CHANGE_ME"` → `"rust"` (ECHO HALT corner cleared). Commands rewritten for Tauri + npm workflow. Project name `savant-core`; project version 0.0.1.
- **Build phase order** — UI-first (was: cognitive-core-first). Phase 1 = Tauri scaffolding; Trigger Bus → Hybrid Tick deferred to Phase 2.
- **`.gitignore`** — Tauri/Rust/Vite/SQLite/vault-secret patterns added; `resources/` and `prompts/` excluded from repo.
- **`coding-standards/release-workflow.md`** — version-bumping section rewritten to enforce "10 patch releases per minor number" (no feature-flag minor bumps).

### Toolchain confirmed (Windows 11 dev box)

- `rustc 1.94.0` + `cargo 1.94.0` + `tauri-cli 2.10.1`
- `node v25.2.1` + `npm 11.13.0` + `next ^15.0.0` (App Router, static export)
- HeroUI v3 alpha caveat: pin via lockfile; alpha API may shift.

### Reference (non-code)

- **hermes-rs `OAUTH_DESIGN.md`**: not a Tauri fork but its auth profile + provider-coverage schema informed the generalized Vault design.
- **HeroUI LLMS at `docs/full-llms.txt`**: 162,699 lines. v3 alpha line 527/531/727 explicitly state CSS-first architecture (no Provider + no Tailwind plugin).

<!-- Agent entries are added below this line -->
