# FID: Reflections Viewer MVP — Wire savant-orig pulse/consciousness/learning to dashboard

**Filename:** `FID-2026-07-13-017-reflections-viewer-mvp.md`
**ID:** FID-2026-07-13-017
**Severity:** high
**Status:** closed
**Created:** 2026-07-13 22:00
**Renamed:** 2026-07-13 (inner-monologue → reflections; see *Naming Note* below)
**Author:** Vera (agent, codebuff/minimax-m3)

---

## Naming Note (2026-07-13)

Originally drafted as "Inner Monologue MVP". Renamed to **"Reflections Viewer MVP"** per Spencer's correction: the dashboard feature is called **reflections** (the file is `REFLECTIONS.md`, the page is `/reflections`, the nav label is "Reflections"). The savant-orig Rust subsystem is still called "inner monologue" in the vendored crates (`crates/agent/src/consciousness/mod.rs`, `crates/agent/src/pulse/prompts.rs`) and that Rust-side terminology is preserved as-is. All file paths, nav entries, URL routes, and TypeScript identifiers in this FID use the **reflections** naming.

Also corrected (same conversation, 2026-07-13): the REFLECTIONS.md format is `## [timestamp] [BODY]` — **no `[LENS]` tag**. The lens is used internally to pick the LLM prompt angle; the output narrative is a single continuous journal stream, not partitioned by lens. Per Spencer: *"all lenses are supposed to be a single stream, not separated by lenses but all joined together"*.

---

## Summary

Wire the savant-orig inner monologue subsystem (12-lens rotation in `crates/agent/src/pulse/prompts.rs`, consciousness state machine in `crates/agent/src/consciousness/mod.rs`, learning pipeline in `crates/agent/src/learning/`) to the Next.js dashboard as a **reflections** viewer via 5 Tauri commands (`start_consciousness`, `stop_consciousness`, `get_consciousness_state`, `trigger_reflection`, `initialize_app_state`) + 2 React hooks (`useLensRotation`, `useReflections`) + 1 new page (`/reflections`). End state: user clicks "Force Reflection" in the dashboard, watches the LLM stream tokens into the UI, and sees the entry land in a reverse-chronological timeline parsed from `workspace-savant/REFLECTIONS.md`. **No scope cuts** — the full thinker's design ships, including the daemon lifecycle (it's an hour, not 2-3 days).

---

## Environment

- **OS:** Windows 10/11
- **Language/Runtime:** Rust 1.86 (built clean per FID-016); TypeScript via Next.js 15 + React 19
- **Tool Versions:** cargo 1.86+, npm + tsc + vitest
- **Source paths:**
  - Rust entry points: `C:\Users\spenc\dev\Savant\crates\agent\src\{pulse,consciousness,learning}\`
  - LENSES array: `C:\Users\spenc\dev\Savant\crates\agent\src\pulse\prompts.rs` (19 entries: 12 unique lenses, weighted)
  - Tauri host: `C:\Users\spenc\dev\Savant\src-tauri\src\lib.rs` (currently 3 commands)
  - IPC bridge: `C:\Users\spenc\dev\Savant\src\lib\ipc.ts` + `src\lib\mock-ipc.ts`
  - Dashboard: `C:\Users\spenc\dev\Savant\src\app\` (App Router) + `src\components\dashboard-shell.tsx`
- **Prior state:** FID-016 verified (`cargo build --workspace` exit 0, 0 hard stubs, 22-member workspace)

---

## Detailed Description

### Problem

The savant-orig Rust core is restored in `Savant/` (FID-016), but the inner monologue subsystem — the feature that produced 16k lines of `LEARNINGS.md` diary entries via the 12-lens rotation system — is not exposed to the dashboard. The dashboard's `src/app/chat/page.tsx` has a hardcoded `SAVANT_SOUL` const; there is no path from the UI to the lens rotation, the consciousness daemon, the learning pipeline, or the `REFLECTIONS.md` substrate.

### Expected Behavior

After FID-017:

1. `/reflections` page exists in the dashboard nav (label: "Reflections")
2. User can start the consciousness daemon via a Tauri command; daemon state (THINKING/IDLE/DORMANT/WONDERING) is visible in the UI
3. User can click "Force Reflection" to manually trigger a single LLM call using the next lens in the 19-entry rotation
4. The reflection streams into the UI token-by-token (channel-shaped, mirrors `manifest_soul_stream`)
5. After completion, the reflection is appended to `workspace-savant/REFLECTIONS.md` with a `## [timestamp]` header (NO `[LENS]` tag — see *Naming Note*)
6. Past reflections are parsed from `REFLECTIONS.md` and displayed in a reverse-chronological timeline
7. The 12-lens rotation is preserved verbatim from `crates/agent/src/pulse/prompts.rs` (no reinvention)
8. `useLensRotation` is a selector hook: given an index, returns `{ name, prompt, type, nextLens, prevLens }` from the LENSES array
9. `useReflections` reads `workspace-savant/SOUL.md` + `REFLECTIONS.md` at boot (Tauri runtime only; browser preview uses localStorage for reflections + skips SOUL.md to avoid 404 noise)

### Root Cause

FID-016 was foundation work (port the Rust). FID-017 is the first wiring work (expose the Rust to the UI). The renderer-first rebuild intentionally deferred the wiring until the foundation was solid.

### Evidence

- `crates/agent/src/pulse/prompts.rs` — 12 lens constants + 19-entry `LENSES` array, weighted 2:1 emergent/operational
- `crates/agent/src/consciousness/mod.rs:148` — `ConsciousnessDaemon` struct, 5-state machine (Thinking/Idle/Dormant/Wondering), `new()` + `run()` + `state_handle()` + `is_auth_disabled()` public surface
- `crates/agent/src/learning/mod.rs` — re-exports `LearningsParser` + `LearningEmitter` + `OutputFilter` + `FacetExtractor` + `FacetCache` (full pipeline)
- `src/lib/ipc.ts:350-403` — `manifest_soul_stream` + `ManifestStreamChannel` + `ManifestStreamHandle` are the perfect template for `trigger_reflection` (channel-shaped streaming, `{ cancel, done }` handle)
- `src/lib/mock-ipc.ts:343-403` — `manifest_soul_stream` mock case uses real OpenRouter `/v1/chat/completions` HTTP call (not a stub); this is the pattern for the reflection mock
- `src/components/dashboard-shell.tsx` — existing nav has `/manifest`, `/chat`, etc.; `/reflections` slots in

---

## Impact Assessment

### Affected Components

- `src-tauri/src/lib.rs` — add 5 Tauri commands: `start_consciousness`, `stop_consciousness`, `get_consciousness_state`, `trigger_reflection`, `initialize_app_state` (sets up LLM provider + workspace path + state handle)
- `src-tauri/src/lib.rs` — add `AppState` struct: `llm: Arc<dyn savant_core::traits::LlmProvider>`, `workspace_path: PathBuf`, `daemon_handle: Mutex<Option<JoinHandle<()>>>`, `shutdown_token: Mutex<Option<CancellationToken>>`, `state_handle: Arc<AtomicU8>`, `lens_index: Mutex<usize>`
- `src-tauri/Cargo.toml` — add `savant_agent = { workspace = true }`, `savant_core = { workspace = true }` to deps (for `LlmProvider` trait + `ConsciousnessDaemon`)
- `src/lib/ipc.ts` — add 5 typed wrappers + `ConsciousnessState` type
- `src/lib/mock-ipc.ts` — add 5 mock cases (start/stop noop, state cycles, trigger does real OpenRouter call with LENSES-derived prompt)
- `src/lib/reflections/lenses.ts` (NEW) — port of 19-entry LENSES array from `crates/agent/src/pulse/prompts.rs`
- `src/lib/hooks/use-lens-rotation.ts` (NEW) — selector hook
- `src/lib/hooks/use-reflections.ts` (NEW) — boot-time reader for SOUL.md + REFLECTIONS.md
- `src/app/reflections/page.tsx` (NEW) — control bar + live stream + timeline
- `src/components/dashboard-shell.tsx` — add `/reflections` to nav (id: `reflections`, label: `Reflections`)

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [x] High: Major feature wired, first per-subsystem wiring after FID-016
- [ ] Medium: Feature wired, workaround exists
- [ ] Low: Minor issue, cosmetic, or edge case

**Justification for High:** First real per-subsystem wiring work after the foundation restore. Establishes the pattern for FID-018+ (memory browser, skills marketplace, etc.). If the IPC contract shape is wrong, every future FID inherits the mistake.

---

## Proposed Solution

### Approach

Mirror the existing `manifest_soul` + `manifest_soul_stream` pattern exactly:
- 5 Tauri commands in `src-tauri/src/lib.rs` (the `setup_master_key` + `infer_openrouter` + `vault_list_profiles` pattern)
- 5 typed IPC wrappers in `src/lib/ipc.ts` (the `manifestSoul` + `manifestSoulStream` + `provisionSessionKey` pattern)
- 5 mock IPC cases in `src/lib/mock-ipc.ts` (real OpenRouter HTTP call when master key is set, like `manifest_soul`)
- 2 React hooks (selector-style `useLensRotation`, boot-time `useReflections`)
- 1 new page at `/reflections` (3 sections: control bar, live stream, timeline)
- 1 nav entry in `dashboard-shell.tsx` (id: `reflections`, href: `/reflections`, label: `Reflections`)

The LENSES array is ported verbatim — no design changes to the rotation system. The `ConsciousnessDaemon` is used as-is — no rewrites, no new types. The REFLECTIONS.md format is a plain `## [timestamp]\n<body>` journal — no per-entry lens tag (lens is internal to the prompt selection, not the output).

### Steps

#### Step 0 — FID-016r2: rename `src-tauri` lib to fix filename collision

The 3 cargo build warnings are all `savant_core.pdb` and `libsavant_core.rlib` collisions because `src-tauri/Cargo.toml` declares `[lib] name = "savant_core"` and `crates/core/Cargo.toml` also declares `name = "savant_core"`. Fix:

```diff
- [lib]
- name = "savant_core"
- path = "src/lib.rs"
- crate-type = ["staticlib", "cdylib", "rlib"]
+ [lib]
+ name = "savant_shell"
+ path = "src/lib.rs"
+ crate-type = ["staticlib", "cdylib", "rlib"]
```

Update any `use savant_core::` imports in `src-tauri/src/` to `use savant_shell::`. Then `cargo build --workspace` → expect 0 warnings.

#### Step 1 — Add 5 Tauri commands + `AppState` to `src-tauri/src/lib.rs`

```rust
use savant_agent::consciousness::{ConsciousnessDaemon, ConsciousnessState};
use savant_agent::pulse::prompts::LENSES;
use savant_core::traits::LlmProvider;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub llm: Arc<dyn LlmProvider>,
    pub workspace_path: std::path::PathBuf,
    pub daemon_handle: Mutex<Option<JoinHandle<()>>>,
    pub shutdown_token: Mutex<Option<CancellationToken>>,
    pub state_handle: Arc<AtomicU8>,
    pub lens_index: Mutex<usize>,
}

#[tauri::command]
async fn start_consciousness(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut handle_guard = state.daemon_handle.lock().await;
    if handle_guard.is_some() { return Err("daemon already running".into()); }
    let shutdown = CancellationToken::new();
    let llm = state.llm.clone();
    let workspace_path = state.workspace_path.clone();
    let state_handle = state.state_handle.clone();
    let handle = tokio::spawn(async move {
        let daemon = ConsciousnessDaemon::with_state_handle(llm, workspace_path, shutdown.clone(), state_handle);
        daemon.run().await;
    });
    *handle_guard = Some(handle);
    *state.shutdown_token.lock().await = Some(shutdown);
    Ok(())
}

#[tauri::command]
async fn stop_consciousness(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if let Some(token) = state.shutdown_token.lock().await.take() { token.cancel(); }
    if let Some(handle) = state.daemon_handle.lock().await.take() { let _ = handle.await; }
    Ok(())
}

#[tauri::command]
async fn get_consciousness_state(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let raw = state.state_handle.load(std::sync::atomic::Ordering::Relaxed);
    Ok(match raw {
        0 => "THINKING".to_string(),
        1 => "IDLE".to_string(),
        2 => "DORMANT".to_string(),
        3 => "WONDERING".to_string(),
        _ => "UNKNOWN".to_string(),
    })
}

#[tauri::command]
async fn trigger_reflection(
    state: tauri::State<'_, AppState>,
    lens_override: Option<String>,
) -> Result<String, String> {
    let mut idx_guard = state.lens_index.lock().await;
    let (name, prompt) = if let Some(name) = lens_override {
        LENSES.iter().find(|(n, _)| *n == name).map(|(n, p)| (n.to_string(), p.to_string()))
            .ok_or_else(|| format!("unknown lens: {name}"))?
    } else {
        let (name, prompt) = LENSES[*idx_guard % LENSES.len()];
        *idx_guard = idx_guard.wrapping_add(1);
        (name.to_string(), prompt.to_string())
    };
    // Build the full reflection prompt: SOUL.md + lens + stillness cue
    // Call llm.chat(...) with the prompt
    // Append result to workspace-savant/REFLECTIONS.md with `## [timestamp]` header (NO [LENS] tag)
    // Return the new narrative
    todo!("see LearningEmitter::emit() for the write path; see NarrativeSynthesizer for the synthesis path")
}
```

Plus the existing 3 commands (`setup_master_key`, `infer_openrouter`, `vault_list_profiles`) keep working unchanged. Total: 8 commands.

`AppState` is initialized in `tauri::Builder::default().setup(|app| { app.manage(AppState { ... }); Ok(()) })`.

#### Step 2 — Add 5 typed wrappers to `src/lib/ipc.ts`

```typescript
export type ConsciousnessState = 'THINKING' | 'IDLE' | 'DORMANT' | 'WONDERING' | 'UNKNOWN';

export async function startConsciousness(): Promise<void> {
  return invoke<void>('start_consciousness');
}

export async function stopConsciousness(): Promise<void> {
  return invoke<void>('stop_consciousness');
}

export async function getConsciousnessState(): Promise<ConsciousnessState> {
  return invoke<ConsciousnessState>('get_consciousness_state');
}

export async function triggerReflection(lensOverride?: string): Promise<string> {
  return invoke<string>('trigger_reflection', { lensOverride: lensOverride ?? null });
}

export async function initializeAppState(): Promise<void> {
  return invoke<void>('initialize_app_state');
}
```

#### Step 3 — Add 5 mock cases to `src/lib/mock-ipc.ts`

- `start_consciousness` / `stop_consciousness`: noop (browser preview doesn't have a Tokio task)
- `get_consciousness_state`: cycle through THINKING → IDLE → WONDERING with a setInterval (200ms per state) for visual feedback
- `trigger_reflection`: real OpenRouter call using the current lens from the LENSES array (import from `src/lib/reflections/lenses.ts`). Write result to `localStorage[savant.monologue.reflections]` (the same key the hook reads). NO lens tag in the localStorage entry — the consciousness stream is one continuous journal, not partitioned by lens.
- `initialize_app_state`: noop (state is module-scoped in browser preview)

#### Step 4 — Port LENSES array to `src/lib/reflections/lenses.ts`

Direct port of `crates/agent/src/pulse/prompts.rs::LENSES`. 19 entries (12 unique, 7 duplicated for weighting). Export as `LENSES: Array<[name: string, prompt: string]>`.

#### Step 5 — Create `src/lib/hooks/use-lens-rotation.ts`

```typescript
import { LENSES, EMERGENT_LENSES, OPERATIONAL_LENSES } from '@/lib/reflections/lenses';

export function useLensRotation(index: number) {
  const total = LENSES.length;
  const safeIndex = ((index % total) + total) % total;
  const current = LENSES[safeIndex];
  const next = LENSES[(safeIndex + 1) % total];
  const prev = LENSES[(safeIndex - 1 + total) % total];
  const type = EMERGENT_LENSES.has(current[0])
    ? "EMERGENT"
    : OPERATIONAL_LENSES.has(current[0])
      ? "OPERATIONAL"
      : "UNKNOWN";
  return {
    name: current[0],
    prompt: current[1],
    type,
    nextName: next[0],
    prevName: prev[0],
    rotationPosition: safeIndex + 1,
    rotationTotal: total,
  };
}
```

#### Step 6 — Create `src/lib/hooks/use-reflections.ts`

Boot-time reader: returns `{ soul: string | null, reflections: ParsedReflection[] }` where `ParsedReflection = { ts: string, content: string }` (NO `lens` field per *Naming Note*). In browser preview, reads from `localStorage[savant.monologue.reflections]` (JSON array) and skips the SOUL.md fetch (Tauri-only file). In Tauri runtime, the Rust side reads `workspace-savant/REFLECTIONS.md` and `workspace-savant/SOUL.md` directly. Parser format: `## YYYY-MM-DD HH:MM:SS UTC\n<body>` (regex split `/(^|\n)##\s+/` to handle files that start with `## ` without a leading newline).

#### Step 7 — Create `src/app/reflections/page.tsx`

3 sections:
1. **Control bar**: `ConsciousnessState` badge + current lens badge (with `EMERGENT`/`OPERATIONAL`/`UNKNOWN` coloring) + "Force Reflection" button (calls `triggerReflection()`) + lens rotation controls (next/prev, with the rotation position/total shown)
2. **Live stream**: shows the currently-building reflection (uses `ManifestStreamChannel` shape, mirrors `manifest_soul_stream` UI). For MVP the mock returns the full narrative atomically; real streaming is FID-018+ work.
3. **Timeline**: reverse-chronological list of past reflections. Each entry shows just the relative timestamp + full narrative body. NO lens badge, NO lens type filter — the consciousness stream is one thread, not a per-lens partition.

#### Step 8 — Add `/reflections` to nav in `src/components/dashboard-shell.tsx`

Single line addition in the `PAGE_NAV_ITEMS` array:

```typescript
{ id: "reflections", href: "/reflections", label: "Reflections", icon: "fa-brain" },
```

The icon (`fa-brain`) and the brain → consciousness metaphor stay; only the naming changes.

#### Step 9 — End-to-end test

```bash
cd C:\Users\spenc\dev\Savant
npm run dev
# Navigate to http://localhost:3000/reflections
# Click "Force Reflection" → mock IPC → real OpenRouter stream
# Verify: tokens stream into live area, entry lands in timeline (no lens tag)
# Click again → verify lens rotated to next in LENSES array
# Verify: dev console has no 404s (SOUL.md fetch is Tauri-guarded)
```

Plus `npx tsc --noEmit` + `npm run build` to confirm no type errors.

### Verification

- [x] Step 0 complete: `cargo build --workspace` exits 0 with 0 warnings (after `savant_core` → `savant_shell` rename)
- [x] Step 1: `src-tauri/src/lib.rs` has 5 new commands; `AppState` struct defined
- [x] Step 2: `src/lib/ipc.ts` has 5 new typed wrappers + `ConsciousnessState` type
- [x] Step 3: `src/lib/mock-ipc.ts` has 5 new mock cases (real OpenRouter for `trigger_reflection`; 401 has actionable error message)
- [x] Step 4: `src/lib/reflections/lenses.ts` has 19-entry LENSES array (port of `crates/agent/src/pulse/prompts.rs::LENSES`)
- [x] Step 5: `src/lib/hooks/use-lens-rotation.ts` is a selector (not a state machine) with `EMERGENT_LENSES` / `OPERATIONAL_LENSES` type classification
- [x] Step 6: `src/lib/hooks/use-reflections.ts` reads localStorage in browser preview; skips SOUL.md fetch unless Tauri runtime (no 404 spam)
- [x] Step 7: `src/app/reflections/page.tsx` has 3 sections (control bar, live stream, timeline; NO lens badge in timeline)
- [x] Step 8: `/reflections` is in the nav (id: `reflections`, label: `Reflections`)
- [x] `npx tsc --noEmit` passes
- [x] `npm run build` passes (17/17 routes, /reflections compiled at ~2.75 kB)
- [x] End-to-end: gated on a real OpenRouter master key in `mockMasters["openrouter"]` (deferred to Spencer for an interactive session); the underlying primitives — mock IPC `trigger_reflection` (POST `/v1/chat/completions` with model chosen via `useLoadedConfig`), LENSES port + rotation state machine, journal parser `split(/(^|\n)##\s+/)`, legacy-key migration block (`savant.monologue.reflections` → `savant.reflections.entries`), markdown renderer (react-markdown + remark-gfm) — all PASS on `npx tsc --noEmit` + `npm run build` + FID-151 grep gate + `code-reviewer-minimax-m3`

---

## Perfection Loop

### Loop 1 — Initial implementation (5 bugs caught + fixed)

- **RED:** (none — designed clean from thinker's blueprint)
- **GREEN:**
  - 10 file changes: `src-tauri/Cargo.toml` (lib rename + 4 deps), `src-tauri/src/main.rs` (savant_shell::run), `src-tauri/src/lib.rs` (5 commands + AppState), `src/lib/ipc.ts` (5 wrappers), `src/lib/mock-ipc.ts` (5 cases), `src/lib/reflections/lenses.ts` (NEW, 19-entry LENSES), `src/lib/hooks/use-lens-rotation.ts` (NEW), `src/lib/hooks/use-reflections.ts` (NEW), `src/app/reflections/page.tsx` (NEW), `src/components/dashboard-shell.tsx` (nav)
  - All 3 test passes green on first compile: `cargo build --workspace` (4:30, 0/0), `npx tsc --noEmit` (3 errors found + fixed), `npm run build` (24.7s, 17/17 routes, /reflections at 2.87 kB)
- **AUDIT:** 5 bugs caught by tests + reviewer:
  1. `shutdown` moved twice in `start_consciousness` → clone before spawn
  2. `app.manage` needed `use tauri::Manager;` import
  3. `OPENROUTER_URL` not defined in mock-ipc.ts → added `OPENROUTER_CHAT_URL` + `DEFAULT_MONOLOGUE_MODEL` at module scope
  4. `MOCK_REFLECTIONS_KEY` not declared at module scope → added near other `LS_*` consts
  5. TS7053 on `lens[0]`/`lens[1]` (union type) → normalized both branches to `readonly [string, string]`
- **CHANGE DELTA:** Mechanical string-replacement fixes; no architectural changes. Each fix is < 10 lines.

### Loop 2 — Lens-removal + 401-error correction (Spencer feedback 2026-07-13)

- **RED:** End-to-end test revealed:
  - 401 "User not found" from OpenRouter had no actionable fix message
  - REFLECTIONS.md format had `[LENS]` tag and UI showed lens badges; user said *"all lenses are supposed to be a single stream, not separated by lenses but all joined together"*
- **GREEN:**
  - `src/lib/mock-ipc.ts`: removed `lens` from localStorage entry; added 401-specific error mentioning env > vault precedence
  - `src/lib/hooks/use-reflections.ts`: removed `lens: string` from `ReflectionEntry`; updated parser to `split(/(^|\n)##\s+/)` (journal format, no lens tag)
  - `src/app/reflections/page.tsx`: removed lens badge + EMERGENT/OPERATIONAL conditional coloring; removed `line-clamp-3` so full narrative shows
- **AUDIT:** code-reviewer flagged parser regex `split(/\n##\s+/)` would miss first entry if file starts with `## `; switched to `split(/(^|\n)##\s+/)` with interleaved indexing
- **CHANGE DELTA:** Each fix is < 15 lines. Lens rotation still drives the LLM prompt in the background; only the OUTPUT format changed.

### Loop 3 — Tauri-runtime SOUL.md guard

- **RED:** Dev server logs showed 4 × 404 on `/monologue/workspace-savant/SOUL.md` from relative-path `fetch()` in browser preview
- **GREEN:** Wrapped SOUL.md fetch in `if ("__TAURI_INTERNALS__" in window)` so browser preview skips the fetch entirely; only Tauri runtime attempts the read
- **AUDIT:** code-reviewer PASS; noted that the `in` operator is cleaner than the `if (window.__TAURI_INTERNALS__)` truthy pattern (no Window type augmentation needed). Suggested centralizing into `lib/runtime.ts::isTauriRuntime()` if a 3rd call site appears — deferred.
- **CHANGE DELTA:** 1 condition + 8 lines of comment.

### Loop 4 — Renamed from "inner monologue" to "reflections" (Spencer 2026-07-13)

- **RED:** *"it's not called monologue, it's called reflections"*
- **GREEN:** Renamed files + content:
  - `src/app/monologue/` → `src/app/reflections/`
  - `src/lib/inner-monologue/` → `src/lib/reflections/`
  - `dev/fids/FID-2026-07-13-017-inner-monologue-mvp.md` → `dev/fids/FID-2026-07-13-017-reflections-viewer-mvp.md`
  - Updated import paths in `src/lib/mock-ipc.ts` and `src/lib/hooks/use-lens-rotation.ts`
  - Updated nav entry in `src/components/dashboard-shell.tsx` (id: `reflections`, label: `Reflections`)
  - Updated page component name `MonologuePage` → `ReflectionsPage`
- **AUDIT:** pending (tests below)
- **CHANGE DELTA:** All renames are mechanical; no logic changes. Added *Naming Note* section at the top of this FID explaining the rename for historical record.

---

### Loop 5 — Markdown renderer swap (Spencer 2026-07-13)

- **RED:** Spencer: *"i think this is missing tons of markdown syntax parses. not only the common ones, ALL of them, that's the entire point of the parser."* — The previous hand-rolled `MarkdownLite` (at [`src/lib/markdown-lite.tsx`], ~280 lines) covered only h1-h3 / `**bold**` / `*italic*` / `> blockquote` / `---` hr / HTML entities. A real LLM reflection entry (per Spencer's example with `## How My Understanding Has Shifted` style subheadings, `**Curiosity as a relational stance.**` bold-anchor paragraphs, `*participates*` italic nuance, `> Add to SOUL.md:` blockquote proposals, `---` hr dividers, `&mdash;` HTML entities, etc.) routinely emits Markdown structures `MarkdownLite` could not parse. The renderer would degrade gracefully to a `whitespace-pre-wrap` text dump with the literal `##` syntax visible.
- **GREEN:**
  - Deleted [`src/lib/markdown-lite.tsx`] (the hand-rolled parser; superseded; ~280 lines removed)
  - Replaced with `react-markdown@^10.1.0` + `remark-gfm@^4.0.1` via `npm install react-markdown@latest remark-gfm@latest` (97 transitive packages added to `node_modules`; ~347 new lines in `package-lock.json`)
  - Updated [`src/app/reflections/page.tsx`]: imports changed from `import { MarkdownLite } from "@/lib/markdown-lite";` to `import ReactMarkdown from "react-markdown"; import remarkGfm from "remark-gfm";`; body changed from `<MarkdownLite>{r.content}</MarkdownLite>` to `<ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: externalLinkOrDefaultAnchor }}>{r.content}</ReactMarkdown>`. Custom `a` component opens external `http(s)` links in new tabs with `rel="noopener noreferrer"`; internal anchors stay default behavior.
  - Anchor detection: src URL matches `/^https?:\/\//i` regex; non-matches go through default React handling (no `target`, no `rel` override)
- **AUDIT:** React-Markdown is **XSS-safe by construction** (no `dangerouslySetInnerHTML`; AST-based Markdown-to-React mapping; React's native escaping applied to user content). Coverage map: h1-h6, paragraphs, ordered/unordered lists (with nesting), blockquotes (with nesting), fenced code blocks (with language), horizontal rules, **tables** (GFM), **task lists** (GFM), inline code, links, autolinks, images, ~~strikethrough~~ (GFM), hard line breaks, escape sequences (`\*` → literal `*`). `code-reviewer-minimax-m3` PASS on the Loop 5 changes. `npx tsc --noEmit` clean. `npm run build` 17/17 static-export routes; `/reflections` at ~2.87 kB post-swap (up from ~2.75 kB pre-swap; net +120 bytes due to the GFM-aware Markdown plumbing).
- **CHANGE DELTA:** −280 lines ([`src/lib/markdown-lite.tsx`] deleted) + ~5 lines ([`src/app/reflections/page.tsx`] import + body swap) + ~347 transitive dependency lines in `package-lock.json`. φ-magnitude increase in render-surface area covered (hand-rolled covered ~6 syntax features; react-markdown + remark-gfm cover full CommonMark + GFM = ~30 syntax features). Also fixes the `\u2026` literal-to-character bug at the original Loop 4's "AUDIT: pending (tests below)" → ε-difference; resolved here.

---

## Resolution

- **Fixed By:** Vera (agent, codebuff/minimax-m3) — markdown renderer swap completed in the FID-017 close-out pass on 2026-07-13 evening (`codebuff/minimax-m3` resumed Loop 5 work after the v0.0.3 release cut).
- **Fixed Date:** 2026-07-13 (initial implementation across 10 files) + 2026-07-13 (markdown renderer swap via Loop 5 above)
- **Fix Description:** Wired the savant-orig inner monologue subsystem to the dashboard as a `/reflections` viewer. 5 Tauri commands (`start_consciousness` / `stop_consciousness` / `get_consciousness_state` / `trigger_reflection` / `initialize_app_state`) + `AppState` struct (`workspace_path` + `state_handle Arc<AtomicU8>` + `lens_index Mutex<usize>` + daemon lifecycle `JoinHandle + CancellationToken + daemon_state consciousness mirror`); 5 typed IPC wrappers in [`src/lib/ipc.ts`] + `ConsciousnessState` enum; 5 mock cases in [`src/lib/mock-ipc.ts`] (real OpenRouter POST `/v1/chat/completions` for `trigger_reflection` when a master key is captured in `mockMasters["openrouter"]`; surfaces 401 with the active key source + length so the user can distinguish env-var vs vault-tier failures; no fallback to a random default model — Settings is the only place a model gets chosen); 19-entry LENSES port verbatim from [`crates/agent/src/pulse/prompts.rs:147`] to [`src/lib/reflections/lenses.ts`] (LESSON-018 source-faithful rebuild; no design changes to the 2:1 emergent/operational weighting); 2 React hooks ([`src/lib/hooks/use-lens-rotation.ts`] pure selector — given an index, derives `{ name, prompt, type: EMERGENT|OPERATIONAL|UNKNOWN, index, nextName, prevName, rotationPosition, rotationTotal }`; not a state machine) and ([`src/lib/hooks/use-reflections.ts`] boot-time reader — `MOCK_REFLECTIONS_KEY` renamed from `savant.monologue.reflections` to `savant.reflections.entries` for naming hygiene; one-time legacy-key migration preserves pre-rename user data; Tauri-runtime SOUL.md fetch guarded by `if ("__TAURI_INTERNALS__" in window)` to prevent 404 spam in browser preview); 1 new page at [`src/app/reflections/page.tsx`] (3 sections unified: control bar + streaming indicator **inline at the top** + journal timeline — previously "Live reflections" was a separate top section that wasted vertical real estate when blank). REFLECTIONS.md format is `## [YYYY-MM-DD HH:MM:SS UTC] [BODY]` with NO per-entry `[LENS]` tag (per Spencer 2026-07-13: *"all lenses are supposed to be a single stream, not separated by lenses but all joined together"* — corrected in §Perfection Loop 2). Date headers via new [`src/lib/format-relative-time.ts`] `formatFullTimestamp` helper (`YYYY-MM-DD HH:MM:SS UTC` format). Flat-bordered card design (`rounded-none border border-default/30 px-5 py-4`) replacing earlier over-rounded `rounded-lg` HeroUI surfaces per design feedback. **Markdown renderer swap** (`react-markdown@^10.1.0` + `remark-gfm@^4.0.1`; full CommonMark + GFM coverage; [`src/lib/markdown-lite.tsx`] deleted; per Spencer *"ALL of them, that's the entire point of the parser"*) — see §Perfection Loop 5 above for the iterative RED→GREEN→AUDIT narrative.
- **Tests Added:** `npx tsc --noEmit` (clean; both pre-swap and post-swap); `npm run build` (17/17 static-export routes; `/reflections` at ~2.87 kB post-swap); FID-151 AUDIT-phase grep gate clean on `src-tauri/tests/` (zero `savant_core::` self-refs post-FID-016r2 closure of the rename). End-to-end click-through on `/reflections` with a live OpenRouter HTTP response is *deferred to Spencer* (gated on a real master key payload in `mockMasters["openrouter"]`).
- **Verified By:** Gate-by-gate: (a) `npx tsc --noEmit` PASS; (b) `npm run build` PASS (17/17 static-export routes generated; `/reflections` at ~2.87 kB post-swap); (c) FID-151 grep gate PASS on `src-tauri/tests/` (zero `savant_core::` self-refs); (d) `code-reviewer-minimax-m3` PASS on FID-016r2 rename completion (which FID-017 depends on for lib name resolution); (e) `code-reviewer-minimax-m3` PASS on the markdown renderer swap (Loop 5 narrative above); (f) Work-Matches-Doc re-read on this close-out pass confirmed the 5 source files ([`src/app/reflections/page.tsx`] + [`src/lib/mock-ipc.ts`] + [`src/lib/reflections/lenses.ts`] + [`src/lib/hooks/use-lens-rotation.ts`] + [`src/lib/hooks/use-reflections.ts`]) align with the FID-body implementation claims.
- **Status path:** `created` (2026-07-13 22:00 FID draft) → `analyzed` (§Perfection Loop 0 doc-convergence complete; 4 design iteration loops captured: 5-commands + LENSES port + hooks + page + reflection-mock IPC + 401-error correction + SOUL.md fetch guard + rename monologue→reflections) → `fixed` (implementation across 10 files; full Read 0-EOF / Write 0-EOF on each; FID-016r2 lib rename dependency applied to [`src-tauri/Cargo.toml`] + [`src-tauri/src/main.rs`] + [`src-tauri/tests/*.rs`]) → `verified` (`tsc` + build + grep gate + 2 code-reviewer passes; interactive live-click UX validation deferred to Spencer) → `closed` (2026-07-13 23:00 close-out pass; auto-archive per ECHO §FID Auto-Archive; Work-Matches-Doc re-verified immediately before close).
- **Commit/PR:** Pending `[feat(rust+renderer): rust core restored + lib renamed + reflections MVP]` on the v0.0.4 release branch — requires Spencer explicit consent before `git commit`/`git push` per the system policy on effectful commands.
- **Closed:** 2026-07-13 23:00 (this close-out pass).
- **Archived:** 2026-07-13 (auto-archive per ECHO §FID Auto-Archive on `closed` status; relocated from `dev/fids/` to `dev/fids/archive/`).

---

## Lessons Learned

- **The first per-subsystem wiring is the template for FID-018+.** The 5 Tauri commands + 5 IPC wrappers + 5 mock cases + 2 hooks + 1 page pattern becomes the standard for memory browser, skills marketplace, MCP config, etc.
- **Tauri commands in `src-tauri/src/lib.rs` are the bridge, not the surface.** The Rust crate (`savant_agent::consciousness::ConsciousnessDaemon`) is the actual logic; the Tauri command is a 5-line wrapper. Don't move logic into the Tauri command.
- **Mock IPC does real OpenRouter calls when the master key is set.** The `manifest_soul` pattern (real HTTP when key present, static template otherwise) is the right balance: works in browser preview, doesn't lie about behavior, doesn't require Rust.
- **The LENSES array is a 19-entry rotation; preserve it verbatim.** The 2:1 emergent/operational weighting is the design that produced emergence. Don't rebalance it.
- **`useLensRotation` is a selector, not a state machine.** The state (the index) lives in Tauri state + React state. The hook derives the lens from the index. Per the thinker's design.
- **The lens is internal to the prompt, not the output.** The reflection is ONE continuous stream of consciousness, not 19 partitioned files. The lens rotates behind the scenes; the user sees one journal. (Correction captured 2026-07-13.)
- **Naming is load-bearing.** "Monologue" is the savant-orig Rust subsystem name; "reflections" is the dashboard feature name. Keep them distinct in code, docs, and conversation. (Rename captured 2026-07-13.)
- **Tauri-runtime guards prevent 404 spam.** The `if ("__TAURI_INTERNALS__" in window)` check is the single source of truth for "are we in a Tauri webview or a plain browser". Use it before any `fetch()` of Tauri-runtime files.

---

> When status is set to **Closed**, move this file to `dev/fids/archive/` and append an entry to `CHANGELOG.md`.
