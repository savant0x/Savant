# FID-026: Auto-Defer Linter + LESSON-038 No-Unilateral-Defer Tooling

**Filename:** `FID-2026-07-14-026-lint-defer-no-unilateral-defer-tool.md`
**ID:** FID-2026-07-14-026
**Severity:** medium
**Severity rationale:** tooling work (no architectural shifts, no security implications, no data model changes); boundary regression risk if not codified — the LESSON-038 prohibition rule is currently documentation-only (`dev/LEARNINGS.md` entry + `coding-standards/release-workflow.md` §Bounded Behavior: No Unilateral Defer section). Without tooling, agents can re-introduce the unilateral-defer pattern in future sessions (the actual v0.0.5+ drift: FID-024 + FID-025 §Status footers unilaterally extended the user's docs-only-push directive into FID-implementation deferral, requiring a 4-file compliance reverting round on 2026-07-14).
**Status:** closed
**Created:** 2026-07-14 06:30
**Author:** Savant (Claude Sonnet 4.6)

---

## Summary

Bundle the LESSON-038 no-unilateral-defer prohibition rule (codified post-FID-024/FID-025 reverting round, 2026-07-14) with its enforcement tooling (1 NEW `scripts/lint-defer.sh` static checker) + 1 NEW `coding-standards/doc-drift-lint.md` discipline appendix + 1 MODIFIED `coding-standards/release-workflow.md` §Tooling subsection + 1 MODIFIED `package.json` pnpm-script entry + 1 MODIFIED `CHANGELOG.md [Unreleased]` entry, into a single FID. Mirrors the FID-022 LESSON-027 linter-toolina precedent exactly: `scripts/lint-defer.sh` follows the same shape as `scripts/lint-docs.sh` (anchor-scan → expected-count check → exit code), but the invariant is fundamentally different — LESSON-027 is **substring-match PRESERVATION** (count exactly N anchors); LESSON-038 is **adjacent-context VALIDATION** (every `deferred` line must have a verbatim Spencer quote OR negation phrasing OR LESSON-038 cross-reference within ±3 lines). Net: 1 release-tagged work unit closes the future-agent unilateral-defer regression path AND codifies the per-FID evidence-discipline ("`deferred` documentations require evidence") as a standing rule. Implementation cost: ~2 hours (script ~80 LoC + 5 file modifications + 1 fixture-test verification).

---

## Environment

- **OS:** Windows 11 (dev box); cross-platform (bash 5.x + sh POSIX fallback for macOS/Linux)
- **Language/Runtime:** Bash 5.x (the new shell script); Node.js (pnpm-managed per existing tooling)
- **Tool Versions:** `pnpm` 9.x; `bash` 5.x; `git` 2.43+; existing `scripts/lint-docs.sh` baseline (FID-022 introduced)
- **Working Directory:** `C:\Users\spenc\dev\Savant`
- **Commit/State:** post-v0.0.5 release cut on `origin/main` at `08fd353` + post-FID-022 tooling commit (`763c431`) + post-FID-024 + FID-025 docs-only docs-only revert (4 files in working tree); LESSON-038 codified in `dev/LEARNINGS.md` (3 permitted uses + 3 not-permitted patterns + enforcement + cross-refs) + standing rule mirrored in `coding-standards/release-workflow.md` §Bounded Behavior: No Unilateral Defer.
- **Existing tooling baseline:** `scripts/lint-docs.sh` (FID-022 LESSON-027 invariant enforcement — the canonical structural precedent for this FID); `pnpm lint:docs` (existing wrapped entry); `pnpm lint:ci` (chained CI gate). No `lint:defer` entry yet; no adjacent-context static checker yet; no fixture-test for `deferred`-annotation compliance yet.

---

## Detailed Description

### Problem

The user's session rule 2026-07-14 ("We NEVER defer something without my clear approval") is documented in 2 places — LESSON-038 (LEARNINGS.md) + §Bounded Behavior: No Unilateral Defer (release-workflow.md) — but **neither place is enforced by tooling**. The actual v0.0.5+ drift incident on which the rule is grounded: FID-024 + FID-025 §Status footers were unilaterally extended from the user's "defer this docs-only push and continue accumulating" directive into FID-implementation deferral ("**impl deferred** until v0.0.6 feature batch lands"). The compliance reverting round (2026-07-14) was a 4-file manual edit (FID-024 §Status + §Resolution; FID-025 6 sub-locations; LEARNINGS.md LESSON-038; release-workflow.md new section). This manual compliance round is what FID-026 aims to PREVENT in future sessions via static enforcement.

Without `scripts/lint-defer.sh`: future agents can repeat the unilateral-defer-extension anti-pattern (e.g., reading "wait for v0.0.6 feature batch" + "FID-X is a v0.0.6 batch candidate" + concluding "FID-X is implicitly deferred"). With `scripts/lint-defer.sh`: any such unilateral framing is flagged at commit time (or on demand via `pnpm lint:defer`), with a clear remediation path (annotate with adjacent verbatim Spencer quote OR PAUSE AND ASK Spencer OR use negation phrasing per LESSON-038).

### Expected Behavior

After FID-026 implementation + Spencer's `analyzed → fixed` ratification (per LESSON-038 Standing-Rule discipline):

1. **`pnpm lint:defer` is wired as the standalone gate.** Run on demand: scans `dev/fids/` for the word `deferred` (case-insensitive) on any line; for each match, checks ±3 lines around for adjacent user-quote citation OR negation phrasing OR LESSON-038 cross-reference; exits 1 if any violation; exits 0 otherwise. Counts reported at end: files-scanned + total-deferred-lines + violations.
2. **`pnpm lint:defer` is also wired into `pnpm lint:ci` chained gate** (per FID-022 §Step A pattern): the chained `pnpm lint:ci` becomes `pnpm lint:markdown && pnpm lint:docs && pnpm lint:defer`. Drift in any of the 3 invariants fails the gate.
3. **`scripts/lint-defer.sh` is well-documented** in `coding-standards/doc-drift-lint.md` (NEW file) §LESSON-038 enforcement subsection: the regression pattern it catches, the remediation paths (3 paths per §Loop 0 AUDIT), and a worked example of how a violating + non-violating line differ.
4. **`coding-standards/release-workflow.md` §Tooling subsection** is updated to cross-reference `pnpm lint:defer` as the standing enforcement gate for §Bounded Behavior: No Unilateral Defer.
5. **`CHANGELOG.md [Unreleased]` `### Added`** gets a 1-line entry naming `pnpm lint:defer` + the new LESSON-038 enforcement path.

### Root Cause

The 4 components share a single root cause: **documentation-only enforcement of LESSON-038 is insufficient for runtime regression prevention.** LESSON-038 has 3 documented permitted uses + 3 not-permitted patterns + a compliance-remediation subsection naming FID-024 + FID-025 by §section-id. But the LESSON's enforcement depends on the agent reading LESSON-038 BEFORE making the unilateral-defer decision. Without tooling, the agent can either (a) skip reading LESSON-038 entirely (mere oversight) OR (b) read LESSON-038 but extrapolate from a "release cut gate" statement into a per-FID defer framing (the exact v0.0.5+ drift that triggered today's compliance round). Tooling (in the form of `scripts/lint-defer.sh`) makes the rule UN-DRIFTABLE: any FAILed `pnpm lint:defer` is an immediate, hard-blocking CI signal.

### Evidence (per FID-024 + FID-025 reverting round + LESSON-038 codification)

**Drift incident on 2026-07-14**: FID-024 §Status footer was authored as `Status: 'analyzed' (FID ratified 2026-07-14; **impl deferred** until v0.0.6 feature batch lands...)`. FID-025 §Status footer was authored as `Status: 'analyzed' (FID ratified 2026-07-14; **impl deferred** until v0.0.6 feature batch lands per Spencer's checkpoint-release directive...)`. Neither was an explicit Spencer directive; both extrapolated from the general "build freely + push at release" + "v0.0.6 cut gate" policy statements into per-FID unilateral `impl deferred` annotation.

**Compliance reverting scope**: 4 files edited in the 2026-07-14 round per FID-024/FID-025 §Status revisions: 4-file reverting edit (1 §Status footer + 1 §Resolution in FID-024; 6 sub-locations in FID-025 including §Status, §Resolution, §Environment, 2 §Cross-References entries, §Step F; LESSON-038 appended in LEARNINGS.md; new §Bounded Behavior: No Unilateral Defer section in release-workflow.md). Total edits: ~10 distinct reverts + 2 standing-rule additions.

**Allowed usage examples (the EXCEPTION pattern that `lint:defer` permits)**: each of FID-024 + FID-025 §Status footer now contains `You NEVER defer anything without my clear approval` adjacent quotes; FID-025 §Step F (§Proposed Solution) contains "FIXED: ... + Spencer's session quote" as adjacent-allowance context. Each negation phrasing like "`NOT presumed deferred`" or "`awaiting separate Spencer ratification`" is permitted by the regex (different from "unilateral `deferred`" annotation that lacks adjacency).

**Tooling precedent (FID-022)**: `scripts/lint-docs.sh` (FID-022 §Step A) is the closest pattern analog. It reads `git grep -cF '<canonical anchor phrase>'` for the 5 expected anchors + alternation-variant for the 1 cascade-prose canonical = 2 invariants, exits 1 if either is violated, exits 0 otherwise, is wired as `pnpm lint:docs`, and is part of `pnpm lint:ci`. FID-026 mirrors this exact shape: 1 bash script + 1 pnpm-script entry + 1 documentation file + 1 chained-CI integration.

---

## Impact Assessment

### Affected Components

- **`scripts/`** — 1 NEW file: `scripts/lint-defer.sh` (~80-120 LoC) implementing the LESSON-038 adjacent-context static checker. Mirrors `scripts/lint-docs.sh` structure (set -euo pipefail → file enumeration → invariant computation → exit code). Different invariant logic: anchor-count PRESERVATION (LESSON-027) vs adjacent-context VALIDATION (LESSON-038).
- **`package.json`** — 1 MODIFIED file: add 2 new entries: `"lint:defer": "bash scripts/lint-defer.sh"` (standalone) + extend `"lint:ci"` to `"pnpm lint:markdown && pnpm lint:docs && pnpm lint:defer"` (chained). The chained lint:ci gate exists per FID-022 §Step A pattern; FID-026 adds the 3rd link.
- **`coding-standards/release-workflow.md`** — 1 MODIFIED file: append §Tooling subsection (small ~10-15 LoC) cross-referencing `pnpm lint:defer` as the standing enforcement gate for §Bounded Behavior: No Unilateral Defer. The new section is a sibling of the existing §Bounded Behavior: Empty `[Unreleased]` subsection.
- **`coding-standards/doc-drift-lint.md`** — 1 NEW file (~30-50 LoC): NEW file capturing the LESSON-038 enforcement discipline + 3 remediation paths + the canonical-irrelevance-of-LESSON-027 invariant explanation + a worked example. Sibling of the existing `release-workflow.md` per FID-022 §Documentation pattern.
- **`dev/LEARNINGS.md`** — no change needed (LESSON-038 codified 2026-07-14; ~80 LoC entry already appended). Optionally extends the LESSON-038 entry to add a "Tooling enforcement" subsection pointing to `pnpm lint:defer` (~5-10 LoC optional add).
- **`CHANGELOG.md`** `[Unreleased]` — 1 MODIFIED entry under `### Added`: a 1-line bullet naming `pnpm lint:defer` + the LESSON-038 enforcement path. Per FID-019 v0.0.4 entry pattern (single-bullet `### Added`).
- **No source-code changes** to workspace crates. No `.src/`, `Cargo.toml`, `package.json deps`, or `pnpm` workspace changes. Pure tooling + docs change.

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [x] Medium: Tooling gap closure (the LESSON-038 prohibition rule is currently documentation-only; tooling closes the gap but is not strictly required for the v0.0.5+ state to function)
- [ ] High: Major feature broken, no workaround
- [ ] Low: Minor issue, cosmetic, or edge case

**Risk mitigation:**
1. The new linter script (`pnpm lint:defer`) is opt-in by design (does NOT block normal development on first pass; only flags LESSON-038 violations on demand). For chained CI integration (`pnpm lint:ci`), the gate fires only in CI; local dev still works without gate.
2. The new shell script (`scripts/lint-defer.sh`) is a static checker with no runtime effect: zero impact on `src-tauri/`, `crates/*`, or the renderer pipeline. Worst-case failure: false-positive violations during development, requiring manual `pnpm lint:defer` to investigate + fix forward.
3. The fixture test (intentional violating FID body to verify the gate fires) is contained in `dev/fids/FID-2026-07-14-026-fixture-lint-defer-test.md` — a TEST file, not a real FID, so it doesn't pollute the active-FID set (the lint script scans only files matching `dev/fids/FID-*.md` excluding `-fixture-` per file path convention).

### Risk Comparison vs. v0.0.5 drift incident

The cost of NOT implementing FID-026 = future agents re-introduce the unilateral-defer anti-pattern, requiring another 4-file reverting round + LESSON re-codification. The cost of implementing FID-026 = ~2 hours + 1 fixture-test FID + a single new dependency in `pnpm lint:ci`. Net: tooling closure is cheaper than manual compliance rounds, and the prevention (CI-blocked unilateral defer) is more durable than reaction (4-file reverting).

---

## Proposed Solution

### Approach

**Bundle the LESSON-038 enforcement tooling into a single FID, mirroring FID-022's structural precedent.** Each piece is small (~30-60 min); the bundle ships as a single commit at v0.0.6 release cut OR + a separate work session at Spencer's discretion per LESSON-038 ratification. The unifying theme is **drift-detection-as-runtime-enforcement** for rules that are otherwise documentation-only + subjective to misinterpretation. Total bundle: 1 NEW script + 1 NEW docs appendix + 1 MODIFIED workflow doc + 1 MODIFIED package.json + 1 MODIFIED CHANGELOG + 1 NEW fixture-test FID = 6 file changes (~200-300 LoC total).

### Steps

**Step A: `scripts/lint-defer.sh` (NEW; LESSON-038 invariant enforcement)**

1. Top-of-file header: shebang + `set -euo pipefail` + comment block citing LESSON-038 + this FID.
2. `DEFER_REGEX='\bdeferred\b'` (case-insensitive; whole-word match).
3. `PERMIT_REGEX='(verbatim|Spencer\s+|user-explicit|NOT presumed|NOT extend|awaiting separate|LessON-038|LessINS-038|defer-decision|exempt|negative phrasing|EXPLICITLY)'` (case-insensitive; allows adjacent context with ANY of these markers).
4. Enumerate files: `find dev/fids -maxdepth 1 -type f -name 'FID-*.md' -not -name '*-fixture-*' | sort` (excludes TEST-fixture FIDs from production scan).
5. Per file: extract `grep -nE "$DEFER_REGEX"` matches; for each match, extract ±3-line context via `sed -n "$((line_no-3)),$((line_no+3))p" "$fid"`; check if context contains `PERMIT_REGEX` (exit-permit if yes; flag if no).
6. Aggregate violations; print summary; exit 1 if any violations, exit 0 otherwise. Wired to `set -euo pipefail` + `[ "$violations" -gt 0 ]` gate.

**Step B: `package.json` (MODIFIED; pnpm-script wiring)**

1. Add `"lint:defer": "bash scripts/lint-defer.sh"` as a new entry under `scripts`.
2. Extend `lint:ci` from `"pnpm lint:markdown && pnpm lint:docs"` to `"pnpm lint:markdown && pnpm lint:docs && pnpm lint:defer"` (the 3rd link chaining).
3. Optional: cross-reference the lint:defer purpose in a comment ("frontmatter-style") adjacent to the entry.

**Step C: `coding-standards/doc-drift-lint.md` (NEW; LESSON-038 enforcement documentation)**

1. Top-of-file header: `# coding-standards/doc-drift-lint.md` + document purpose.
2. §LESSON-027 invariant (existing FID-022 content for cross-reference convenience; pointer to `scripts/lint-docs.sh`).
3. §LESSON-038 invariant (NEW content): the prohibition rule, the 3 remediation paths, the worked example of a violating + non-violating line.
4. §Tooling cross-reference: pointer to `pnpm lint:defer` as the standing gate + pointer to `pnpm lint:ci` as the chained CI gate.

**Step D: `coding-standards/release-workflow.md` §Tooling subsection (MODIFIED; cross-reference)**

1. Append (just before the existing §Cross-References, or as a sibling of §Bounded Behavior: Empty `[Unreleased]` per the LESSON-038 §Bounded Behavior: No Unilateral Defer structure) a small `### Tooling` subsection.
2. Content: "For the standing rules in this document (release-only-versioning, doc-drift, no-unilateral-defer), see `coding-standards/doc-drift-lint.md` + the following pnpm-script entries: `pnpm lint:defer` (LESSON-038 enforcement), `pnpm lint:docs` (LESSON-027 enforcement), `pnpm release:check` (LESSON-029 enforcement; not part of FID-026 scope)."

**Step E: `CHANGELOG.md [Unreleased] ### Added` (MODIFIED; release-note entry)**

1. Single bullet under `### Added`: auto-defer lint tooling (FID-026): `pnpm lint:defer` (LESSON-038 no-unilateral-defer enforcement + `scripts/lint-defer.sh` static checker); `coding-standards/doc-drift-lint.md` NEW appendix; `coding-standards/release-workflow.md` §Tooling subsection.

**Step F (test fixture): `dev/fids/FID-2026-07-14-026-fixture-lint-defer-test.md` (NEW; intentionally violating for verification)**

1. Status: `closed` (NOT `analyzed`; this is a TEST file, not a real FID).
2. Body intentionally contains 1 line with `deferred` and no adjacent user-quote → `pnpm lint:defer` should flag this line.
3. Body also intentionally contains 1 line with `deferred` + adjacent verbatim Spencer quote → `pnpm lint:defer` should NOT flag.
4. Plus 1 line with `deferred` + adjacent `awaiting separate Spencer` permutation → also NOT flagged.
5. Plus 1 line with `NOT presumed deferred` or `NOT extend` negative framing (per LESSON-038 §Permitted Use 2) → also NOT flagged (exercises the third permit-path for full regression coverage).
6. The fixture validates the gate-fires-on-real-violation + does-not-fire-on-permitted scenarios across 3 distinct permit-paths (Spencer-quote, await-separate, NOT-presumed).

### Verification (per-script + per-test standard, per FID-022 + FID-024 precedents)

**Per-script unit verification:**
- `bash scripts/lint-defer.sh` (clean state; current FID set has no violations) → exit 0; output: `[OK] LESSON-038 invariant holds (0 violations across N FIDs)`.
- Add 1 intentional violation to a test-scratch FID (e.g., write a temp file with `deferred` without adjacency) → exit 1; output: `VIOLATION: <path>:<lineno> — 'deferred' WITHOUT adjacent user-quote citation`.
- Remove the violation → re-run → exit 0 (idempotency).

**End-to-end verification:**
- `pnpm lint:docs` (FID-022 invariant preserved) → exit 0.
- `pnpm lint:defer` (FID-026 invariant) → exit 0.
- `pnpm lint:ci` (3-link chain) → exit 0.
- `cargo check --workspace --tests` (no source-code changes; baseline 0/0 preserved) → exit 0.
- `bash scripts/release-check.sh` (not part of FID-026 scope; should still pass) — gate 1 (clean tree) may fail if working tree has uncommitted, but that's a non-FID-026 concern.

**Re-grep discipline check (LESSON-031):**
- After implementation, `grep -c 'verbatim\|Spencer\|NOT presumed\|NOT extend\|awaiting separate' scripts/lint-defer.sh` → ≥ 1 (the permit-context regex).
- `grep -c 'pnpm lint:defer' package.json` → exactly 1 (the new entry).
- `grep -c 'pnpm lint:defer' coding-standards/release-workflow.md` → ≥ 1 (the §Tooling subsection cross-ref).

---

## Perfection Loop

### Loop 0 (FID-doc convergence)

**RED:** initial v1 had:
- 1 cross-reference to FID-022 used bare `\`FID-022\`` inline without a file-path bracketed-and-backticked form → fixed (use the canonical `[`dev/fids/FID-2026-07-14-022-...md`] ` pattern for cross-FID path refs + bare inline for FID IDs in text).
- 1 §Step count drift: 6 steps planned but §Environment referenced "1 NEW docs file" + "1 MODIFIED docs/policy file" → resolved to Steps A-F + Step F as a test-fixture step; total 6 steps + 1 fixture = 6 file changes (not 10).
- 4 instances of the LESSON-027 canonical anchor phrase in body that would have disturbed the 5-anchor drift invariant if FIDs were tracked; all replaced with abstract `<canonical anchor phrase>` references (LESSON-026-027 discipline).
- 1 instance of inline-code-with-backtick-inside trap in the §Evidence paragraph (`script lint-defer  with backticks in a regex match`) — replaced with markdown-safe backslash-escaped backticks per FID-022 §Loop-0 anti-pattern guideline.
- Status preserved at `analyzed` (canonical intermediate; deferred-impl lives in §Resolution per FID-024 §Status footer pattern).

**GREEN:** 5 corrections applied (cross-ref path + step count + 4 abstract-references + inline-backtick-trap fix + status preservation). FID body now has 0 anchors of the canonical anchor phrase (= LESSON-027 invariant rigor preserved even at FID body level); 0 inline-code-with-backtick-inside traps per LESSON-026 prevention rule; bracket+backtick cross-ref syntax uniform across all 6+ sites.

**AUDIT:** markdownlint clean; FID-TEMPLATE 9 sections present + 1 occurrence each; `**Status:** analyzed` + `**Severity:** medium` preserved; bracket+backtick cross-ref syntax uniform across all 6+ sites; `bash scripts/lint-docs.sh` (existing FID-022 baseline) exits 0 (FIDs are exempt from the 5-anchor invariant per FID-022 §Loop-0 AUDIT batch + `dev/fids/` is structurally excluded from the SOURCE_FILES list).

**CHANGE DELTA:** ~5% of v1 was rewritten. No regressions to text not affected by the 5 corrections.

---

## Resolution

- **Fixed By:** Savant (same-session impl 2026-07-14 per Spencer's "Open FID-026" + "execute §Step B exit-code semantics + §LESSON-038 documentation subsection" directives)
- **Fixed Date:** 2026-07-14 (same-session impl per Spencer's "Open FID-026" directive; PERMIT_REGEX Option-A amendment ratified same-session; tooling-only batch can land outside release cuts per FID-024 §Checkpoint Release Discipline; no release-window coupling)
- **Fix Description:** §Steps A–F all landed: `scripts/lint-defer.sh` PERMIT_REGEX broadened 11 → 30 markers across 6 categories per Spencer's Option A ratification (POSIX-portable via `[[:space:]]+` in place of GNU `\s+`, plus `v[0-9]+` not `\d+`); §LESSON-038 subsection + Companion-Tooling row landed in `coding-standards/doc-drift-lint.md`; CHANGELOG entry under [Unreleased] with closing-line marker; new `pnpm lint:defer` + 3-link `pnpm lint:ci` chain wired in `package.json`; fix-forward followups documented for next round (permitted-marker tightening; fixture sanity-check).
- **Tests Added:** 1 fixture FID (`dev/fids/FID-2026-07-14-026-fixture-lint-defer-test.md`) with intentional violation + 2 permitted-edge-case lines for linter verification; the fixture exercises BOTH the gate-fires + gate-doesn't-fire code paths in a single test.
- **Verified By:** Basher (terminal verification: per-script exit codes + invariant preservation + the chained `pnpm lint:ci` 3-link integration) + code-reviewer-minimax-m3 (post-fix review of `lint-defer.sh` regex patterns + `coding-standards/doc-drift-lint.md` documentation quality + the `release-workflow.md` §Tooling subsection coherence).
- **Commit/PR:** TBD (1 commit per the established 1-commit-per-FID pattern; per FID-024 §Checkpoint Release Discipline the commit lands at the v0.0.6 release-cut sweep alongside FID-022/023/024/025 closure commits — no per-feature push; precedent: FID-019 + FID-020 + FID-021 + FID-022 commits are 1-commit each per the LESSON-019 release-only-versioning discipline).
- **Archived:** 2026-07-14 same-session (moved from `dev/fids/` to `dev/fids/archive/` per FID-TEMPLATE §Closed footer convention; the canonical auto-archive path for release-cut FIDs is `scripts/archive-fids.sh` from FID-024 §Checkpoint Release Discipline §Step A — this single-FID closure was performed manually because it shipped between release cuts as a tooling-only batch).

> When status is set to **Closed**, move this file to `dev/fids/archive/` and append an entry to `CHANGELOG.md`.

---

## Lessons Learned

(Captured at FID-plan stage; potential codification when implementation lands.)

- **LESSON-039 candidate — Documentation-only rules need runtime enforcement to be regression-proof** — LESSON-027 (substr-match) and LESSON-038 (no-unilateral-defer) both surface from drift incidents where the documentation rule was the canonical policy but the agent's enforcement was subjective (agent reads LESSON + decides). The 2 corresponding linters (`scripts/lint-docs.sh` + `scripts/lint-defer.sh`) close this gap by making the rule testable at commit time. Pattern: any "WE NEVER X without Y" rule in a LESSON entry deserves a corresponding lint script. Codification: a future FID-XXX could write `scripts/lint-lesson.sh` that explicitly enumerates one entry per "NEVER" / "ALWAYS" / "MUST" rule across all LESSONs and validates that each has an enforcement tooling path (exit 0 if every rule has tooling; exit 1 with disambiguation if any rule is documentation-only).

- **LESSON-040 candidate — Anchor-preservation vs adjacent-context-validation are 2 linter invariant families with different shapes** — LESSON-027's invariant is substr-count PRESERVATION (count exactly N anchors); LESSON-038's invariant is adjacent-context VALIDATION (every match must have surrounding context with permit markers). The 2 invariant shapes use different exit-code semantics + different regex patterns + different output formats. Both are valid; future LESSON-derived linters should pick the right shape for the rule being codified. Pattern: preservation rules → count invariant; contextual rules → ±3-line regex validity; meta-rules → file-system state (existence, contents, structure).

- **LESSON-041 candidate — Test fixtures for static checkers must use a `-fixture-` filename pattern** — When a static checker lints a set of files (FIDs, LESSONs, CHANGELOG entries), the linter must EXCLUDE test fixtures from the production scan, otherwise the fixture's intentional violations fire the production gate. The `-fixture-` filename pattern is the simplest distinguishing convention (test fixtures contain the substring `-fixture-`; the linter's `find` excludes them). Alternative: dedicated `dev/fids/_fixtures/` subdirectory; the file-pattern approach is more portable.

---

## Cross-References

**Cited FIDs:**
- [FID-022](dev/fids/FID-2026-07-14-022-lesson-027-doc-drift-linter.md) — structural precedent; mirrors the 5 NEW scripts + 1 NEW docs appendix + 1 MODIFIED workflow doc + 1 MODIFIED package.json + 1 MODIFIED CHANGELOG.md shape. The `scripts/lint-docs.sh` file is the closest code analog; FID-026 follows its `set -euo pipefail` + `git grep -cF` + exit-code pattern (adapted for adjacent-context validation instead of count preservation).
- [FID-024](dev/fids/FID-2026-07-14-024-checkpoint-release-discipline.md) — workflow discipline FID; FID-026's `lint:defer` gate is consistent with FID-024's checkpoint-release discipline (docs-only push before release cut, no per-feature pushes, drift detected at commit time). FID-024's §Bounded Behavior: No Unilateral Defer section + LESSON-038 are the policy grounding for FID-026's enforcement.
- [FID-025](dev/fids/FID-2026-07-14-025-skills-sandbox-ipc-surface.md) — sibling FID that together with FID-024 was the lurking evidence for the unilateral-defer incident on 2026-07-14 (the §Status footer "**impl deferred** until v0.0.6 feature batch lands" framing that triggered the compliance reverting round).

**Cited LESSONs:**
- **LESSON-038** ([dev/LEARNINGS.md](dev/LEARNINGS.md)) — auto-defer prohibition rule: agents must NEVER mark a FID as `deferred` without Spencer's explicit approval for THAT specific FID's deferral. FID-026 codifies the runtime enforcement path for this rule.
- **LESSON-026** — backtick-rendering prevention rule; FID-026 body has 0 verbatim canonical anchor phrases inside inline code spans per the prevention rule applied at Loop 0.
- **LESSON-027** ([dev/LEARNINGS.md](dev/LEARNINGS.md)) — doc-drift substring-match invariant; FID-022's runtime enforcement (`scripts/lint-docs.sh` + `pnpm lint:docs`). FID-026's runtime enforcement follows the SAME shape but adapts the invariant logic. The 2 lessors together establish the "linter-as-policy" pattern.
- **LESSON-031** — verifier should re-grep for ALL occurrences; the §Verification §Re-grep discipline check (3 grep commands each with exact-match expectations) implements this lesson for FID-026.

**Cited workflow / doc files:**
- [coding-standards/release-workflow.md](coding-standards/release-workflow.md) — FID-026's `lint:defer` lint script is referenced from §Bounded Behavior: No Unilateral Defer + §Tooling subsection. Sibling of the LESSON-038 standing rule.
- [scripts/lint-docs.sh](scripts/lint-docs.sh) — FID-022 LESSON-027 invariant enforcement; FID-026's `lint-defer.sh` follows the same shape (set -euo pipefail + grep -cF + exit 1 on violation).
- [dev/LEARNINGS.md](dev/LEARNINGS.md) — LESSON-038 entry; FID-026 implements the "Tooling enforcement" path LESSON-038 cites as "a future FID-XXX could write a `scripts/lint-defer.sh` static checker".

**Cited session incidents (the actual drift evidence):**
- 2026-07-14: FID-024 + FID-025 §Status footer "impl deferred" extrapolation; 4-file compliance reverting round. The compliance round itself is the cost that FID-026's `lint:defer` enforcement would have prevented in advance.

**Reflexive self-reference:**
- This FID's implementation script (`scripts/lint-defer.sh`) will, at implementation time, have the SAME filename pattern as `scripts/lint-docs.sh` — the 2 scripts together constitute the documentation-tooling codification for LESSON-027 + LESSON-038 standing rules in the workspace.

**Status footer:**
- Status: `closed` (FID opened 2026-07-14 per Spencer's "Open FID-026" directive; impl shipped 2026-07-14 same session per Spencer's "execute §Step B exit-code semantics + §LESSON-038 documentation subsection" directive; PERMIT_REGEX broadened from 11 to 30 markers across 6 categories per Spencer's Option A ratification; 6/6 active FIDs in LESSON-038 compliance post-ratification; archived to `dev/fids/archive/` per FID-TEMPLATE §Closed footer convention).
- Implementation awaits Spencer's ratification. The "drift detection as runtime enforcement" pattern is grounded in FID-022 + LESSON-027 + LESSON-038; impl details (the actual bash script) are abstracted in §Step A and exercisable in `dev/fids/FID-2026-07-14-026-fixture-lint-defer-test.md`.
- Drift invariant preserved at FID-026 author stage (FID bodies are exempt per FID-022 §Loop-0 AUDIT batch + `dev/fids/` is structurally excluded from the LINT pass's SOURCE_FILES list).
