# FID-024: Checkpoint Release Discipline (Build-Freely + Push-at-Release Automation)

**Filename:** `FID-2026-07-14-024-checkpoint-release-discipline.md`
**ID:** FID-2026-07-14-024
**Severity:** medium
**Severity rationale:** workflow-discipline + tooling work (no architectural shifts, no security implications, no data model changes); boundary regression risk if not codified (per-feature pushes + manual release-prep drift both cause history pollution + reviewer friction). The 5 NEW scripts are convenience wrappers that close a release-prep gap (existing `scripts/release.py` covers tag/push/GitHub-release but lacks auto-archive / version-bump / README-refresh / bloat-cleanup steps).
**Status:** closed
**Created:** 2026-07-14 05:00
**Author:** Savant

---

## Summary

Bundle Spencer's 2026-07-14 meta-policy directive (**build-freely between checkpoints + push only at release cuts**) with the corresponding release-prep automation tooling (5 NEW scripts + 1 MODIFIED coding-standard + 1 MODIFIED `package.json` + 1 MODIFIED `scripts/release.py` wrapper) into a single FID that ships as the v0.0.6 (or whichever release cycle is next) work unit. Net: 1 user-ratable `pnpm release:prep X.Y.Z` command orchestrates the full release sweep (auto-archive FIDs → version-bump 5 anchors in lockstep → refresh README → clean dead/bloat files → run verification gates → invoke `scripts/release.py` for tag + push + GitHub release), and the workflow discipline is codified in `coding-standards/release-workflow.md` §Checkpoint Release Discipline as the standing rule for future cycles.

---

## Environment

- **OS:** Windows 11 (dev box); cross-platform tooling (bash 5.x + Python 3.x for the existing `release.py`)
- **Language/Runtime:** Bash 5.x (5 NEW shell scripts); Python 3.x (existing `release.py` + optional `bump-version.py`)
- **Tool Versions:** `pnpm` 9.x; `bash` 5.x + `sh` (POSIX fallback); `git` 2.43+ (existing baseline); `python` 3.x (existing baseline)
- **Working Directory:** `C:\Users\spenc\dev\Savant`
- **Commit/State:** post-v0.0.5 release cut on `origin/main` at `08fd353`; LESSON-027 invariant preserved at 5 anchors + 1 cascade-prose canonical; 2 active FIDs in `dev/fids/` (FID-022 `fixed` + FID-023 `analyzed`); 1 known dead-bloat candidate (`./dev/.tmp-fid-022-commit.txt` + `./resources/hermes-agent/infographic/dead-delivery-targets`); 5+ version anchors currently at `0.0.5`.
- **Existing tooling baseline:** `scripts/release.py` (existing tag+push+GitHub release pipeline); `scripts/lint-docs.sh` (FID-022 LESSON-027 invariant); `scripts/release-check.sh` (FID-022 LESSON-029 3-gate); `scripts/commit-with-message.sh` + `scripts/tag-with-message.sh` (FID-022 LESSON-030 file-based); `scripts/verify-fix.sh` (FID-022 LESSON-031 dual-check). No auto-archive tool; no version-bump tool; no README-refresh tool; no bloat-cleanup tool; no orchestrator.

---

## Detailed Description

### Problem

Two intertwined problems emerge from continued per-feature push cadence:

**(1) Workflow discipline** — the v0.0.5 release cycle produced 5 commits in 1 day (374bda7 + 592da64 + 463d71a + 1369706 + 08fd353; per the existing `coding-standards/release-workflow.md` + LESSON-019 two-commit release pattern). Each per-feature push pollutes downstream `git log` review surface area, scatters rollback semantics (a mis-shipped commit requires revert + forward-fix vs. a tag delete), and confuses contributors who can't easily diff `v0.0.5..v0.0.6` against 80+ post-release micro-commits. Per Spencer's 2026-07-14 meta-policy: **build-freely between releases + push only at release-cuts**.

**(2) Release-prep automation gap** — the existing `scripts/release.py` covers 3 of 8 release-time concerns: tag creation, push, GitHub release notes. The remaining 5 concerns (FID auto-archive, version-bump across 5 anchors, README Status refresh, dead/bloat cleanup, verification gates) are manual, performed inconsistently across release cuts, and prone to silent drift (e.g., forgetting to bump `protocol.config.yaml project.version` while bumping `package.json`; keeping `.tmp-*.txt` files in the working tree at release time per LESSON-029). The audit reveals 5 missing steps + a single 1-command orchestrator would close the gap.

### Expected Behavior

After FID-024 implementation + Spencer ratification + the next release cut:

1. **Between releases**: working tree may carry drift (uncommitted FIDs, in-progress refactors, scratch). No push. No per-feature commit ceremony. LESSON-027 substring-match invariant is the bare-minimum discipline (callable via `pnpm lint:docs`).
2. **At release cut**: Spencer ratifies "ship" → runs `pnpm release:prep X.Y.Z` (1 command) → the orchestrator sweeps + verifies + tags + pushes in atomic lockstep. Manual touches pre-cleared.
3. **Post-release**: `dev/fids/` is empty (auto-archive moved all FIDs); new `## [Unreleased]` section is auto-seeded empty in `CHANGELOG.md`; all 5 version anchors mirror the new release's number; 0 `.tmp-*` files; working tree clean per LESSON-029.
4. **Documentation**: `coding-standards/release-workflow.md` codifies the discipline as the standing rule; future contributors / FIDs / agent sessions inherit the pattern automatically.

### Root Cause

The v0.0.5 release cycle's manual release-prep had several near-miss incidents documented in `dev/LEARNINGS.md` Session 2026-07-14-0400 (LESSON-028 / LESSON-029 / LESSON-030 / LESSON-031). Each LESSON codifies a specific tool-discipline gap. FID-022 closed some of the gaps (`pnpm lint:docs` for LESSON-027; `pnpm release:check` for LESSON-029; `pnpm git:commit` + `pnpm git:tag` for LESSON-030; `pnpm verify:fix` for LESSON-031). The remaining gap is: **no automated composition of all release-prep steps into 1 user command**. Each cycle repeats the same ceremony manually. The manual ceremony's drift-prone steps (FID auto-archive, version-bump, README-refresh, bloat-cleanup) are exactly the steps that compound across cycles and produce post-release hygiene gaps (e.g., the v0.0.4 session-summary-not-shipped gap per LESSON-019).

### Evidence (per the ground-truth audit at `pnpm release:prep` analysis time)

**5 version-bearing files** (all currently at `0.0.5`):
- `VERSION` (single-line, integer-string)
- `package.json` (the `"version"` field; currently `"0.0.5"`)
- `protocol.config.yaml` (`project.version` field under `project:` block; currently `0.0.5`)
- `src-tauri/tauri.conf.json` (the `"version"` field; currently `"0.0.5"`)
- `Cargo.toml` ([workspace.package] version field; currently `0.0.5` — `src-tauri/Cargo.toml` inherits via `version.workspace = true`)

**2 active FIDs** requiring auto-archive at next release cut:
- `dev/fids/FID-2026-07-14-022-lesson-027-doc-drift-linter.md` (Status: `fixed`, awaiting `closed` + archive)
- `dev/fids/FID-2026-07-14-023-post-fid-022-tree-cleanup.md` (Status: `analyzed`, awaiting Spencer ratification → `fixed` → `closed` + archive)

**2 known dead/bloat file candidates** (auto-cleanup targets):
- `./dev/.tmp-fid-022-commit.txt` (LESSON-030 temp file; LESSON-029 cleanup gap if still present)
- `./resources/hermes-agent/infographic/dead-delivery-targets` (filename heuristic: "dead-" prefix indicates stale)

**README.md sections quoting stale state** (auto-refresh targets):
- Status badge line: `[![Status](https://img.shields.io/badge/Status-v0.0.5_Released-...)]` — needs replacement at bump
- "What's New in v<X.Y.Z>" section insertion between previous-version block + footer
- Architecture table status column: `Status (v0.0.4)` row update
- Optional: Verification / Test count / FID count references if present (the "[X] tests" + "[N] FIDs" lines)

**scripts/release.py gaps** (per `grep` audit):
- 0 matches for `dev/fids/archive` (FID auto-archive NOT implemented)
- 0 matches for `VERSION`, `package.json`, `Cargo.toml` etc. in the version-bump context (No automated lockstep bumping)
- 0 matches for `.tmp-`, `cleanup`, `dead-`, `scratch-` (No automated bloat cleanup)
- Existing functionality: tag creation + push + GitHub release notes (excellent foundation; FID-024 extends AROUND this core)

---

## Impact Assessment

### Affected Components

- **`scripts/`** — 5 NEW scripts per Strategy B (FID-022-aligned pattern): `archive-fids.sh` + `bump-version.sh` + `refresh-readme.sh` + `clean-bloat.sh` + `release-prep.sh` (orchestrator). The existing `scripts/release.py` is the downstream target of the orchestrator (no behavioral change to the existing script).
- **`coding-standards/release-workflow.md`** — 1 MODIFIED file: appended new §Checkpoint Release Discipline section (~150 lines) capturing the workflow + the 5-script composition + the policy rationale + the 6 new pnpm script entries.
- **`package.json`** — 1 MODIFIED file: added 5-6 new script entries (`release:prep`, `release:archive`, `release:bump`, `release:readme`, `release:clean`, plus a `release` alias).
- **`CHANGELOG.md`** — modified at release-time (auto-adds new `## [Unreleased]` empty section post-cut; the discover-write cycle is part of `scripts/release-prep.sh` step 5).
- **`dev/fids/`** — auto-depleted at release-time (every FID auto-archives per the orchestrator). The empty state IS the desired post-release state (per `## [Unreleased]` empty placeholder + FID-TEMPLATE §Closed footer).
- **No source-code changes.** Pure tooling + docs + workflow-discipline change. Zero Rust / TypeScript / Cargo.toml behavior changes.

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [ ] High: Major feature broken, no workaround
- [x] Medium: Workflow discipline change + tooling (5 NEW scripts); boundary regression risk if cwd `.tmp-*.txt` files or unarchived FIDs are mishandled, but no behavior break for existing tooling per LESSON-029 forward-validation
- [ ] Low: Cosmetic, or edge case

**Risk mitigation:** each of the 5 NEW scripts is a small (~30-60 LoC each; ~200 LoC for the orchestrator) + idempotent (re-runnable) + dry-run by default; the orchestrator validates 5 version-anchor lockstep equality BEFORE proceeding (mismatch → exit 1, no destructive operations); `scripts/release.py` is the existing trusted foundation, invoked last (after all sweeps succeed), with its existing `--dry-run` flag for pre-emptive testing.

### Risk Comparison vs. v0.0.5 manual cycle

The risk of automating release-prep is lower than the risk of NOT automating it. The manual cycle for v0.0.5 had: 5 separate commits, 2 temp-file cleanup near-misses, 2 verifier false-positives (LESSON-028 evidence), 1 cascade-doc drift incident (LESSON-027 evidence). The automated FID-024 cycle has: dry-run by default, lockstep version equality check, fresh `[Unreleased]` section seed, all tooling idempotent. Net-risk reduction.

---

## Proposed Solution

### Approach

**Bundle the new workflow policy + the 5 NEW scripts into a single FID** — matches the FID-022 pattern (1 commit per work unit, 5 NEW scripts, ~5h implementation budget). Released as part of v0.0.6 (or whatever cycle the active work is ready for). The workflow discipline becomes the standing rule for future cycles via the `coding-standards/release-workflow.md` update.

### Steps

**Step A: `scripts/archive-fids.sh` (NEW; LESSON-019 FID-auto-archive discipline)**

For each `dev/fids/FID-*.md` (active):
1. Read the `**Status:**` field
2. If status ∈ {`analyzed`, `fixed`, `verified`, `closed`} → move `dev/fids/FID-*.md` to `dev/fids/archive/FID-*.md` + update Status to `closed` (no-op for already-closed files; safe flip for non-closed archives)
3. Verify the moved file's §Closed footer copy matches the FID-TEMPLATE standard
4. Exit 0 with summary: `Archived N files`

**Step B: `scripts/bump-version.sh` (NEW; LESSON-019 lockstep discipline)**

Reads target version from CLI arg (`$1`). For each of the 5 version-bearing files:
- `VERSION` → write the target version as a single line
- `package.json` → update the `"version"` field (preserve JSON formatting)
- `protocol.config.yaml` → update `project.version` (preserve YAML structure)
- `src-tauri/tauri.conf.json` → update the `"version"` field (preserve JSON formatting)
- `Cargo.toml` → update `[workspace.package] version` (preserve TOML formatting)

Validate after bump: all 5 files must contain the target version (grep for the new version + count must be 1 in each). Mismatch → exit 1, revert.

**Step C: `scripts/refresh-readme.sh` (NEW; auto-refresh stale state)**

Reads target version from CLI arg. Updates README.md sections:
- Status badge line: `[![Status](https://img.shields.io/badge/Status-vX.Y.Z_Released-...)]` → replace `vX.Y.Z` with new version
- Architecture table row: `Status (vX.Y.Z)` → replace
- (Optional) Verification section test-count line: enumerate `pnpm test` summary + replace

Optionally inserts a "What's New in vX.Y.Z" section based on the CHANGELOG.md `## vX.Y.Z` block extraction.

**Step D: `scripts/clean-bloat.sh` (NEW; LESSON-029 LESSON-030 cleanup)**

Dry-run by default. `--apply` to actually remove. Targets:
- `.tmp-*.txt`, `.tmp-*.md`, `.tmp-*.py`, `.tmp-*.sh` (LESSON-030 patterns)
- `.scratch-*`, `dead-*`, `*.bak` (heuristic)
- `.DS_Store`, `*.swp`, `.idea/` (IDE files)

Filter exclusions: `node_modules/`, `.git/`, `target/`, `.next/`, `src-tauri/target/`.

**Step E: `scripts/release-prep.sh` (NEW; orchestrator)**

Compose Steps A-D + verification gates + `scripts/release.py` invocation:
1. `bash scripts/archive-fids.sh` (Step A)
2. `bash scripts/bump-version.sh $VERSION` (Step B)
3. `bash scripts/refresh-readme.sh $VERSION` (Step C)
4. `bash scripts/clean-bloat.sh --dry-run` then `bash scripts/clean-bloat.sh --apply` (Step D, dry-run + apply pair)
5. Verification gates (sequential, exit-on-fail):
   - `bash scripts/lint-docs.sh` (LESSON-027 invariant)
   - `pnpm lint:ci` (markdown + doc-chain)
   - `pnpm test` (vitest)
   - `bash scripts/release-check.sh $VERSION` (LESSON-029 3-gate)
6. README `[Unreleased]` post-section seed: insert empty `## [Unreleased]\n\n## vX.Y.Z — YYYY-MM-DD\n\n### Added\n...` block (idempotent; only inserts if absent)
7. `python scripts/release.py $VERSION` (existing scripts/release.py with current functionality — tag + push + GitHub release)

**Step F: `package.json` + CHANGELOG.md updates (NEW; pnpm-script wiring)**

Add 5 new pnpm scripts:
- `"release:prep": "bash scripts/release-prep.sh"`
- `"release:archive": "bash scripts/archive-fids.sh"`
- `"release:bump": "bash scripts/bump-version.sh"`
- `"release:readme": "bash scripts/refresh-readme.sh"`
- `"release:clean": "bash scripts/clean-bloat.sh"`
- Plus an alias: `"release": "pnpm release:prep"` (presupposes `$npm_config_version` set)

### Verification (per-script exit-code standard, per LESSON-030 + FID-022 §Verification)

**Per-script unit verification:**
- `bash scripts/archive-fids.sh --dry-run` (NEW) — would archive FID-022 + FID-023 idempotently; expect 0 destructive operations + 0 errors
- `bash scripts/bump-version.sh 0.0.6 --dry-run` — would update 5 anchors; expect lockstep equality assertion passes
- `bash scripts/refresh-readme.sh 0.0.6 --dry-run` — would update Status badge + Architecture table; expect 0 errors
- `bash scripts/clean-bloat.sh --dry-run` — would emit the list of candidates (per the audit: 2 candidates); expect `--apply` to remove them idempotently

**End-to-end orchestrator verification:**
- `bash scripts/release-prep.sh 0.0.6 --dry-run` (NEW orchestrator flag) — full chain dry-runs; expect all 5 sub-steps to validate without destructive operations
- `git status --porcelain | wc -l` after orchestrator dry-run: should equal 0 (idempotency check)
- `pnpm lint:docs` post-orchestrator: should equal 0 (LESSON-027 invariant preserved)
- `bash scripts/release-check.sh 0.0.6` post-orchestrator: should equal 0 (LESSON-029 3-gate + lockstep + transient cleanup)

**Re-grep discipline check (LESSON-031):**
- After step E's verification gates, `grep -nF 'dev/fids/archive' scripts/release.py` — still 0 hits (no behavioral change to the existing release.py; FID-024 composes AROUND, not INTO, the existing tool)

---

## Perfection Loop

### Loop 0 (FID-doc convergence)

**RED:** initial v1 had:
- 1 cross-reference to FID-022 §Resolution using the inline-SHA convention; replaced with `git log --grep='FID-022' --oneline` invariant reference (more durable + amend-friendly)
- 1 §Steps numbering inconsistency (Step G → Step F); renumbered after Step F identification (the `package.json` + `CHANGELOG` updates belong IN §Steps, not §Documentation)
- 1 §Evidence paragraph using the verbatim canonical anchor phrase `'Precedence & `.env` loading'`; replaced with `<canonical anchor phrase>` abstract reference per FID-022 §Loop-0 abstraction discipline (LESSON-026 prevention rule applied)
- 1 chapter-marker drift: "Step C" → "scripts/refresh-readme.sh" double-mentioned in both §Steps Step C and §Package.json §Steps Step F; consolidated under Step F with cross-ref to Step C

**GREEN:** 4 fixes applied (SHA convention → log convention; step renumber; canonical-phrase abstraction; cross-ref consolidation). FID body has 0 anchors of the canonical anchor phrase (per LESSON-027 invariant rigor); bracket+backtick cross-ref syntax uniform across all 8+ sites; §Steps math matches the 5-script budget (5 NEW + 1 MODIFIED doc + 1 MODIFIED package.json; per FID-022 §Approach precedent).

**AUDIT:** markdownlint clean; FID-TEMPLATE 9 sections present + 1 occurrence each; Status:analyzed + Severity:medium preserved; bracket+backtick cross-ref check passes (8+ uniform sites); drift invariant LINT pass: `bash scripts/lint-docs.sh` exits 0 (FID-024 is not in SOURCE_FILES per the LESSON-027 lintscript design + `dev/fids/` is structurally excluded).

**CHANGE DELTA:** ~5% of v1 was rewritten. No regressions to text not affected by the 4 fixes.

## Verifier Pass (2026-07-14 — meta-review of post-Loop-0 state)

**RED (gaps surfaced in this verifier pass):**

1. **5 NEW scripts NOT on disk.** Loop 0's §Steps Step A-E specifies 5 NEW shell scripts (`archive-fids.sh`, `bump-version.sh`, `refresh-readme.sh`, `clean-bloat.sh`, `release-prep.sh`); per the pre-edit baseline audit `git ls-files --error-unmatch scripts/{archive-fids,bump-version,refresh-readme,clean-bloat,release-prep}.sh` returns 0 matches. The 5 scripts exist ONLY in the FID body — implementation has not landed.
2. **§Package.json wiring absent.** §Step F specifies 6 new pnpm scripts (`release:prep`, `release:archive`, `release:bump`, `release:readme`, `release:clean`, plus the `release` alias); current `package.json` has NEITHER `release:prep` NOR `release:archive` (only `lint:docs`/`lint:defer`/`release:check` from FID-022 + FID-026). The wiring is self-referential — the FID recommends scripts that don't yet exist.
3. **§Resolution §Commit/PR `TBD` lacks defer-scope parenthetical.** Per LESSON-038 + Spencer's "defer this docs-only push and continue accumulating" directive, the `TBD` is correct (impl timing separate), but the defer-approval SCOPE should be explicit: docs-only-push defer ≠ implementation defer. Recommend adding a `(defer-scope: docs-only-push only; impl is NOT deferred)` annotation.
4. **§Orchestrator idempotency unproven.** §Step E specifies dry-run by default + lockstep equality check, but no §Verification gate verifies the orchestrator can be run 3× consecutively without divergent state. Recommend a §Idempotency Test Gate (run-N-times → same final state).
5. **§Branch guard missing.** §Step E says orchestrator runs on `git push origin main`, but no `git rev-parse --abbrev-ref HEAD == 'main'` guard exists. The orchestrator would happily run on a feature branch and the bulk FID-archive could disrupt in-progress FIDs.

**GREEN (recommendations for next session, NOT applied):**

1. **`scripts/scaffold-release-tools.sh` (FUTURE FID-029+ candidate)** — adds 5 EMPTY stubs with `#!/usr/bin/env bash` preambles + FID-TEMPLATE-shaped docstrings; subsequent FIDs fill the bodies. Splits stub-impl from body-impl = better reviewer-clarity per LESSON-039 (declared post-hoc).
2. **§Failure-recovery semantics for `bump-version.sh`** — define explicit roll-back behavior for mid-write failure (e.g., if 2 of 5 anchors updated before a write error). Currently says "exit 1, revert" but "revert" is unspecified (per-anchor `git checkout`? atomic temp-file swap?).
3. **§Release branch guard** — add explicit `git rev-parse --abbrev-ref HEAD | grep -Fx 'main'` check in orchestrator's prelude; ABORT with `ERROR: pnpm release:prep must run on main, current branch is X` if not.
4. **§Idempotency Test Gate** — add `orchestrator's §Verification §Idempotency` subsection: spin `bash scripts/release-prep.sh 0.0.6 --dry-run` 3× consecutively + assert `git status --porcelain | wc -l` returns 0 each time + assert the worker scripts produced no side-effects beyond the declared operations.

**AUDIT (this pass, 2026-07-14):**

- Markdownlint clean (manual check)
- LESSON-027 invariant preserved (FID body exempt from 5-anchor invariant)
- LESSON-038 marker-compliant: §Resolution + §Status footer contain the verbatim-quote anchor markers (`Build freely`, `Cut v0.0.6 only when`, `defer this docs-only push`, `NEVER defer without clear approval`) which match PERMIT_REGEX's `verbatim|Spencer\s+|user-explicit` alternation
- `pnpm lint:defer` exit 0 confirmed
- §Status footer explicitly cites LESSON-038 + user's verbatim quotes; defer-extension prohibition codified

**CHANGE DELTA:** ~5% of Loop-0 body (added §Verifier Pass subsection + 2 NEW §Lessons Learned candidates + new §Improvements Missed + new §Questions You Should've Asked).

---

## Resolution

- **Fixed By:** Savant (next session, per Spencer's ratification)
- **Fixed Date:** TBD (deferred per Spencer's 2026-07-14 directive; v0.0.6 cut gate is a ≥1 non-tooling feature batch, not tooling alone)
- **Fix Description:** TBD (per §Steps Step A-F above); impl will execute as part of the v0.0.6 feature batch when one lands — impl is NOT a release-cut trigger on its own
- **User policy references (2026-07-14, verbatim — these are user-explicit, not extensions):**
  - 'Build freely + push only at release checkpoints; don't push every file change.'
  - 'Cut v0.0.6 only when a meaningful feature batch (not just docs) lands — the 5 FID-022 scripts are tooling, not user-facing functionality.'
  - 'Defer this docs-only push and continue accumulating.' (approves defer-of-docs-push only — does NOT extend to FID-024's implementation as a unilateral defer)

  Per LESSON-038 + the session rule 'NEVER defer without clear approval', these quotes do NOT constitute approval for FID-024's implementation to be deferred. Implementation timing is at Spencer's separate discretion. Working-tree accumulation behavior is bounded by Spencer's 'don't push every file change' directive + `coding-standards/release-workflow.md` §Checkpoint Release Discipline (the new §Auto-defer prohibition added 2026-07-14 enforces the rule on the workflow doc side).
- **Tests Added:** Per-script exit-code tests (analogous to FID-022's per-script verification approach; the 5 NEW scripts are mostly idempotent + dry-run defaults, so the verification surface is the dry-run grep pattern)
- **Verified By:** Basher (terminal verification of 5 scripts + orchestrator dry-run + drift invariant) + code-reviewer-minimax-m3 (post-fix review)
- **Commit/PR:** TBD (1 commit per the established 1-commit-per-FID pattern; will land as part of the v0.0.6 release cycle's feature bundle OR independently at Spencer's ratification timing)
- **Archived:** TBD (when this FID's status advances to `closed`; per the FID-TEMPLATE §Closed footer + the new Checkpoint Release Discipline §Checkpoint Release Discipline pattern, it will be auto-archived by `scripts/archive-fids.sh` at the v0.0.6 release cut)

> When status is set to **Closed**, move this file to `dev/fids/archive/` and append an entry to `CHANGELOG.md`.

---

## Lessons Learned

(Captured at FID-plan stage; potential codification when implementation lands.)

- **LESSON-032 candidate — Build-freely + push-at-release cycles preserve history coherence** — Per-feature pushes scatter rollback surface area + bloat `git log` + confuse contributors diffing `v0.0.5..vX.Y.Z`. Codifying the build-freely + push-at-release discipline as a workflow (not as a tooling patch) creates a *standing rule* the project can revert to on demand, vs. a one-off fix. **Pattern:** "I want code review on each feature commit" ≠ "I want incremental pushes" — feature-quality review happens at FID-analysis time (FID-TEMPLATE discipline), not at push time.

- **LESSON-033 candidate — Disciplines are codified best in tooling, not just documentation** — The Checkpoint Release Discipline is documented in `coding-standards/release-workflow.md` (the WHAT), but the orchestrator that ENFORCES the discipline is `scripts/release-prep.sh` (the HOW). Codifying in both forms gives future contributors / agents: (a) the human-readable rule (the doc) for context, AND (b) the automated rule (the orchestrator) for execution. **Anti-pattern:** documenting without tooling (agents drift past docs); tooling without documentation (humans lack context for unusual cases).

- **LESSON-034 candidate — Auto-archive is the canonical FID lifecycle disposition** — Per LESSON-019 status-name hygiene, FIDs advance `created → analyzed → fixed → verified → closed` → `archived`. The `archived` transition is the LEAF state (no further status changes). `scripts/archive-fids.sh` codifies this transition as idempotent + reversible + composable. **Pattern:** status hygiene is enforced by tooling, not by convention; convention drifts. **Tooling makes the discipline un-driftable.**

- **LESSON-043 candidate — Tooling stubs should scaffold separately from impl cycles** — When a FID scopes N NEW scripts (FID-024 here: 5; FID-022: 5), the impl cycle benefits from being split into (a) empty-stub scaffolding (1 commit = N `#!/usr/bin/env bash` files + docstrings + preambles) and (b) body-impl (1 commit per script OR batched). The split pays off when impl stalls on one script while others are GREEN. **Pattern:** keep stub-impl and body-impl as separate commit-units; reviewer-clarity improves; partial-progress is committable; per-script rollback is atomic. Cross-ref: FID-024 §Loop 1 RED item 1 + GREEN item 1.

- **LESSON-044 candidate — Orchestrator scripts need explicit idempotency test gates + dry-run defaults** — Per LESSON-029 + the `clean-bloat.sh` LESSON-030 cleanup discipline, release-prep orchestrators MUST be idempotent (run-N-times → same final state). Without an idempotency test gate, idempotency degrades silently across cycles. **Pattern:** every release-time script gets (a) `--dry-run` default flag, (b) per-step exit-code assert (`set -e` + explicit `|| exit N` per logical step), (c) idempotency test in §Verification that runs N≥3 → checks no divergent state. Cross-ref: FID-024 §Loop 1 RED item 4 + GREEN item 4.

---

## Improvements Missed

Surfaced by this verifier pass; NOT implemented in this FID body update (out of scope per user's "DO NOT CODE" directive — these are FUTURE-FID candidates):

1. **`scripts/scaffold-release-tools.sh` (FUTURE FID-029+ candidate).** See §Loop 1 GREEN item 1 — stub-scaffolding is a TODO that unlocks the rest of FID-024's impl. Benefit: impl can land in N smaller commits (1 per script body) + the orchestrator's impl is a 6th commit that wires them. **Reference:** FID-022's pattern (5 scripts in 1 commit, but each is small + standalone).
2. **`scripts/check-release-prereqs.sh` (FUTURE FID-029+ candidate).** Verify all 6 pnpm scripts are wired into `package.json` + all 5 worker scripts exist + the orchestrator's idempotency test passes — BEFORE the release-cut is attempted. Currently `scripts/release.py`'s pre-flight is local-only (LESSON-029); this would extend the pre-flight to verify the NEW tooling exists.
3. **§Migration Plan for per-feature-push-trained contributors.** For users / agents trained on per-feature-push cadence (the v0.0.5 release-cycle precedent), the build-freely + push-at-release model is unfamiliar. Recommend a §Mental-Model Migration doc + a `docs/build-freely-vs-per-feature.md` note covering the WHY (history clarity + rollback simplicity + reviewer sanity) + the HOW (`pnpm release:prep X.Y.Z`).
4. **§Lockstep Failure Recovery semantics. `bump-version.sh` mid-write failure (currently §Step B says "exit 1, revert")** — recommend explicit revert semantics: `git checkout HEAD~1 -- <per-anchor>` per unwritten anchor + per-written-anchor rollback via temp-file atomic swap pattern. Without this, a network/filesystem mid-write failure leaves the repo in inconsistent state (per `scripts/release.py`'s local-only pre-flight + LESSON-029 forward-validation).
5. **§Order of Operations re-think.** Currently `archive-fids.sh` (Step A) runs BEFORE `bump-version.sh` (Step B). This means: at mid-step failure, some FIDs are moved to archive (and the git-state has moved them) but the version is not bumped. Inconsistent. Recommend: `bump-version.sh` FIRST (commits are reversible on a single file via `git checkout`) + `archive-fids.sh` SECOND (mass file moves are harder to revert). Cross-ref: FID-024 §Loop 1 RED item 4 + GREEN item 4.

---

## Questions You Should've Asked

Surfaced by this verifier pass; recommended for Spencer's next session review pass:

1. **Q:** What if `pnpm release:prep 0.0.6` is accidentally invoked on a feature branch?
   - **Context:** The orchestrator should abort with a clear error, but currently no branch guard exists. Feature-branch invocations could disrupt in-progress FIDs via bulk archive.
   - **Recommended:** Explicit `git rev-parse --abbrev-ref HEAD | grep -Fx 'main'` check + `ERROR: pnpm release:prep must run on main, current branch is X` abort.
   - **Trade-off:** Branch guard adds ~3 LoC to orchestrator prelude; benefit is preventing catastrophic in-progress-FID disruption.
2. **Q:** Should the orchestrator support `--resume-from-step=N`?
   - **Context:** If `clean-bloat.sh --apply` accidentally deletes a critical file mid-run, the orchestrator's current behavior is to abort + leave the partial state. A `--resume-from-step=N` would skip already-completed steps.
   - **Recommended:** Ship `--resume-from-step=N` as FUTURE FID-030+ (NOT in FID-024's primary scope).
   - **Trade-off:** Adds session-state complexity (must persist per-step completion marker); benefit is recoverable-from-mistakes. Trade-off currently weighted against the cost of state-management overhead.
3. **Q:** Is `pnpm release:check` (FID-022 gate) sufficient as the orchestrator's pre-flight, or does the orchestrator need its own gate?
   - **Context:** FID-024 §Step E says the orchestrator runs `release-check.sh` BEFORE `release.py`, but doesn't independently verify the 5 NEW scripts exist on disk.
   - **Recommended:** Orchestrator's pre-flight ALSO checks `(test -x scripts/release-prep.sh) && (test -x scripts/archive-fids.sh) && (test -x scripts/bump-version.sh) && (test -x scripts/refresh-readme.sh) && (test -x scripts/clean-bloat.sh)` before any work. Cross-ref: §Improvements Missed item 2.
   - **Trade-off:** Bundled pre-flight reduces dual-tool surface; orchestrator-own pre-flight is faster (no script delegation). Recommend bundled form for state-coherence.
4. **Q:** Empty `[Unreleased]` at Release-Time — how does auto-seed work?
   - **Context:** Per FID-024 §Step E item 6, the orchestrator inserts empty `## [Unreleased]` AFTER the new `## vX.Y.Z — YYYY-MM-DD` block. But the existing CHANGELOG.md already has `## [Unreleased]` at the top — accumulation risk.
   - **Recommended:** Orchestrator REMOVES the old `## [Unreleased]` (with its prior content) + INSERTS a fresh empty `## [Unreleased]` at the top + the `## vX.Y.Z — YYYY-MM-DD` block immediately after.
   - **Trade-off:** Remove-then-replace preserves clean `[Unreleased]` boundary; append-only is simpler but accrues historical-section bleed.
5. **Q:** `release.py` post-tag-push failure handling?
   - **Context:** If `release.py` succeeds in tag creation + push but fails at the GitHub-release step (e.g., rate-limited), tag is pushed but GH-release is missing.
   - **Recommended:** `--dry-run-finalize` flag (FUTURE FID-030+) that runs all steps EXCEPT `git push` + then a manual ratify. Cross-ref: §Improvements Missed items 1-4.
   - **Trade-off:** `--dry-run-finalize` adds manual ratify step (slower); `--abort-after-push` is more aggressive (refuses tag-push if GH-release fails).

---

## Cross-References

**Cited FIDs:**
- [FID-022](dev/fids/FID-2026-07-14-022-lesson-027-doc-drift-linter.md) — predecessor; introduced 5 NEW scripts; the FID-024 design mirrors FID-022's structural pattern (5 NEW scripts per force-divided concern group + light coupling via orchestrator + `package.json` wiring)
- [FID-023](dev/fids/FID-2026-07-14-023-post-fid-022-tree-cleanup.md) — sibling; scopes the pre-FID-024 drift that this FID's auto-archive step will sweep at the next release cut
- [FID-021](dev/fids/archive/FID-2026-07-13-021-cascade-doc-consolidation.md) — historical reference; codified the LESSON-027 invariant pattern that FID-024 explicitly preserves across the LINT pass

**Cited LESSONs:**
- LESSON-019 — release-only-versioning discipline; the bedrock for the lockstep version-bump across 5 anchors (FID-024 §Step B)
- LESSON-027 — doc-drift substring-match invariant; the between-release floor guarantee (FID-024 §Expected Behavior §1)
- LESSON-028 — field-specific verifier anchors; informs FID-024's lockstep-equality assertion pattern (`grep -E '^version:'` not `grep -E '^\\s*version:'`)
- LESSON-029 — `release.py` pre-flight is local-only; preserved by FID-024's orchestrator which re-invokes `release-check.sh` BEFORE `release.py`
- LESSON-030 — file-based commit/tag pattern; FID-024's orchestrator uses the file-based pattern for the v<X.Y.Z> tag message
- LESSON-031 — verifier should re-grep for ALL occurrences; informs the §Verification §Re-grep discipline check + the §Lessons Learned §LESSON-034 candidate

**Cited session/summary:**
- (Implicit) Spencer's 2026-07-14 directive (this turn's user input) — formalized in §Summary; the originating context for the entire FID

**Reflexive self-reference:**
- This FID's implementation script (`scripts/release-prep.sh`) will, at the v0.0.6 release cut, archive THIS file to `dev/fids/archive/FID-2026-07-14-024-checkpoint-release-discipline.md` via Step A. The FID's own lifespan becomes a canonical example of the new discipline in action.

**Status footer:**
- Status: `analyzed` (FID authored 2026-07-14 per Spencer's 'Open FID-024 + automate release-prep' directive. Spencer EXPLICITLY approved: 'defer this docs-only push and continue accumulating.' This approval covers this FID's docs-only push only. **NOT extended** to FID-024's implementation. Per LESSON-038 + the session rule 'NEVER defer without clear approval', implementation awaiting separate Spencer ratification.)
- Per the LESSON-019 release-only-versioning discipline + the new Checkpoint Release Discipline pattern, this FID will be auto-archived at the v0.0.6 release cut (via `scripts/release-prep.sh` Step A)
- Per the user's 2026-07-14 meta-policy directive, this FID will NOT be auto-pushed between check-points; commits made locally, pushed only at v0.0.6 release (which itself requires a meaningful feature batch)
- Drift invariant preserved at FID-024 author stage (FID bodies are exempt per FID-022 §Loop-0 AUDIT batch + `dev/fids/` is structurally excluded from the LINT pass's SOURCE_FILES list)
- Verifier pass (Loop 1) applied 2026-07-14; impl NOT deferred per LESSON-038 (Spencer's "defer docs-only push" directive covers docs-push only, NOT FID-024's implementation). Implementation timing awaits separate Spencer ratification; the §Resolution §Commit/PR `TBD` is explicit per defer-scope annotation.
