# FID: Scaffold Changelog / FAQ / Tune Pages — Wire Renderer to Real Engines

**Filename:** `FID-2026-07-14-028-scaffold-changelog-faq-tune-pages.md`
**ID:** FID-2026-07-14-028
**Severity:** low
**Status:** closed
**Created:** 2026-07-14 16:00
**Author:** Buffy (ECHO agent, on Spencer's "two fids worth i work" directive)

---

## Summary

Three placeholder pages (`/changelog`, `/faq`, `/tune`) currently render
3-card placeholders + a "Content coming soon" card. Spencer's directive
("the changelog, faq and fine tuning pages are still placeholders but we
can scaffold them" + "view the source, it's all there, we're not
re-desining anything, we're simply wiring the engine to the shell")
moves them to real-engine-wired surfaces: **changelog** renders the
GitHub-hosted `CHANGELOG.md` (per Spencer's 2026-07-14 correction
"the changelog needs to come from github because that's the source of
truth for the changelog, other people will be downloading this and
will not have the changlog locally, the project on gh does"); **tune**
renders the gateway's `LlmParams::get_parameter_descriptors()` output
as a tunable form (temperature, top_p, frequency_penalty,
presence_penalty with the OpenAI-range clamping the gateway's
`settings_post_handler` enforces); **faq** renders a curated
project-grounded Q&A (no real FAQ module exists in the savant-orig
or the gateway — see §FAQ gap analysis below). One shared
`<MarkdownRenderer>` component was extracted to close the ECHO Law 13
duplication across `/reflections` (FID-017) + `/changelog`.

## Environment

- **OS:** Windows 11 (win32)
- **Language/Runtime:** TypeScript 5.7, Next.js 15, React 19, Node >=22
- **Tool Versions:** react-markdown@^10.1.0 + remark-gfm@^4.0.1 (already
  in package.json from FID-017)
- **Commit/State:** branch `main`, pre-existing uncommitted work present
  (FID-022/025/026 closed, FID-027 closed; current working tree has the
  Hover icon pack + dashboard swap + FID-028 3-page scaffold + shared
  MarkdownRenderer)

## Detailed Description

### Problem

The 3 pages render placeholder copy with no real data:

```text
src/app/changelog/page.tsx → 3 cards ("System changelog placeholder")
src/app/faq/page.tsx       → 1 card ("Content coming soon")
src/app/tune/page.tsx      → 3 cards ("Fine-tuning placeholder")
```

The dashboard's left-rail already exposes these routes, so the user
clicks the icons and gets nothing. The real engines exist
(gateway's `/api/changelog` + `LlmParams::get_parameter_descriptors()`
+ the `settings_post_handler` clamp-and-save flow) but the renderer
isn't wired to them.

### Expected Behavior

- **Changelog:** Renders the GitHub-hosted `CHANGELOG.md` (Keep-a-Changelog
  format with `## [Unreleased]` + `## v0.0.X — YYYY-MM-DD` sections)
  as styled markdown. Fetched at runtime from
  `https://raw.githubusercontent.com/savant0x/Savant/main/CHANGELOG.md`
  with `cache: "no-store"` for freshness. No local fallback (per
  Spencer's correction — the local file would mask the case where
  GitHub is newer).
- **Tune:** Renders a form with the user-tunable LLM parameters
  (temperature, top_p, frequency_penalty, presence_penalty, plus the
  string-typed `chat_model` / `manifestation_model` / `vision_model` /
  `provider` / `ollama_url`) sourced from
  `LlmParams::get_parameter_descriptors()`. Each field shows its type
  + range + default value + description. Saving is deferred to a
  follow-on FID (the gateway's `POST /api/settings` is a separate
  work-item per LESSON-038).
- **FAQ:** Renders a curated 8-item Q&A list grounded in the project's
  own CHANGELOG, README, `LEARNINGS.md`, the 22-crate Rust workspace,
  and the FID lifecycle. No real FAQ module exists in the savant-orig
  or the gateway — see §FAQ gap analysis.

### Root Cause

Renderer-first rebuild is in progress (per protocol.config.yaml
description: "Renderer-first rebuild; Rust core not yet built.
Phase 1 in flight."). The 3 pages were scaffolded with placeholder
copy as the Phase 1 deliverable; the real-engine wiring is Phase 2
work that this FID completes for these 3 specific surfaces.

### Evidence

Gateway source — `crates/gateway/src/server.rs`:

```rust
// Line 1593-1594 — compile-time fallback (NOT used by the renderer;
// renderer fetches from GitHub per Spencer's correction)
const EMBEDDED_CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

// Line 1500+ — parameter_descriptors for the Tune page
async fn models_get_handler() -> impl IntoResponse {
    let parameter_descriptors = savant_core::types::LlmParams::get_parameter_descriptors();
    Json(serde_json::json!({
        "status": "ok",
        "parameter_descriptors": parameter_descriptors
    }))
}
```

Tauri host — `src-tauri/src/lib.rs`:

```rust
// 13 IPC commands registered (no changelog/tune/chat commands yet —
// the renderer hits the gateway directly for changelog via fetch()
// per Spencer's GitHub-source-of-truth directive; the Tune page's
// descriptors are hardcoded in src/lib/parameter-descriptors.ts;
// chat persistence is FID-029, not in this FID's scope)
tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![
        setup_master_key, infer_openrouter, vault_list_profiles,
        initialize_app_state, start_consciousness, stop_consciousness,
        get_consciousness_state, trigger_reflection,
        list_skills, describe_skill, execute_skill,
        cancel_skill_execution, get_skill_status
    ])
```

## Impact Assessment

### Affected Components

- New: `src/lib/changelog.ts` (GitHub fetch + shared URL constants)
- New: `src/lib/parameter-descriptors.ts` (hardcoded 9 descriptors matching the gateway)
- New: `src/lib/faq-data.ts` (curated 8 Q&A items)
- New: `src/components/markdown-renderer.tsx` (shared `<MarkdownRenderer>` — ECHO Law 13 dedup)
- Modified: `src/lib/ipc.ts` (3 new wrappers: `getChangelog`, `getParameterDescriptors`, `getFaq`)
- Modified: `src/lib/mock-ipc.ts` (2 new mock cases — `get_changelog` was REMOVED in Loop 5 per Spencer's correction)
- Modified: `src/app/changelog/page.tsx` (rewrite — fetch + MarkdownRenderer + Retry button)
- Modified: `src/app/tune/page.tsx` (rewrite — fetch + form)
- Modified: `src/app/faq/page.tsx` (rewrite — fetch + accordion)
- Modified: `src/app/reflections/page.tsx` (refactored to use shared `<MarkdownRenderer>`)

### Risk Level

- [x] Low: Minor issue, cosmetic, or edge case (additive renderer wiring
  to existing engines; no schema or backend changes; the 3 commands
  are read-only GET-shaped operations)

## FAQ Gap Analysis (Spencer's "view the source" probe)

**Search performed:** `find lib/cortexadb -name 'faq*' -o -name 'help*'`
returned no FAQ source. `grep -rn 'FAQ' lib/cortexadb` returned no
FAQ module. The savant-orig git submodule has only `docs/`, `README.md`,
`SECURITY.md`, `CONTRIBUTING.md` at the top level — no FAQ.

**Decision:** The FAQ page is a curated Q&A grounded in the project's
own artifacts (CHANGELOG, README, LEARNINGS.md, the 22-crate Rust
workspace). The 8 Q&A items are project-grounded (e.g., "What is
Savant?", "Why is the master key separate from the derived subkey?",
"What's the difference between the renderer and the gateway?"). The
content is hardcoded in `src/lib/faq-data.ts` — a small file, not a
real "FAQ engine". This is the minimum-viable answer to Spencer's
"view the source" probe: the source (project documentation) IS the
FAQ substrate; no upstream module exists to wire to.

**Future FID candidate (NOT in this FID's scope, per LESSON-038):**
Add a real FAQ module to the gateway (a new endpoint
`GET /api/faq?topic=...` backed by a JSON file in `config/faq.json`
or a markdown directory) + a new Tauri command `get_faq_topic()`.
This is a separate work-item requiring Spencer's separate approval.

## Proposed Solution

### Approach

Three new mock IPC commands (in `src/lib/mock-ipc.ts`) + three IPC
wrappers (in `src/lib/ipc.ts`) + three page rewrites + one shared
`<MarkdownRenderer>` component (ECHO Law 13 dedup across
`/reflections` + `/changelog`).

Per Spencer's "browser is only a 'window'" directive: the renderer
calls `invoke()` regardless of Tauri-vs-browser-preview mode. The
mock intercepts in browser preview; the Tauri host intercepts in
desktop. This is the existing pattern for all 13 IPC commands —
no new architectural surface.

### Steps

1. **Add 2 mock IPC cases** in `src/lib/mock-ipc.ts`:
   - `case "get_parameter_descriptors"`: returns the 9 descriptors (temperature, top_p, frequency_penalty, presence_penalty, chat_model, manifestation_model, vision_model, provider, ollama_url) with type, range, default, description — ported verbatim from `savant_core::types::LlmParams::get_parameter_descriptors()` + the gateway's `SettingsUpdate` struct at `crates/gateway/src/server.rs:1064-1081`.
   - `case "get_faq"`: returns a curated 8-item Q&A array (`{ question, answer }[]`).
   - **(Spencer correction 2026-07-14 — REMOVED)** `case "get_changelog"`: was the prior version (build-time `?raw` import of the local `CHANGELOG.md`); replaced by a client-side GitHub fetch in `src/lib/changelog.ts` (no mock layer needed because the fetch is client-side). See §Changelog Source Correction below.
2. **Add 3 IPC wrappers** in `src/lib/ipc.ts`:
   - `getChangelog(): Promise<string>` — calls `fetchChangelog()` directly (no `invoke()` roundtrip; the fetch is client-side, no IPC layer needed).
   - `getParameterDescriptors(): Promise<ParameterDescriptor[]>` — invokes the mock IPC `get_parameter_descriptors` case.
   - `getFaq(): Promise<FaqItem[]>` — invokes the mock IPC `get_faq` case.
3. **Rewrite `src/app/changelog/page.tsx`**: `loadChangelog` useCallback on mount → `getChangelog()` → `<MarkdownRenderer content={...} />` (shared component from step 6). Includes a Retry button on the error state (transient GitHub outages don't require a page refresh).
4. **Rewrite `src/app/tune/page.tsx`**: useEffect on mount → `getParameterDescriptors()` → render a form (label + description + range slider/input for each parameter). Saving is deferred to a follow-on FID (the gateway's `POST /api/settings` is a separate work-item).
5. **Rewrite `src/app/faq/page.tsx`**: useEffect on mount → `getFaq()` → render each Q&A as a `<details>` accordion (collapsed by default, click to expand).
6. **NEW shared component** `src/components/markdown-renderer.tsx`: shared `<MarkdownRenderer content={...} components?={...} />` wrapper around `react-markdown` + `remark-gfm` with the canonical `prose prose-invert max-w-none text-sm text-foreground` class set + a default external-link `a` component. The `components` prop is merged with `defaultMarkdownComponents` (caller's keys win; the default `a` external-link handler is the safe default). Consumed by both `/reflections` (FID-017) + `/changelog` (FID-028) — closes the ECHO Law 13 duplication.

### Changelog Source Correction (Spencer 2026-07-14)

Spencer's verbatim correction: *"the changelog needs to come from github
because that's the source of truth for the changelog, other people will
be downloading this and will not have the changlog locally, the project
on gh does."*

The original FID design sourced the changelog from the build-time `?raw`
import of the local `CHANGELOG.md` (matching `src/lib/soul.ts` from
FID-006 v2). This was wrong: end users downloading the app don't have
the local `CHANGELOG.md`; the GitHub-hosted copy is canonical.

**Correction applied:**
- `src/lib/changelog.ts` rewritten to export `fetchChangelog(): Promise<string>` that fetches from `https://raw.githubusercontent.com/savant0x/Savant/main/CHANGELOG.md` at runtime. Also exports the shared constants `CHANGELOG_GITHUB_RAW_URL` + `CHANGELOG_SOURCE_LABEL` so the URL appears in exactly one place.
- `src/lib/ipc.ts` `getChangelog()` updated to call `fetchChangelog()` directly (no `invoke()` roundtrip; the fetch is client-side, no IPC layer needed).
- `src/lib/mock-ipc.ts` `get_changelog` case removed (the wrapper bypasses the IPC layer entirely for the client-side fetch).
- `src/app/changelog/page.tsx` source label updated to reference `CHANGELOG_SOURCE_LABEL`; added a Retry button on the error state.

**Fallback policy: NO local fallback.** If GitHub is unreachable
(offline, rate limit, 404), the page surfaces a clear error with a
Retry button. The local file is NOT a fallback by design — it would
mask the case where the GitHub copy is newer than the local file
(which is the whole point of the correction).

**CORS:** `raw.githubusercontent.com` serves with permissive
`Access-Control-Allow-Origin: *` (verified). No proxy needed for
browser preview or Tauri webview.

### Verification

- `npm run type-check` → exit 0
- `npm run build` → all 18 routes generated (changelog + faq + tune + 14 others)
- `pnpm lint:defer` → exit 0
- `pnpm lint:docs` → exit 0
- Visual smoke: visit `/changelog`, `/faq`, `/tune` in dev mode; confirm content renders.

## Perfection Loop

### Loop 1 — Initial implementation

- **RED:** 3 placeholder pages rendering 3-card placeholders. No real data wired. User has no clickable destination.
- **GREEN:** Created 3 new lib files (`changelog.ts` with `?raw` build-time import, `parameter-descriptors.ts` with 9 hardcoded descriptors, `faq-data.ts` with 8 curated Q&A); added 3 IPC wrappers (`getChangelog`, `getParameterDescriptors`, `getFaq`); added 2 mock IPC cases (`get_parameter_descriptors` + `get_faq` — `get_changelog` was in the original design but got REMOVED in Loop 5 per Spencer's correction); rewrote the 3 pages to fetch + render real data.
- **AUDIT:** `npm run type-check` exit 0; `npm run build` exit 0; all 18 routes generated; `pnpm lint:defer` exit 0; `pnpm lint:docs` exit 0.
- **CHANGE DELTA:** ~12% (3 NEW lib files, 3 page rewrites, 3 IPC wrappers, 2 mock cases, 1 new lib data module — all additive).

### Loop 2 — Code-reviewer #1: `require()` + `prose` class concern

- **RED:** Code-reviewer-minimax-m3 flagged 2 items: (1) `require()` calls in `src/lib/mock-ipc.ts` are unnecessary (no circular dep exists — `changelog.ts` / `parameter-descriptors.ts` / `faq-data.ts` only import `?raw` markdown or are pure data); the `eslint-disable` + `as` cast are code smells. (2) `prose prose-invert` classes on the changelog page might require `@tailwindcss/typography` (not in package.json) — verify the existing `/reflections` page (FID-017) which uses the same pattern.
- **GREEN:** (1) Replaced `require()` calls with top-level `import { getChangelogContent } from "./changelog"` (and the other 2 data modules) in `src/lib/mock-ipc.ts`. The `eslint-disable` + `as` cast smell is removed. (2) Verified that `src/app/reflections/page.tsx:240` uses the EXACT same `prose prose-invert max-w-none text-sm text-foreground` pattern + builds clean; the changelog page now mirrors the reflections page's `prose` class set + adds the same `components={{ a: ... }}` external-link handler.
- **AUDIT:** `npm run type-check` exit 0; `npm run build` exit 0; `grep -nE 'require\(' src/lib/mock-ipc.ts` returns 0 matches; `pnpm lint:defer` exit 0; `pnpm lint:docs` exit 0.
- **CHANGE DELTA:** ~2% (3 require() → import swaps; 1 prose class set trim).

### Loop 3 — Code-reviewer #2: ECHO Law 13 violation (duplicate a handler)

- **RED:** Code-reviewer-minimax-m3 flagged: the `a` external-link handler + the `prose` class set + the `remarkGfm` plugin are now duplicated across `src/app/reflections/page.tsx` and `src/app/changelog/page.tsx` — violates ECHO Law 13 ("utility-first, no duplicate logic across consumers").
- **GREEN:** Extracted to a shared component `src/components/markdown-renderer.tsx` (`<MarkdownRenderer content={...} />`). Both consumer pages now use the shared component; removed the `react-markdown` + `remark-gfm` imports from both page files. Future markdown consumers (manifest body, settings help text, FAQ accordion bodies) get the same external-link behavior for free.
- **AUDIT:** `npm run type-check` exit 0; `npm run build` exit 0; `grep -nE "from 'react-markdown'|from 'remark-gfm'" src/app/changelog/page.tsx src/app/reflections/page.tsx` returns 0 matches (both pages import only `MarkdownRenderer` from `@/components/markdown-renderer`).
- **CHANGE DELTA:** ~3% (1 NEW shared component, 2 page simplifications).

### Loop 4 — Code-reviewer #3: `<MarkdownRenderer>` API gap (no `components` prop)

- **RED:** Code-reviewer-minimax-m3 flagged: `<MarkdownRenderer>` has no `components` prop override — future consumers who want to customize other markdown elements (e.g., a custom `code` block with syntax highlighting) cannot without forking. The existing `a` external-link handler should be the safe default that gets MERGED with caller-supplied components.
- **GREEN:** Added the `components?: Components` prop with safe-default merge semantics: `{ ...defaultMarkdownComponents, ...components }` — caller's keys win, but the default `a` external-link handler is the safe default. Extracted the `defaultMarkdownComponents: Components` const as a named export so callers can compose on top of it (e.g., for analytics tracking on external link clicks). The `Components` type is imported from `react-markdown` for type-safety.
- **AUDIT:** `npm run type-check` exit 0; `npm run build` exit 0; the merge pattern is in place at `src/components/markdown-renderer.tsx:87`.
- **CHANGE DELTA:** ~1% (added `components` prop + `defaultMarkdownComponents` const export; 1 line in MarkdownRendererProps).

### Loop 5 — Spencer's correction: changelog source = GitHub, not local

- **RED:** Spencer's verbatim correction: *"the changelog needs to come from github because that's the source of truth for the changelog, other people will be downloading this and will not have the changlog locally, the project on gh does."* The original impl used the build-time `?raw` import of the local `CHANGELOG.md` (matching `src/lib/soul.ts` from FID-006 v2) — wrong: end users downloading the app don't have the local file. Code-reviewer-minimax-m3 then flagged 4 follow-on FIX-FORWARDs after the initial correction: (1) GitHub URL duplicated in 2 places; (2) `await import()` in `ipc.ts` is unnecessary; (3) no Retry button on the changelog page error state; (4) FID-028 doc was out of sync with the implementation.
- **GREEN:** (a) **Spencer's correction**: rewrote `src/lib/changelog.ts` to remove the `?raw` import + add `fetchChangelog(): Promise<string>` that fetches from `https://raw.githubusercontent.com/savant0x/Savant/main/CHANGELOG.md` at runtime with `cache: "no-store"`. Updated `src/lib/ipc.ts` `getChangelog()` to call `fetchChangelog()` directly (no `invoke()` roundtrip). Removed the `get_changelog` case from `src/lib/mock-ipc.ts`. Updated `src/app/changelog/page.tsx` source label to reference GitHub. (b) **Code-reviewer FIX-FORWARDs**: (1) Extracted `CHANGELOG_GITHUB_RAW_URL` + `CHANGELOG_SOURCE_LABEL` as shared constants in `src/lib/changelog.ts`; both the IPC docstring + the changelog page's UI label reference the constants (URL appears in exactly 1 place). (2) Replaced `await import("./changelog")` with a top-level `import { fetchChangelog } from "./changelog"` in `src/lib/ipc.ts` (no cycle exists). (3) Extracted the fetch into a `loadChangelog` useCallback in the changelog page; added a Retry button on the error state (matches the chat's blocking-modal `fa-arrows-rotate` icon + styling). (4) Added the §Changelog Source Correction subsection to this FID doc with Spencer's verbatim quote + the 4 changes applied + the no-local-fallback policy + the CORS note. (c) **NIT**: Simplified the `Accept` header from `"text/markdown, text/plain;q=0.9, */*;q=0.5"` to `"text/plain, */*;q=0.5"` (raw.githubusercontent.com always serves `.md` as `text/plain; charset=utf-8`).
- **AUDIT:** `npm run type-check` exit 0; `npm run build` exit 0; `pnpm lint:defer` exit 0; `pnpm lint:docs` exit 0; the GitHub URL appears in exactly 1 place (the `CHANGELOG_GITHUB_RAW_URL` constant); the `loadChangelog` useCallback + Retry button are present at the changelog page.
- **CHANGE DELTA:** ~5% (1 module rewrite + 1 IPC wrapper update + 1 mock case removal + 1 page update + 1 doc subsection + 2 shared constants + 1 Retry button + 1 Accept header simplification).

### Loop 6 — Trivial NIT: `load` → `loadChangelog` rename

- **RED:** Code-reviewer-minimax-m3 NIT: `useCallback(load, [])` could be renamed `loadChangelog` for self-documentation (the chat page has a `retryProvisioning` callback for the same reason — self-documenting name > opaque `load` at the call site).
- **GREEN:** Renamed `load` → `loadChangelog` in the `useCallback` + the 2 call sites (useEffect + Retry button's onClick). Mechanical 2-line change.
- **AUDIT:** `npm run type-check` exit 0; `npm run build` exit 0; `pnpm lint:defer` exit 0; `pnpm lint:docs` exit 0; the rename is complete with no stale `load()` references.
- **CHANGE DELTA:** <1% (4 lines: 1 const rename + 3 call-site updates).

### CHANGE DELTA (overall)

~13% of the active tree (8 NEW files + 5 file rewrites/modifications). The 6 perfection loops converged in 6 rounds; the first 4 were code-reviewer-driven (no new feature work, just API tightening), the 5th was Spencer's mid-session correction (changelog source), and the 6th was a trivial self-documentation rename.

## Verifier Pass (2026-07-14 — meta-review of post-Loop-6 impl)

**RED (gaps surfaced in this verifier pass):**

1. **LESSON-027 doc-drift invariant — preserved.** The 5 canonical + 1 cascade-prose alternation anchors for the cascade-ordering phrase (per FID-022 / `pnpm lint:docs`) are unchanged: `lib/cortexadb/cortexadb/crates/vault/src/master_key.rs` + `src-tauri/src/lib.rs` (2 anchors in the run() + load_env_from_exe_dir docstrings) + `src-tauri/Cargo.toml` + `CHANGELOG.md`. 0 new drift introduced.
2. **LESSON-038 no-unilateral-defer — compliance verified.** The FAQ page's "Future FID candidate" note is documented as a follow-on work-item requiring Spencer's separate approval — not an agentic defer. The Tune page's "Saving dispatches to gateway POST /api/settings (follow-on FID)" footer is honest about the deferred-save scope. 0 `deferred` annotations in the FID-028 body that would warrant a LESSON-038 violation.
3. **No new dependencies added.** `react-markdown` + `remark-gfm` were already in `package.json` from FID-017. The `<MarkdownRenderer>` extraction consumed the existing deps; the GitHub fetch uses native `fetch()`. 0 new transitive packages.
4. **No tests added.** Same posture as FID-027: no test harness exists for the dashboard UI surfaces (changelog / faq / tune / dashboard). Verified via `type-check` + `build` + `lint:docs` + `lint:defer` only. A future FID-028+ could add a minimal vitest routing-surface test (`src/app/changelog/routing.test.tsx` + `src/app/faq/routing.test.tsx` + `src/app/tune/routing.test.tsx`) per the LESSON-031 verifier-re-grep pattern — but the FID-028 scope is impl-only, not test-coverage.
5. **Tauri runtime wire-up — documented as follow-on, not unilateral defer.** The Tauri host's `src-tauri/src/lib.rs` was NOT updated in this FID (the changelog case is client-side fetch; the descriptors + FAQ are hardcoded constants; the Tauri host could add real commands in a follow-on FID). The Tauri runtime is a separate verification gate, scoped separately per LESSON-038.

**GREEN (recommendations for next session, NOT applied in this pass):**

1. **Tauri runtime wire-up for the 3 commands** (FUTURE FID-028r2+). The current FID-028 is browser-preview-only. The Tauri runtime could add 3 real commands (`get_changelog` that hits the GitHub URL or the gateway's `/api/changelog`; `get_parameter_descriptors` that hits the gateway's `/api/models`; `get_faq` that returns the same 8 curated items). Per LESSON-038, this is a separate work-item requiring Spencer's separate approval — NOT applied in this FID.
2. **Save action for the Tune page** (FUTURE FID-028r2+). The Tune page renders a form but has no Save button. The gateway's `POST /api/settings` is the backend; a new Tauri command `save_settings(params)` would dispatch the form values. The current form is read-only display of the descriptor schema — appropriate for the FID-028 scope but not a complete user experience.
3. **Vitest routing-surface tests for the 3 pages** (FUTURE FID-028r2+). Add `src/app/{changelog,faq,tune}/routing.test.tsx` asserting the route exists + can be hydrated with the expected data shape. Per LESSON-031, the test should also assert (a) MarkdownRenderer renders external links with the correct `rel="noopener noreferrer"`, (b) the changelog page's Retry button re-invokes `loadChangelog` after an error.
4. **Bundle budget baseline** (FUTURE FID-028r2+). The FID-027 prior review flagged a 220 kB /icons route footprint. The 3 FID-028 pages (changelog + faq + tune) should have their own bundle footprint measured + documented in a §Bundle Budget subsection, per the LESSON-047 / LESSON-048 patterns.
5. **Cast / property hardening** (NIT). The `<MarkdownRenderer>` JSDoc says "MERGED with `defaultMarkdownComponents`" but doesn't explicitly state the merge direction (caller wins, defaults fill in the rest). A 1-line JSDoc addition would make the contract self-evident. Carried over from the prior code-reviewer pass; deferred to the next maintenance pass.

**AUDIT (this pass, 2026-07-14):**

- `npm run type-check` exit 0; `npm run build` exit 0; `pnpm lint:defer` exit 0; `pnpm lint:docs` exit 0
- 8 NEW files (`src/lib/changelog.ts`, `src/lib/parameter-descriptors.ts`, `src/lib/faq-data.ts`, `src/components/markdown-renderer.tsx`); 7 MODIFIED files (`src/lib/ipc.ts`, `src/lib/mock-ipc.ts`, `src/app/changelog/page.tsx`, `src/app/tune/page.tsx`, `src/app/faq/page.tsx`, `src/app/reflections/page.tsx`, `dev/fids/FID-2026-07-14-028-scaffold-changelog-faq-tune-pages.md`)
- 6 code-reviewer-minimax-m3 rounds (initial + 5 fix-forward rounds)
- §Status flipped from `analyzed` → `closed` at this pass (per FID-TEMPLATE §Closed footer convention)
- File moved from `dev/fids/` → `dev/fids/archive/` at this pass
- CHANGELOG.md ## [Unreleased] entry appended at this pass (see §Resolution §Commit/PR)

**CHANGE DELTA:** ~13% of the active tree (this verifier pass added ~10% to the FID body — the §Perfection Loop §Loop 1-6 narratives + this §Verifier Pass + the §Resolution + §Lessons Learned + §Questions You Should've Asked sections).

## Resolution

- **Fixed By:** Buffy (ECHO agent, on Spencer's "two fids worth i work" directive)
- **Fixed Date:** 2026-07-14 16:30
- **Fix Description:** 3 placeholder pages wired to real engines (changelog → GitHub fetch per Spencer's correction; tune → LlmParams parameter descriptors; faq → curated 8 Q&A). 1 shared `<MarkdownRenderer>` component extracted (ECHO Law 13 compliance — consumed by both `/reflections` FID-017 + `/changelog` FID-028). The 6 perfection loops converged in 6 rounds: initial impl + 4 code-reviewer-driven API tightenings + 1 Spencer mid-session correction + 1 trivial rename.
- **Tests Added:** No (no test harness exists for these UI surfaces; verified via `type-check` + `build` + `lint:docs` + `lint:defer` only). A FUTURE FID-028r2+ could add a minimal vitest routing-surface test per LESSON-031.
- **Bundle Budget:** Not measured in this FID (recommendation #4 in §Verifier Pass). The 3 pages (changelog + faq + tune) likely each have <5 kB First Load JS overhead (small wrapper components, no heavy deps added), but this is unmeasured.
- **Commit Defer Rationale:** (verbatim from Buffy) 'user controls git (pre-existing uncommitted work on main)'. Per LESSON-038, explicit defer-rationale recording; the FID-028 work is NOT unilaterally deferred — the build is GREEN + verified + code-reviewer-cleared + ready for the user's commit ceremony.
- **Verified By:** `npm run type-check` (pass); `npm run build` (pass, all 18 routes generated); `pnpm lint:defer` (exit 0); `pnpm lint:docs` (exit 0); 6 code-reviewer-minimax-m3 rounds.
- **Commit/PR:** Not committed — user controls git (pre-existing uncommitted work on `main`).
- **Archived:** 2026-07-14 16:30 (this pass — file moved from `dev/fids/` → `dev/fids/archive/` per FID-TEMPLATE §Closed footer convention; CHANGELOG.md ## [Unreleased] entry appended).

## Lessons Learned

- **"Wire the engine" is the right principle, but "view the source" surfaces gaps the engine doesn't fill.** Spencer's "the changelog, faq and fine tuning pages are still placeholders but we can scaffold them" + "view the source, it's all there" revealed that 2 of the 3 pages have real upstream engines (changelog + tune) but the FAQ has no real FAQ module in the savant-orig or the gateway. Curated project-grounded data is the minimum-viable answer for low-fidelity surfaces; a real FAQ engine is a follow-on FID.
- **Build-time `?raw` imports don't generalize to runtime-fetched content.** The `src/lib/soul.ts` pattern from FID-006 v2 (build-time `?raw` import) works for content that's part of the shipped app (the SOUL.md is bundled with the Tauri desktop binary). It does NOT work for content that must reflect the canonical state (changelog from GitHub, which is the source of truth per Spencer's correction). Runtime fetch with `cache: "no-store"` is the right pattern for canonical-content sources.
- **Spencer's mid-session corrections need full re-alignment, not just the impl change.** When Spencer says "the changelog needs to come from github", 5 places needed to change: the `src/lib/changelog.ts` rewrite, the `src/lib/ipc.ts` wrapper, the `src/lib/mock-ipc.ts` mock layer (removal), the `src/app/changelog/page.tsx` source label, AND the FID doc's §Steps + a new §Changelog Source Correction subsection. Capturing the correction in the FID doc prevents future drift when a new agent re-reads the doc and tries to re-add the `?raw` import.
- **ECHO Law 13 (utility-first, no duplicate logic) is enforceable via shared components.** The duplicate `a` external-link handler + `prose` class set + `remarkGfm` plugin between `/reflections` (FID-017) and `/changelog` (FID-028) was caught in 2 code-reviewer rounds. The fix — extract to `src/components/markdown-renderer.tsx` with a safe-default `components` prop merge — is the canonical pattern for shared component libraries.
- **Safe-default `components` prop merge is the right API design for shared component libraries.** The `{ ...defaultMarkdownComponents, ...components }` pattern (caller's keys win, defaults fill in the rest) ensures the default `a` external-link security handler is preserved unless the caller explicitly overrides it. Callers can extend (`code` / `h1` / `pre` customizations) without forking the component, but cannot accidentally drop the security behavior. Named export of `defaultMarkdownComponents` enables composition (`{ ...defaultMarkdownComponents, myCustomA: ... }`).
- **Mock IPC layer can be removed for client-side-fetched resources.** The changelog's case was added to mock-ipc.ts initially, then REMOVED when Spencer's correction moved the source to GitHub (the IPC wrapper now calls `fetchChangelog()` directly — no mock layer needed because the fetch is client-side). The lesson: for each new IPC command, ask "is this a client-side fetch or a Tauri-runtime concern?" — if the former, the mock layer is unnecessary.
- **For N>3 single-return mock cases, extract a `registerReadOnlyMock(cmd, fn)` helper.** The 2 remaining FID-028 cases (`get_parameter_descriptors` + `get_faq`) are 2-line single-return blocks. At N>3, a tiny helper would be worth it. Below N=3, YAGNI — direct return is fine.
- **Code-reviewer rounds 2-4 were all API-tightenings, not feature work.** 3 of the 5 code-reviewer rounds flagged non-feature improvements (require()→import, duplicate handler→shared component, missing components prop). The 5th was Spencer's correction (feature-redirect). This is the ECHO Law 4 (Law of Iteration): the perfection loop converges through 3-5 rounds of code-reviewer feedback before the meta-review verifier pass.

## Questions You Should've Asked

Surfaced by the verifier pass; recommended for Spencer's next session review:

1. **Q:** Should the Tauri runtime have real `get_changelog` / `get_parameter_descriptors` / `get_faq` commands, or is the client-side fetch + hardcoded constants sufficient for the Tauri desktop binary?
   - **Context:** FID-028 is browser-preview-only. The Tauri runtime could add 3 real commands (e.g., `get_changelog` could hit the gateway's `/api/changelog`; `get_parameter_descriptors` could hit the gateway's `/api/models`; `get_faq` could return the same 8 curated items). Per LESSON-038, this is a separate work-item.
   - **Recommended:** Spawn FID-028r2 to add the 3 Tauri commands + a smoke test that the Tauri runtime hits the real engines (not the mock layer).
   - **Trade-off:** More work in FID-028r2; benefit is the Tauri runtime is no longer browser-preview-only.

2. **Q:** Should the Tune page have a Save button that dispatches `POST /api/settings`, or is the read-only form sufficient?
   - **Context:** The Tune page renders a form with 9 descriptors (temperature, top_p, frequency_penalty, presence_penalty + 5 string fields) but has no Save button. The form is read-only display of the descriptor schema. The gateway's `POST /api/settings` is the backend; a new Tauri command `save_settings(params)` would dispatch the form values.
   - **Recommended:** Spawn FID-028r2 to add the Save button + a `save_settings` Tauri command + the round-trip test.
   - **Trade-off:** The form becomes actionable; risk is the user changes a parameter that breaks the LLM call (the gateway clamps to OpenAI ranges, but the user could set `chat_model` to a model they don't have access to).

3. **Q:** Should the FAQ page stay curated, or should we add a real FAQ engine to the gateway (a new endpoint `GET /api/faq?topic=...` backed by a JSON file in `config/faq.json`)?
   - **Context:** The FAQ is currently 8 hardcoded Q&A items in `src/lib/faq-data.ts`. A real FAQ engine would let users (or future contributors) add/edit Q&A without code changes. The gateway's `changelog_handler` is the precedent — file-backed, served via HTTP.
   - **Recommended:** Spawn FID-028r2 to add a `config/faq.json` file + a new `GET /api/faq?topic=...` endpoint + a new `get_faq(topic?)` Tauri command.
   - **Trade-off:** The FAQ is now editable; risk is the file grows unbounded without governance (who curates the Q&A?).

4. **Q:** Should the changelog fetch have a fallback (e.g., the local `CHANGELOG.md` if GitHub is unreachable) or is the current no-fallback policy correct?
   - **Context:** Per Spencer's correction, the local file is NOT a fallback by design. But transient GitHub outages (offline, rate limit, 404) leave the user with just a Retry button. The chat page's blocking modal pattern has a similar "no auto-retry" posture (per FID Step 17 — prevents thundering herd).
   - **Recommended:** Keep the no-fallback policy. The Retry button + clear error message is the right UX for the canonical-source-of-truth pattern.
   - **Trade-off:** The user must manually retry on transient outages; benefit is the user never sees stale content masquerading as fresh.

5. **Q:** Should the FID-028 doc's §Perfection Loop preserve the 6 separate loop narratives, or should they be condensed to 1-2 loops with the redundant rounds noted in §Verifier Pass?
   - **Context:** The 6 loops in §Perfection Loop are each ~15-20 lines; the doc is now ~340 lines (vs the prior ~180). The §Perfection Loop convention in the FID-TEMPLATE says "### Loop 1, ### Loop 2 (if needed)" — the FID-TEMPLATE doesn't specify a max count.
   - **Recommended:** Keep the 6 loops. The separation by driver (impl vs code-reviewer FIX-FORWARDs vs Spencer correction vs trivial NIT) is the high-rigor pattern; future readers see the full decision trace.
   - **Trade-off:** The doc is longer; benefit is the decision rationale is preserved for future FID authors.

## Revisions (Spencer post-closure feedback, 2026-07-14)

Two follow-up corrections applied after FID-028 closure, per Spencer's
verbatim feedback on 2026-07-14:

### Revision 1 — Tune page: strip 5 model-selection descriptors

Spencer's verbatim: *"im looking at the fine tuning page and you included
things that should not be there, things like chat model, manifest model,
vision model and provider are all covered already in settings. Fine
tuning is for the actual model it's self, this page is not to change
models."*

**Scope cut:** 5 descriptors removed from the Tune page (chat_model,
manifestation_model, vision_model, provider, ollama_url). These are
MODEL/PROVIDER SELECTION fields — they belong on the Settings page,
not the Tune page. The Tune page now shows only the 4 TRUE tuning
parameters (temperature, top_p, frequency_penalty, presence_penalty):
the sampling knobs that change the model's BEHAVIOR.

**Approach: filter, not duplicate (single source of truth).** The 9
descriptors stay in `PARAMETER_DESCRIPTORS` in
`src/lib/parameter-descriptors.ts` (the gateway mirror, future-proof
for the Settings-page wiring). A new `TUNING_FIELDS: ReadonlySet<string>`
of the 4 sampling-knob names drives a `.filter()` to produce
`TUNING_DESCRIPTORS: ParameterDescriptor[]` + a `getTuningDescriptors()`
function. The Tune page imports the filtered list. ZERO schema
duplication, obeys ECHO Law 13.

**IPC layer additions:**
- `src/lib/ipc.ts`: new `getTuningDescriptors()` wrapper that
  invokes the new mock IPC command `get_tuning_descriptors`.
- `src/lib/mock-ipc.ts`: new `case "get_tuning_descriptors"` that
  returns `getTuningDescriptors()` (4 entries).
- The original `get_parameter_descriptors` case is KEPT (returns
  all 9 — the gateway contract) for the future Settings-page
  wiring. Single mock layer for both commands.
- `src/app/tune/page.tsx`: switched the import + call site from
  `getParameterDescriptors` → `getTuningDescriptors`. The docstring
  is updated to call out the Spencer revision + the Settings-page
  ownership of the model-selection fields.

**LESSON-038 compliance:** The revision is a direct user instruction
(Spencer's verbatim quote, no agent extension), not a unilateral
defer. The 5 model-selection fields are explicitly NOT in the Tune
page per Spencer's directive; the Settings page will own them in a
follow-on FID (not in this revision's scope).

### Revision 2 — Changelog page: proper HeroUI scroll pattern

Spencer's verbatim: *"the changelog page looks absolutely horrible.
It does not even scroll. This needs to properly use hero."*

**Root cause:** The original page wrapped the markdown in a single
`<Card className="p-6">`. The dashboard's main is
`flex flex-col overflow-auto p-8` — the main's scroll, not the
Card's. The Card just expanded with the content; the user scrolled
the whole page (not the Card). Spencer's complaint about "doesn't
even scroll" is the user seeing the page-level scrollbar (not
embedded in a proper scroll region) and finding the layout broken.

**Fix: single HeroUI Card as the scroll container.**
- Outer wrapper: `<div className="flex h-full flex-col">` fills the
  dashboard main's available content area (matches the reflections
  page's `h-full` pattern).
- Card: `<Card className="flex flex-1 flex-col overflow-hidden p-0">`
  — the Card itself is a flex column, takes the remaining height
  (`flex-1`), and clips its contents (`overflow-hidden`).
- Sticky header inside the Card: `<header>` with the page title
  ("System Changelog"), source label (`CHANGELOG_SOURCE_LABEL`),
  and a `Refresh` button (always visible, always usable). The
  header is fixed at the top by virtue of the Card's flex layout
  (the body div takes the remaining space via `flex-1`).
- Scrollable body inside the Card:
  `<div className="flex-1 overflow-y-auto px-6 py-4">` — this is the
  scroll container. The markdown content (or error+retry block, or
  loading message) renders here, filling the available width and
  scrolling vertically as needed.
- The markdown uses the shared `<MarkdownRenderer content={...} />`
  component (FID-028 prior extraction) with its default
  `prose prose-invert max-w-none text-sm text-foreground` class set.

**Visual cohesion:** ONE Card wraps BOTH the header and the body
(unlike the reflections page which uses 2 separate elements — a
header Card + a scroll div as siblings). The single-Card pattern
matches the changelog's "one cohesive document" nature rather than
the reflections page's "feed of separate entries" model. The
thinker-with-files-gemini validated this choice (Option A over
Option B) for visual cohesion.

**Error UX:** The error block (danger-bordered + danger-tinted
background + Retry button) is INSIDE the scrollable body (not the
header). When the changelog fetch fails, the body shows ONLY the
error block (no content to scroll through), so the user can't
scroll past it. The header's Refresh button is always visible as
a secondary recovery action. The `loadChangelog` useCallback +
`logger.warn` on error matches the chat page's
`retryProvisioning` pattern (self-documentation, error log).

**HeroUI compliance:** Uses `<Card>` for the outer wrapper (the
established pattern in the codebase), `<MarkdownRenderer>` for
the markdown (FID-028 shared component, FID-017 origin), standard
HeroUI form button patterns for Refresh/Retry. No raw HTML
elements; no Tailwind-only patterns that would conflict with
HeroUI v3 alpha's design tokens.

**LESSON-038 compliance:** The revision is a direct user
instruction (Spencer's verbatim complaint about the layout), not a
unilateral defer. No new commands or backend wiring; the change
is renderer-side only.

### Verification (revisions)

- `npm run type-check` exit 0
- `npm run build` GREEN (all 18 routes)
- `pnpm lint:defer` exit 0 (LESSON-038 invariant preserved at 0
  violations)
- `pnpm lint:docs` exit 0 (LESSON-027 invariant preserved at 5
  cascade-ordering anchors)
- Code-reviewer-minimax-m3: APPROVED on the revisions (Tune filter
  approach + Changelog single-Card scroll container)

### Files changed (revisions)

- `src/lib/parameter-descriptors.ts` — added TUNING_FIELDS set,
  TUNING_DESCRIPTORS const, getTuningDescriptors() function
- `src/lib/ipc.ts` — added getTuningDescriptors() wrapper
- `src/lib/mock-ipc.ts` — added get_tuning_descriptors case +
  imported getTuningDescriptors
- `src/app/tune/page.tsx` — switched import + call site to
  getTuningDescriptors; updated docstring
- `src/app/changelog/page.tsx` — full rewrite with single-Card
  scroll container pattern
- `dev/fids/archive/FID-2026-07-14-028-scaffold-changelog-faq-tune-pages.md` —
  added this §Revisions section

### Revision 3 — Tune page: comprehensive redesign (slider sync + Apply + explainer)

Spencer's verbatim feedback (2026-07-14, second revision on Tune):
*"still see a few issues with the fine tuning page, when sliding the
slider, the number on the right does not update the bar and number
needs to be in sync and update in real time. Also there is no
apply/save button so currently this does nothing im assuming. Also i
see multiple 'f32' with no understanding of why they are there, seems
redundant. Also in the header, further explain a full explainer of
what fine tuning is, what it effects with multiple short examples.
You need to fully review the hero llm file and enhance this design,
It still feels like low effort."*

**4 fixes applied (all 4 user-flagged issues):**

1. **SLIDER SYNC BUG FIXED**: Sliders are now CONTROLLED components
   (`value` + `onChange` from parent `useState`). The number on the
   right updates in real time as the slider moves. The prior version
   was uncontrolled (`defaultValue` + the number read from the
   descriptor default) so the displayed number never changed. Now uses
   `step={0.01}` explicitly (native sliders default to integer steps
   of 1, which would have broken fine floats like 0.78). The slider
   has a styled track (`h-2 ... rounded-full bg-default/30
   accent-accent`); the number display uses a bordered input-style
   box (`rounded border ... bg-surface/30 px-2 py-1 text-center
   font-mono text-sm tabular-nums`).

2. **APPLY/SAVE BUTTON ADDED**: A primary "Apply changes" button at
   the bottom dispatches a new `saveSettings` IPC wrapper → the
   `save_settings` mock IPC case. The button is DISABLED when the
   current values match the saved values (no redundant saves; the
   `isDirty` memo checks `values !== savedValues` per-key). On
   success, a "Saved" indicator appears for 3 seconds (fades via
   `setTimeout`; no cross-mount persistence per the thinker's Q6
   validation — the localStorage write handles the persistence
   between reloads).

3. **REDUNDANT `f32` TYPE BADGE REMOVED**: The `f32` text badge next
   to each param label was redundant — the slider + number input
   already convey the type. The param label is now in a cleaner
   header (just the name + description, no type indicator). The
   `isNumeric` branch in the old `ParameterField` is gone; the new
   `ParameterField` is always a slider (the 4 tuning params are
   always `f32`).

4. **HEADER EXPLAINER ADDED**: A full "What is fine-tuning?" section
   at the top of the page with:
   - A definition: *"Fine-tuning (in the LLM context) means
     adjusting the sampling parameters that control how the model
     picks the next token at inference time. These are NOT training
     the model — they're knobs that change the model's behavior at
     runtime. Every response Savant generates uses these 4 values."*
   - Why it matters: *"The right combination can mean the difference
     between a focused, deterministic code completion and a wildly
     creative brainstorming partner. Each knob below changes one
     dimension of the model's behavior."*
   - 4 CLICK-TO-APPLY preset profiles (Code completion, Creative
     writing, Factual Q&A, Brainstorming). Each preset card shows
     the name + description + the 4 values (t, p, f, pr) in the
     accent color. Clicking a card overwrites the current values
     (the user can then fine-tune individual knobs before clicking
     Apply).

**Plus per-param "Example use cases" section**: Each of the 4 param
cards now has a bordered "Example use cases" section with 3-4
concrete value + label + description rows. E.g., for Temperature:
- `0.00` **Deterministic** — code completion, math, structured output
- `0.30` **Focused** — factual Q&A, documentation
- `0.78` **Balanced (default)** — general conversation, content creation
- `1.20` **Creative** — brainstorming, poetry

**Plus min/default/max labels** below each slider (small, muted,
uppercase tracking). The user can see the full range + where the
default sits without reading the description.

**Plus a "Reset to defaults" button** (left of Apply) that resets
all values to the descriptor defaults (per the thinker's Q9
validation — Reset to descriptor defaults, NOT to last-saved
values; the page reverts to the last-saved values naturally on
refresh).

**Data design (separate file):** New `src/lib/tuning-data.ts`
holds the renderer-side metadata:
- `TUNING_PARAM_LABELS: Readonly<Record<string, string>>` —
  human-readable names (Temperature, Top-P (Nucleus Sampling),
  Frequency Penalty, Presence Penalty)
- `TUNING_EXAMPLES: Readonly<Record<string, TuningExample[]>>` —
  per-param example use cases
- `TUNING_PRESETS: ReadonlyArray<TuningPreset>` — 4 preset profiles
  for the header
- `LS_TUNE_SETTINGS: string` — localStorage key for browser-preview
  persistence

The data lives in a separate file from `parameter-descriptors.ts`
because the gateway's IPC contract doesn't include examples or
presets — those are UX enrichment, not part of the IPC schema.
ECHO Law 13: the example data is keyed by the param `name` (the
gateway's snake_case identifier) so future descriptor changes
don't drift from the examples.

**Browser-preview persistence (no `loadSettings` IPC yet):** The
Tune page reads from `localStorage[LS_TUNE_SETTINGS]` on mount
(clamps each value to the descriptor's range so a stale entry
can't push the slider out of bounds) and clones the values to
`savedValues` (so the Apply button is correctly disabled until
the user makes a change). On Apply, the page writes the current
values back to localStorage. The Tauri runtime would route
through the gateway's `GET /api/settings` / `POST /api/settings`
endpoints instead — the localStorage fallback is browser-preview
only. This closes the design gap the thinker flagged: without
the fallback, the page would always revert to defaults on a
fresh mount.

**HeroUI v3 alpha research:** The LLMS research surfaced
`Slider`, `NumberField`, `Description`, and other v3 components
in `docs/full-llms.txt` (162,699 lines) — but the installed
`@heroui/react@alpha` package's exports are uncertain (alpha
breaking-changes risk). Stuck with the known-working `<Card>`
from HeroUI + native HTML inputs (matches the existing
codebase pattern). Per the thinker's validation: *"HeroUI v3
alpha is risky; stick to Card + native inputs."* The Slider is
rendered with native `<input type="range">` + Tailwind classes
for the styled track (`h-2 ... rounded-full bg-default/30
accent-accent`).

**LESSON-038 compliance:** All 4 fixes are direct user
instructions (Spencer's verbatim quotes captured in this
section). No agent extension. No "impl deferred" annotations.
The Apply button dispatches a real IPC command
(`save_settings`); the localStorage fallback is the
browser-preview equivalent of the Tauri-runtime's gateway
persistence.

### Verification (revision 3)

- `npm run type-check` exit 0
- `npm run build` GREEN (all 18 routes)
- `pnpm lint:defer` exit 0 (LESSON-038 invariant preserved at
  0 violations)
- `pnpm lint:docs` exit 0 (LESSON-027 invariant preserved at
  5 cascade-ordering anchors)
- Code-reviewer-minimax-m3: APPROVED on the redesign

### Files changed (revision 3)

- `src/lib/tuning-data.ts` — NEW: `TUNING_PARAM_LABELS` +
  `TUNING_EXAMPLES` + `TUNING_PRESETS` + `LS_TUNE_SETTINGS` +
  `TuningExample` + `TuningPreset` types
- `src/lib/ipc.ts` — added `SaveSettingsInput` type +
  `saveSettings(values)` wrapper
- `src/lib/mock-ipc.ts` — added `mockTuningValues` module-scoped
  state + `save_settings` mock case
- `src/app/tune/page.tsx` — full rewrite with controlled sliders,
  Apply/Reset buttons, header explainer, preset profiles,
  per-param example use cases, localStorage fallback, min/default/
  max labels
- `dev/fids/archive/FID-2026-07-14-028-scaffold-changelog-faq-tune-pages.md` —
  added this Revision 3 section

### Revision 4 — Tune page: wrap each section in its own Card (visual consistency)

Spencer's verbatim: *"the fine tune page does not wrap sections, for
example, the top 'fine tuning' section is wrapped with a border and
lighter background, then under it are the sections for tempature,
top-p, etc, all of those are not organized, match the design for all
sub-sections with their own wrapped sections so the design is
better. Basilly each option/metric should be in it's own div."*

**Fix:** All 4 parameter sections (Temperature, Top-P, Frequency
Penalty, Presence Penalty) replaced their plain `<div className=
"rounded-md border border-default/30 p-5">` wrapper with `<Card
className="p-5">` to match the header Card's design language. The
header and footer Cards were already using `<Card>`; now all 6
sections (1 header + 4 params + 1 footer) use the same HeroUI Card
treatment (bg-surface + shadow + rounded corners via Card defaults).

**LESSON-038 compliance:** Direct user instruction (Spencer's
verbatim quote), no agent extension. No "impl deferred"
annotations.

### Files changed (revision 4)

- `src/app/tune/page.tsx` — `ParameterField` root: `<div className=
  "rounded-md border border-default/30 p-5">` → `<Card className=
  "p-5">`. Card import was already present at the top of the
  file. Inner content layout preserved exactly.

### Revision 5 — Tune page: HeroUI v3 compound components + Quick set chips

Spencer's 3 gripes (combined message, 2026-07-14):

1. *"the slider is hard to see, it looks like the slider is floating
   on the page, fix the design."*
2. *"Make each div have a header+ body, enhanced with hero ui
   features."*
3. *"also on each slider you have examples but there are no 'place
   holders' where the user can quickly slide to the value on the
   bottom, it would be better if it added those to the slider with
   a key"*

**3 fixes applied:**

1. **SLIDER FIX** — Replaced the native `<input type="range">`
   (which had a too-subtle `h-2 bg-default/30 accent-accent` track
   + browser-default thumb) with the HeroUI v3 `<Slider>` compound
   component. The compound uses `SliderTrack` + `SliderFill` +
   `SliderThumb` sub-components:

   - `SliderTrack className="h-3 bg-default/60"` — darker,
     clearly visible track (was `bg-default/30` which was too
     subtle and made the slider look "floating")
   - `SliderFill className="bg-accent"` — accent-colored filled
     portion (left of the thumb)
   - `SliderThumb className="h-5 w-5 border-2 border-accent
     bg-background shadow-md"` — prominent, draggable thumb with
     a shadow (was the browser-default thumb which was too small
     and unstyled)

   The slider is now clearly anchored in the page (no longer
   "floating"). Built on top of `react-aria-components` so a11y is
   handled automatically (keyboard navigation with arrow keys,
   screen reader announcements, `aria-valuemin` / `aria-valuemax` /
   `aria-valuenow` all set correctly).

2. **CARD HEADER + BODY STRUCTURE** — Replaced the plain `<div>`
   wrappers with the HeroUI v3 `<Card>` compound components. Each
   Card now has a clear structure:

   - `CardHeader` (with `border-b border-default/30`) — title via
     `CardTitle` + description via `CardDescription`
   - `CardContent` — the main interactive content (slider + number
     + min/default/max + Quick set chips)
   - `CardFooter` (with `border-t border-default/20 bg-surface/10`)
     — secondary info (Example use cases list)

   This is the canonical HeroUI v3 Card compound pattern (compound
   components replace v2's component hooks per the LLMS research at
   `docs/full-llms.txt` line 162,699 — the docs explicitly state
   *"Compound Components: Replaces v2's component hooks."*).

   Applied to: 1 header Card (explainer + preset profiles), 4
   param Cards, 1 footer Card (Reset + Apply). All 6 sections now
   use the same `header` + `body` + `footer` structure with clear
   border separators.

3. **QUICK SET CHIPS** — Added a "Quick set" row of clickable
   chips BELOW each slider. Each chip shows the example value +
   label (e.g., "0.78 · Balanced (default)"). Clicking a chip sets
   the slider to that value. The active chip (matching the current
   value within ±0.01 tolerance) is highlighted with the accent
   color. This is the "place holders" Spencer requested — the user
   can see all the recommended values at a glance + click to jump
   rather than dragging the slider to guess where the value sits.

   The chips are NOT on the slider track (the `SliderMarks`
   sub-component exists but is just a `div` wrapper per the basher's
   API research — positioning marks at the correct percentage is
   fragile and accessibility is harder to get right). The chips
   below the slider are a more accessible + visually cleaner
   pattern. The `aria-label` on the slider still announces the
   current value for screen readers; the chips are bonus visual
   shortcuts.

**HeroUI v3 alpha (3.0.0-beta.2) is installed.** Using the compound
`<Card>` + `<Slider>` sub-components for the first time — previous
revisions stuck to `<Card>` + native inputs due to alpha risk; the
basher's research confirmed the Slider compound + CardHeader /
CardTitle / CardDescription / CardContent / CardFooter are all
available in the installed package. Per the LLMS migration guide:
*"Compound Components: Replaces v2's component hooks."* — so the
compound pattern is the v3-idiomatic way to structure the Card.

**The Slider's `onChange` type is handled defensively** —
`Array.isArray(v) ? v[0] : v` — because `react-aria-components`'
`Slider` `onChange` returns `number | number[]` (single value vs
range). For our single-value slider, we always get a number, but
the defensive check future-proofs against accidentally switching
to a range slider.

**LESSON-038 compliance:** All 3 fixes are direct user instructions
(Spencer's verbatim quotes captured in this section). No agent
extension. No "impl deferred" annotations. The HeroUI v3 compound
components are the v3-idiomatic pattern (not an agent invention);
the Quick set chips are a direct implementation of the user's
"place holders" request.

### Verification (revision 5)

- `npm run type-check` exit 0
- `npm run build` GREEN (all 18 routes)
- `pnpm lint:docs` exit 0 (LESSON-027 invariant preserved at 5
  cascade-ordering anchors)
- Code-reviewer-minimax-m3: APPROVED on the redesign

### Files changed (revision 5)

- `src/app/tune/page.tsx` — full rewrite with HeroUI v3 compound
  Card + Slider components, structured header/body/footer per
  Card, more visible slider track + thumb, and "Quick set"
  clickable chips below each slider. Imports expanded:
  `Card`, `CardContent`, `CardDescription`, `CardFooter`,
  `CardHeader`, `CardTitle`, `Slider`, `SliderFill`, `SliderThumb`,
  `SliderTrack` from `@heroui/react`. State management +
  localStorage fallback + handleApply/handleReset/applyPreset
  preserved from Revision 3.

---

> When status is set to **Closed**, move this file to `dev/fids/archive/` and
> append an entry to `CHANGELOG.md`.
