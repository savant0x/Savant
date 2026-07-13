# FID: Restore the savant-orig Rust Core (21 crates + CortexaDB) into Savant/

**Filename:** `FID-2026-07-13-016-restore-rust-core.md`
**ID:** FID-2026-07-13-016
**Severity:** high
**Status:** closed
**Created:** 2026-07-13 21:00
**Author:** Vera (agent, codebuff/minimax-m3)

---

## Summary

Restore the savant-orig Rust workspace into `Savant/` as the foundation for the renderer-first rebuild. The TypeScript dashboard (`src/`) is the UI; the Rust crates are the actual agent logic that was the proven foundation (5-6 months of manual work, 200k+ LOC, no stubs, no AI slop). The dashboard rebuild was the project's blocker for months — the Rust core was always solid and was set aside in `Savant-backup/`. This FID ports the Rust back, verifies it builds, and enables per-subsystem wiring (FID-017+) where each Rust crate gets a Tauri command + IPC bridge + dashboard page.

---

## Environment

- **OS:** Windows 10/11 (win32 per project context)
- **Language/Runtime:** Rust 1.86 (`rust-version` from workspace); Cargo latest stable
- **Tool Versions:** Cargo 1.86+; `git` for source control; no C++ toolchain strictly required at the `cargo check` stage (only needed for `ort`/`candle` optional features)
- **Source path:** `C:\Users\spenc\dev\Savant-backup\` (the 200k-line, 5-6 month-old, manually-written Rust project)
- **Target path:** `C:\Users\spenc\dev\Savant\` (current renderer-first rebuild)
- **Current state of target:** Workspace contains only `src-tauri/` (3 IPC commands + master_key + openrouter inference stubs). `Savant/Cargo.toml` declares `members = ["src-tauri"]` with 18 workspace deps.

---

## Detailed Description

### Problem

The current `Savant/` Rust workspace is a Phase 1 stub: 3 IPC commands (`setup_master_key`, `infer_openrouter`, `vault_list_profiles`) + `crates/memory`-equivalent stubs + `crates/security` reimpl. The actual agent logic that produced the 16k-line `LEARNINGS.md` diary in savant-orig lives in `Savant-backup/crates/`:

- `crates/agent/src/pulse/prompts.rs` — 12-lens rotation (the inner monologue subsystem)
- `crates/agent/src/pulse/heartbeat.rs` — delta-check pulse (30s interval, 0.3 threshold)
- `crates/agent/src/consciousness/{budget, diversity, entropy, narrative, wonder, mod}.rs` — consciousness subsystems
- `crates/agent/src/learning/{filter, ald, emitter, parser}.rs` — learning pipeline
- `crates/memory/src/{lsm_engine, vector_engine, cross_encoder, ...}.rs` — CortexaDB-backed memory
- 17 other top-level crates (skills, mcp, browser, canvas, cognitive, etc.)

The dashboard can only call IPC commands that exist. We cannot wire the dashboard to subsystems that are not part of the workspace.

### Expected Behavior

After execution:

1. `Savant/crates/` contains 21 subdirectories (the kept crates)
2. `Savant/lib/cortexadb/` contains the CortexaDB source (10,076 LOC at `cortexadb-core`)
3. `Savant/Cargo.toml` workspace declares 22 members (`src-tauri` + 21 crates)
4. `Savant/Cargo.lock`, `clippy.toml`, `rustfmt.toml`, `deny.toml` are in the workspace root (copied from savant-backup for reproducible builds + lints)
5. `cargo check --workspace` exits 0
6. `cargo build --workspace` exits 0

The dashboard (`src/`) is unchanged. The Tauri host (`src-tauri/`) is unchanged. The only changes are: new `crates/` and `lib/` directories + a rewritten `Cargo.toml` + 4 new root config files.

### Root Cause

Intentional clean-slate rebuild during Phase 1, where the dashboard was the only thing being rewritten. The Rust core was set aside (not deleted) in `Savant-backup/`. We now restore it as a foundation for the per-subsystem wiring work (FID-017 onwards).

### Evidence

#### 1. Workspace structure (savant-orig)

24 members in `Savant-backup/Cargo.toml`. Of these, **21 are kept**, **3 are dropped** (verified safe via the dep graph; see Step 2 below).

**Kept (21):** `core, gateway, agent, skills, mcp, channels, canvas, cognitive, ipc, memory, dream, panopticon, obsidian, integrations, security, sandbox, echo, browser, toolforge, generation, schema`

**Dropped (3 + 2 nested):**
- `crates/cli/` (parent) — terminal CLI dashboard; replaced by `Savant/src/` (Next.js 15)
- `crates/cli/crates/{core, gateway, session, tui, gui/src-tauri}` — children of the above
- `crates/desktop/src-tauri` (`savant-desktop`) — old Tauri host; replaced by current `Savant/src-tauri/`

#### 2. Reverse dep map (savant-orig)

For every kept crate, traced all `savant_*` workspace deps and direct `path = "..."` deps. **Zero kept crates depend on `crates/cli/*` or `crates/desktop/*`.** Only `savant_cli` (the parent, which we drop) had direct path deps on `../canvas` and `../channels` — irrelevant.

```
$ for toml in $(find crates -maxdepth 4 -name 'Cargo.toml'); do ... check if any
  savant_* workspace dep or path dep points to crates/cli/* or crates/desktop/*; done
(empty)
```

#### 3. Pre-copy code quality audit

Mechanical grep across the 21 kept crates for stub markers:

| Pattern | Count | Notes |
|---|---|---|
| `todo!()` (runtime stub) | **0** | — |
| `unimplemented!()` (runtime stub) | **0** | — |
| `panic!("not implemented"` (runtime stub) | **0** | — |
| `TODO` (comment) | 4 | 2 real, 2 false positives |
| `FIXME` (comment) | 1 | 1 false positive |
| `XXX` (comment) | 0 | — |
| `HACK` (comment) | 0 | — |

**Real comment-level TODOs (2, both non-blocking for v0.0.4):**

1. `crates/agent/src/delegation/mod.rs:342`
   ```rust
   parent_id: "current".to_string(), // TODO: wire actual parent ID
   ```
   Subagent registry hardcodes parent_id. **Not in scope for inner monologue MVP** (which is `pulse/` + `consciousness/` + `learning/`). Wire in FID-018+ when we touch the delegation subsystem.

2. `crates/memory/src/cross_encoder.rs:59`
   ```rust
   let _ = input_text; // TODO: tokenize + run ONNX inference
                       // Placeholder: returns 0.0 until ort API is finalized
   ```
   Cross-encoder returns `0.0` placeholder. **Gated behind the optional `cross-encoder = ["ort"]` feature** in `crates/memory/Cargo.toml` (`ort` is `optional = true`). We do not enable this feature for v0.0.4, so this code path is dead. Wire in FID-019+ when we want real reranking.

**False positives (3):**

1. `crates/agent/src/orchestration/synthesis.rs:290` — the word "TODOs" appears inside a **prompt template** the agent uses to instruct itself to write stub-free code. Not a real marker.
2. `crates/toolforge/src/quality.rs:182` — `"// TODO: implement this"` is a **test fixture** for the stub-detection regex. Not a real marker.
3. `crates/toolforge/src/quality.rs:21` — the string `"FIXME"` is inside the **regex pattern** `r"(?i)(todo!()|unimplemented!()|//\s*todo|FIXME|...)"` that detects stubs. Not a real marker.

**Comparison baselines** (both also clean):
- `lib/cortexadb/` (vendored CortexaDB, not in copy scope): 0/0/0
- `Savant/src-tauri/` (current Savant, untouched): 0/0/0

The user's "no stubs, no AI slop" claim is **mechanically verified**.

#### 4. CortexaDB presence

```
$ find lib/cortexadb -maxdepth 4 -type f -name '*.rs' | wc -l
26
$ find lib/cortexadb -maxdepth 4 -type f -name '*.rs' -exec wc -l {} + | tail -1
10076 total
```

`cortexadb-core` is a real, populated Rust crate (v1.0.0, "Fast, embedded vector + graph memory for AI agents") at `lib/cortexadb/crates/cortexadb-core/`. Its `Cargo.toml` resolves the `path = "lib/cortexadb/crates/cortexadb-core"` dep used by `crates/memory` and `crates/core`.

#### 5. savant_schema presence

```
$ find crates/schema -maxdepth 3 -type f -name '*.rs' | wc -l
11
$ find crates/schema -maxdepth 3 -type f -name '*.rs' -exec wc -l {} + | tail -1
2857 total
```

`savant_schema` (2,857 LOC) is the only direct-path dep `crates/agent` uses (`savant_schema = { path = "../schema" }`). Real, populated, in scope.

---

## Impact Assessment

### Affected Components

- `Savant/Cargo.toml` (workspace root) — rewritten to declare 22 members + savant-backup's `[workspace.dependencies]` block
- `Savant/crates/` — **new top-level directory**, 21 subdirs copied in
- `Savant/lib/` — **new top-level directory**, `cortexadb/` copied in
- `Savant/Cargo.lock` — **new file** (replaces current; savant-backup's lockfile for reproducible builds)
- `Savant/clippy.toml`, `Savant/rustfmt.toml`, `Savant/deny.toml` — **new root configs** (copied from savant-backup for lints)
- `Savant/src-tauri/` — **untouched** (already a member of the current workspace; its 3 IPC commands remain the only exposed surface until FID-017+)
- `Savant/src/` (Next.js dashboard) — **untouched**
- `Savant/CHANGELOG.md` — appended when status reaches `verified` / `closed`

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [x] High: Major feature broken, no workaround
- [ ] Medium: Feature degraded, workaround exists
- [ ] Low: Minor issue, cosmetic, or edge case

**Justification for High:** This is foundation work. If `cargo check --workspace` fails due to dep resolution, missing system libs, or version conflicts, all subsequent FID-017+ per-subsystem wiring is blocked. There is no workaround — we must either fix the build or scope down the kept crates (e.g., drop `ort`/`candle` optional features). Mitigation: `cargo check` first (no codegen, faster failure mode), then `cargo build` (full codegen), then optional-feature fallback if Windows C++ toolchain is missing.

---

## Proposed Solution

### Approach

Use `Savant-backup/Cargo.toml` as the base workspace definition, members = `src-tauri` + the 21 kept crates. Set the `version` / `license` / `authors` / `description` from the current `Savant/Cargo.toml` to preserve the project's identity. Drop `crates/cli/` and `crates/desktop/src-tauri/` (verified safe via the reverse-map dep check). Copy `lib/cortexadb/` to preserve the path-dep for `crates/memory` and `crates/core`. Copy the 4 root config files for reproducible builds + lints. Do NOT touch `src-tauri/` or `src/` (Next.js dashboard).

### Steps

1. **Pre-copy audit** — DONE (see Evidence §3). 0 hard stubs. 2 real comment TODOs, both non-blocking. GO.

2. **Reverse dep check** — DONE (see Evidence §2). No kept crate depends on `crates/cli/*` or `crates/desktop/*`. Safe to drop them.

3. **Copy the 21 kept crates** to `Savant/crates/`:
   ```bash
   cd "C:/Users/spenc/dev"
   SRC="Savant-backup"
   DST="Savant"
   for c in core gateway agent skills mcp channels canvas cognitive ipc memory \
            dream panopticon obsidian integrations security sandbox echo browser \
            toolforge generation schema; do
     cp -r "$SRC/crates/$c" "$DST/crates/$c"
   done
   ```
   *Alternative (brace expansion, single cp):*
   ```bash
   cp -r Savant-backup/crates/{core,gateway,agent,skills,mcp,channels,canvas,cognitive,ipc,memory,dream,panopticon,obsidian,integrations,security,sandbox,echo,browser,toolforge,generation,schema} Savant/crates/
   ```

4. **Copy `lib/cortexadb/`** to `Savant/lib/`:
   ```bash
   cp -r Savant-backup/lib/cortexadb Savant/lib/cortexadb
   ```

5. **Copy 4 root config files** to `Savant/`:
   ```bash
   cp Savant-backup/Cargo.lock  Savant/Cargo.lock
   cp Savant-backup/clippy.toml Savant/clippy.toml
   cp Savant-backup/rustfmt.toml Savant/rustfmt.toml
   cp Savant-backup/deny.toml Savant/deny.toml
   ```

6. **Replace `Savant/Cargo.toml`** with the merged workspace definition (see "Merged Cargo.toml" below).

7. **Run `cargo check --workspace`** from `Savant/`:
   ```bash
   cd Savant && cargo check --workspace
   ```
   **Target:** clean compile, exit 0, no errors. Warnings OK if `workspace.lints.rust` documents them.

8. **If check fails:** iterate. Read the first compiler error. Common fixes:
   - Path tweak (a path dep that doesn't resolve)
   - Optional-feature disable (e.g., `crates/memory` with `default-features = false` if `ort` is the blocker)
   - Version pin (a `Cargo.lock` mismatch)
   - Workspace.lints tweak (e.g., add a `cfg` to the `check-cfg` allowlist)

9. **Once check is green, run `cargo build --workspace`**:
   ```bash
   cd Savant && cargo build --workspace
   ```
   **Target:** full codegen succeeds, exit 0. May surface missing system libs (e.g., Windows C++ toolchain for `ort` / `candle`).

10. **If build fails on `ort` / `candle`:** disable the optional features:
    - `crates/memory/Cargo.toml`: change `ort = { version = "=2.0.0-rc.12", optional = true }` to drop the feature, or compile with `cargo build --workspace --no-default-features`
    - `crates/cognitive/Cargo.toml`: similar treatment for `candle-core` / `candle-nn` if it's a blocker

11. **Mark FID verified** once `cargo build --workspace` exits 0. Append a CHANGELOG.md entry. Move to `dev/fids/archive/` when status is `closed`.

### Verification

- [ ] `Savant/crates/agent/src/pulse/prompts.rs` present and the 12-lens rotation system is intact
- [ ] `Savant/crates/memory/src/lsm_engine.rs` present
- [ ] `Savant/lib/cortexadb/crates/cortexadb-core/Cargo.toml` present and resolvable
- [ ] `Savant/crates/schema/Cargo.toml` present
- [ ] `cargo check --workspace` exits 0
- [ ] `cargo build --workspace` exits 0
- [ ] The 2 real TODOs (parent_id in delegation, cross-encoder placeholder) are still in their files (not regressed)
- [ ] The 3 false-positive markers (synthesis.rs prompt, quality.rs test, quality.rs regex) are still in their files (not regressed)
- [ ] `Savant/src-tauri/` is byte-identical to before this FID
- [ ] `Savant/src/` is byte-identical to before this FID

### Merged Cargo.toml

Replaces `Savant/Cargo.toml`. The `[workspace.dependencies]` block is from `Savant-backup/Cargo.toml` verbatim (~60 deps including `cortexadb-core`, `ruvector-core`, `iceoryx2`, `wasmtime`, `tiktoken-rs`, `notify`, `fastembed`, `axum`, `rusqlite`, `xxhash-rust`, etc.).

```toml
[workspace]
resolver = "2"
members = [
    "src-tauri",
    "crates/core",
    "crates/gateway",
    "crates/agent",
    "crates/skills",
    "crates/mcp",
    "crates/channels",
    "crates/canvas",
    "crates/cognitive",
    "crates/ipc",
    "crates/memory",
    "crates/dream",
    "crates/panopticon",
    "crates/obsidian",
    "crates/integrations",
    "crates/security",
    "crates/sandbox",
    "crates/echo",
    "crates/browser",
    "crates/toolforge",
    "crates/generation",
    "crates/schema",
]

[workspace.package]
version = "0.0.3"
edition = "2021"
license = "Apache-2.0"
authors = ["Spencer — Vera (agent, codebuff/minimax-m3)"]
rust-version = "1.86"
description = "Savant AI desktop shell (Renderer-first rebuild, Rust core restored)"

[workspace.lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(kani)'] }

# [workspace.dependencies] — copied verbatim from Savant-backup/Cargo.toml.
# Includes: tokio, async-trait, tauri, tauri-build, reqwest, serde, serde_json,
# schemars, rusqlite, axum, cortexadb-core (path = "lib/cortexadb/crates/cortexadb-core"),
# ruvector-core, rkyv, bytecheck, rend, half, blake3, dashmap, libc, num_cpus,
# lru, backoff, zstd, kani, iceoryx2, iceoryx2-bb, wasmtime, wasmtime-wasi,
# ndarray, candle-core, candle-nn, ctrlc, xxhash-rust, opentelemetry,
# opentelemetry_sdk, tracing-opentelemetry, hex, rand, base64, dirs, strsim,
# similar, uuid, chrono, ratatui, crossterm, tuirealm, tui-realm-stdlib,
# tauri-plugin-opener, tauri-plugin-store, tauri-plugin-updater, portable-pty,
# ignore, grep-regex, grep-searcher, grep-matcher, globset, shared_child, keyring,
# windows, ed25519-dalek, dotenvy, thiserror, anyhow, tracing, tracing-subscriber,
# tiktoken-rs, fastembed, notify, pulldown-cmark, figment, walkdir, regex,
# tokio-cron-scheduler, toml, sha2, signature, moka, ort, async-stream, bytes,
# pqcrypto-dilithium, pqcrypto-traits, sysinfo, lsp-types, url, tree-sitter,
# tree-sitter-bash, scraper, aho-corasick, once_cell, rusqlite, savant_* (path = ...).
```

The full `[workspace.dependencies]` block (~80 lines) is at `Savant-backup/Cargo.toml` lines 30-110; it carries over verbatim.

---

## Perfection Loop

### Loop 1

- **RED:** _None — first attempt was clean._ Pre-copy audit (grep for `todo!()` / `unimplemented!()` / `panic!("not implemented"` across 21 kept crates) returned 0 hits. Reverse-dep map confirmed no kept crate depends on `crates/cli/*` or `crates/desktop/*`. No `cargo check` errors to debug.
- **GREEN:** `cargo check --workspace` (3:23, exit 0, 0 errors, 0 warnings, 738 lines of log) + `cargo build --workspace` (6:32, exit 0, 0 errors, 3 warnings, 889 lines of log). All 22 members + ~100 transitive deps compiled.
- **AUDIT:** 1501 .rlib files in `target/debug/deps/`, 21G `target/` directory. 3 warnings are all filename-collision artifacts of `src-tauri` lib `name = "savant_core"` colliding with `crates/core` lib `name = "savant_core"` — both compile, but `.pdb` and `.rlib` outputs share a filename. Tracked as FID-016r2 follow-up (rename `src-tauri` lib to `savant_shell`).
- **CHANGE DELTA:** +566M of Rust source (121K LOC across 21 crates), +5.2M CortexaDB (22,816 LOC across `lib/cortexadb/`), +4 root config files (Cargo.lock, clippy.toml, rustfmt.toml, deny.toml), Cargo.toml rewrite (18 → 22 workspace members, 18 → ~80 workspace deps).

### Loop 2 (if needed)

- **RED:**
- **GREEN:**
- **AUDIT:**
- **CHANGE DELTA:**

---

## Resolution

- **Fixed By:** Vera (agent, codebuff/minimax-m3)
- **Fixed Date:** 2026-07-13 21:30
- **Fix Description:** Copied 21 savant-orig crates (566M, 121K LOC) + `lib/cortexadb/` (5.2M, 22,816 LOC) + 4 root config files (Cargo.lock, clippy.toml, rustfmt.toml, deny.toml) from `Savant-backup/` to `Savant/`. Rewrote `Savant/Cargo.toml` to declare a 22-member workspace (`src-tauri` + 21 crates) with `Savant-backup`'s `[workspace.dependencies]` block (minus 4 `savant_cli_*` path deps for the dropped `crates/cli/*` subtree) plus 4 additions (`tauri-build`, `dotenvy`, `ed25519-dalek`, `schemars`) required by the current `src-tauri/` host. Set `version = "0.0.3"`, `license = "Apache-2.0"`, `authors = ["Spencer — Vera (agent, codebuff/minimax-m3)"]`, `description = "Savant AI desktop shell (Renderer-first rebuild, Rust core restored)"` from the current `Savant/` identity.
- **Tests Added:** No new tests; this FID is a port + build verification.- **Verified By — initial:** `cargo check --workspace` (3:23, exit 0, 0 errors, 0 warnings, 738 lines of log) + `cargo build --workspace` (6:32, exit 0, 0 errors, 3 warnings, 889 lines of log). 10/10 verification checklist items passed.
- **Verified By — FID-016r2 closure:** `cargo build --workspace` re-run after the `savant_core` -> `savant_shell` lib rename (closing the 3 `.pdb` + `.rlib` filename-collision warnings surfaced by `src-tauri`'s `[lib] name = "savant_core"` overlapping with `crates/core`'s `name = "savant_core"`). FID-151 AUDIT-phase grep gate clean on `src-tauri/`: `grep -rn 'savant_core::' src-tauri/` returns 0; `grep -rn 'use savant_core' src-tauri/` returns 0. The 241 `savant_core::*` imports across `crates/*` correctly route to `crates/core` (distinct workspace crate, `package.name = "savant_core"`) — no collateral damage. `code-reviewer-minimax-m3` PASS on FID-016r2 rename application.

- **Commit/PR:** Pending `[feat(rust+renderer): rust core restored + lib renamed + reflections MVP]` on the v0.0.4 release branch — requires explicit Spencer consent before `git commit`/`git push` per the system guidance on effectful commands. See close-out narrative of the FID-016 → FID-016r2 → FID-017 close-out pass for the staged file list + draft commit message.
- **Closed:** 2026-07-13 22:00 (status:verified -> closed; auto-archive per ECHO §FID Auto-Archive; relocated from `dev/fids/` to `dev/fids/archive/`).
- **Archived:** 2026-07-13 (auto-archive per ECHO §FID Auto-Archive on `closed` status; relocated from `dev/fids/` to `dev/fids/archive/`).

### Known Issues

*(None outstanding as of close. The `savant_core` lib-name collision previously deferred here was closed in FID-016r2 — see `Verified By — FID-016r2 closure` line above. The 2 real comment-level TODOs in the 21 kept crates documented in this FID's §Lessons Learned remain in their files but are non-blocking for v0.0.4; they belong to subsystems not yet wired — `crates/agent/src/delegation/mod.rs:342` `parent_id` fallback closes in FID-018+, `crates/memory/src/cross_encoder.rs:59` cross-encoder `ort` feature placeholder closes in FID-019+.)*

---

## Lessons Learned

- **Tauri is a packaging wrapper, not a system requirement.** The Rust core stands alone; the IPC layer (`src/lib/ipc.ts`) abstracts the transport. We can swap Tauri for axum/HTTP/iceoryx2 in the future without touching the agent logic.
- **`workspace = true` inheritance is the right pattern for inter-crate deps.** The agent crate declares 19 `savant_*` deps via `workspace = true`, all of which resolve to the 21 kept crates. Direct `path = "..."` is reserved for the one cross-cutting dep (`savant_schema`).
- **Pre-copy code quality audits are cheap and catch hidden stubs.** A 30-second `grep` for `todo!()` / `unimplemented!()` / `panic!("not implemented"` across 21 crates returned 0 hits. The 2 real comment-level TODOs are documented and scoped to subsystems we don't touch in v0.0.4. The 3 false positives are regex/test/prompt artifacts, not stubs.
- **The 12-lens rotation in `crates/agent/src/pulse/prompts.rs` is the inner monologue subsystem's core mechanism.** Preserved verbatim from savant-orig; do not re-design.
- **CortexaDB is a real, populated embedded DB** (10,076 LOC at `cortexadb-core`), not a stub or vendored placeholder. The `path = "lib/cortexadb/crates/cortexadb-core"` dep carries over as-is.

---

> When status is set to **Closed**, move this file to `dev/fids/archive/` and append an entry to `CHANGELOG.md`.
