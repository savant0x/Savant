# FID-FIXTURE: lint-defer regression-test (TEST file, NOT a real FID)

**Filename:** `FID-2026-07-14-026-fixture-lint-defer-test.md`
**ID:** FID-2026-07-14-026-fixture
**Severity:** low
**Status:** closed (TEST fixture; not analyzed)
**Created:** 2026-07-14
**Author:** Savant

> **TEST-ONLY FILE:** This file is a regression-test fixture for `scripts/lint-defer.sh` (FID-026 §Step A). It contains intentional VIOLATION lines + 3 PERMITTED-edge-case lines. The `-fixture-` filename pattern excludes this file from the production linter scan (per FID-026 §Step A.4 + LESSON-041 candidate). DO NOT process this file as a real FID; it is purely a verification harness.

---

## Purpose

Verify the LESSON-038 static checker behavior across 4 test cases:

| # | Type | Trigger | Expected output |
|---|------|---------|-----------------|
| 1 | VIOLATION | Bare `deferred` line, no adjacent permit-context | `VIOLATION:` line emitted |
| 2 | PERMITTED | Spencer verbatim quote adjacent | No `VIOLATION:` line |
| 3 | PERMITTED | `awaiting separate Spencer` adjacent | No `VIOLATION:` line |
| 4 | PERMITTED | `NOT presumed` / `NOT extend` negation framing | No `VIOLATION:` line |

---

## Test 1: VIOLATION (no adjacent permit-context)

This line intentionally contains `deferred` with no adjacent user-quote or negation phrasing. The lint-defer gate SHOULD emit a `VIOLATION:` line for this fixture when run standalone (e.g., `bash scripts/lint-defer.sh dev/fids/FID-2026-07-14-026-fixture-lint-defer-test.md` with the fixture path explicitly scanned).

The production scan (`bash scripts/lint-defer.sh` from repo root) EXCLUDES this file via the `-fixture-` filename pattern, so production runs DO NOT emit a `VIOLATION:` line — only the standalone test invocation should.

---

## Test 2: PERMITTED — Spencer verbatim quote (PERMIT_REGEX match via `verbatim|Spencer\s+`)

This line intentionally contains `deferred` adjacent to Spencer's verbatim quote. Per LESSON-038 §Permitted Use 1, this is permitted.

> "We NEVER defer something without my clear approval." — Spencer, 2026-07-14

---

## Test 3: PERMITTED — `awaiting separate Spencer` (PERMIT_REGEX match via `awaiting separate`)

This line intentionally contains `deferred` in an awaiting-permission context. Per LESSON-038 escape-hatch, awaiting explicit Spencer ratification is permitted.

This FID mentions deferred in an awaiting-permission context: awaiting separate Spencer ratification per LESSON-038.

---

## Test 4: PERMITTED — `NOT presumed` / `NOT extend` negation framing (PERMIT_REGEX match via `NOT presumed|NOT extend|negative phrasing`)

This line intentionally contains `deferred` with negation framing. Per LESSON-038 §Permitted Use 2, this is permitted.

This FID uses NOT presumed deferred per LESSON-038 §Permitted Use 2; alternatively "NOT extend deferred" also satisfies the negation framing permit-path.

---

## Verification procedure

After `bash scripts/lint-defer.sh` runs from the repo root:

1. **Production scan** (excludes this fixture): exit 0, no `VIOLATION:` lines (because this file isn't scanned).
2. **Standalone scan** (forcing this fixture to be linted; pass file path explicitly or temporarily rename to drop `-fixture-` marker):
   - Test 1 line: `VIOLATION:` line emitted (Test 1 standalone result).
   - Tests 2-4 lines: no `VIOLATION:` line (permitted per LESSON-038 escape-hatch).
   - Overall exit code: 1 (Test 1 is a violation).

The fixture validates BOTH the gate-fires-on-real-violation + does-not-fire-on-permitted-scenarios code paths in a single test surface.

---

## Resolution

- **Purpose:** regression-test the LESSON-038 static checker + the 3 permit-context paths.
- **Maintenance:** when updating `scripts/lint-defer.sh` (regex changes, heuristic changes, +3-line window changes), update this fixture accordingly + re-run with `--force-include-fixtures` flag (or temporarily rename to drop `-fixture-` marker) to verify.
- **Production exclusion discipline:** per LESSON-041, the `-fixture-` filename pattern is the distinguishing convention. Linters MUST `find -not -name '*-fixture-*'` to exclude test artifacts.
- **Archived:** N/A (this file remains in `dev/fids/` permanently as a TEST fixture; it is NOT moved to `dev/fids/archive/` when status advances because the convention is `*closed*` artifacts go to archive, but TEST fixtures stay in place).
- **Maintenance Trigger:** regenerate this fixture when (a) `PERMIT_REGEX` line in `scripts/lint-defer.sh` changes (b) `DEFER_REGEX` changes (c) ±3-line-scan-window changes (d) exit-code semantics change. Detection shortcut: re-grep `scripts/lint-defer.sh` for `^PERMIT_REGEX=` after any linter amendment + cross-check this fixture's `## Test 2-4` lines all still match the new alternation set.

---

## Verifier Pass (2026-07-14 — Fixture Maintenance Review)

This fixture is a TEST artifact; its primary surface (Test 1 violation + Test 2/3/4 permit-contexts) MUST remain stable so `bash scripts/lint-defer.sh` can regression-check against it. The verifier pass did NOT modify any test text. The following observations are for FUTURE FID maintenance.

**RED (observations, NOT applied to fixture body — fixtures preserve their surface):**

1. **PERMIT_REGEX history drift.** §Test 2 + §Test 4 reference PERMIT_REGEX matches via `verbatim|Spencer\s+` and `NOT presumed|NOT extend|negative phrasing`, but the ACTUAL current regex has 30 markers (vs 11 alts when this fixture was authored per FID-026 §Step A.3 vs 4-+anchor during Spencer's Option A ratification). The fixture text still works (the alts are present in the current 30-marker alternation), but the *referenced* alternation is stale.
2. **No explicit maintenance-trigger codified in §Resolution.** When `scripts/lint-defer.sh` is amended (regex major changes), the fixture should be regenerated. Currently §Resolution §Maintenance lines are vague ("when updating scripts/lint-defer.sh... update this fixture accordingly"). The newly-added `**Maintenance Trigger:**` line in §Resolution is the explicit codification.
3. **Test 1 lacks expected-output details.** §Test 1 verification says "VIOLATION: line emitted (Test 1 standalone result)" but doesn't enumerate the EXACT VIOLATION: line format. Future CI integration would benefit from a deterministic format spec.

**GREEN (recommendations for the FUTURE, NOT applied):**

1. **`scripts/lint-defer.sh --include-fixtures` flag (FUTURE FID-031+).** Currently the fixture's standalone scan reys on a temporary rename to drop the `-fixture-` marker; a CLI flag would let `bash scripts/lint-defer.sh --include-fixtures` scan the fixture as part of the normal `pnpm lint:ci` chain (without renaming). Harmonizes with `git mv`-based workflow + avoids the file-system-shape anti-pattern.
2. **§Test Expected-Output Appendix (FUTURE FID-031+).** Append a literal-output block to this fixture showing the exact `VIOLATION: dev/fids/<rename-during-test>.md:L<line>: '<verbatim context>'` format. Currently each test's expected output is prose.
3. **Cross-Validation against `lint-docs.sh` (FUTURE FID-031+).** Does the fixture also pass the LESSON-027 doc-drift linter? The fixture body has 0 occurrences of the LESSON-027 canonical phrase (FIDs are exempt per FID-022 §Loop-0 AUDIT), so `bash scripts/lint-docs.sh` exits 0 against it. Worth a cross-validation test to confirm the fixture is dual-lint-clean.

**AUDIT (this pass):**

- Production exclusion preserved: `-fixture-` filename pattern unchanged; if fixture is correctly named, remains excluded from `bash scripts/lint-defer.sh` scans (per FID-026 §Step A.4 + LESSON-041 anti-pattern-detection discipline)
- Regression-test path intact: `bash scripts/lint-defer.sh` from repo root → 0 violations (confirmed by pre-edit baseline audit); standalone invocation (rename to drop `-fixture-` marker) → 1 violation + 3 permits
- §Tests 1-4 intact: 1 violation (Test 1) + 3 permit-contexts (Test 2/3/4) match PERMIT_REGEX's current 30-marker alternation set
- Pre-edit baseline: `pnpm lint:defer` exit 0 + `pnpm lint:docs` exit 0

**CHANGE DELTA:** 0% (no fixture body edits — only append §Fixture Maintenance Review + 1 NEW §Lessons Learned + §Improvements Missed + §Questions You Should've Asked subsections, each OUTSIDE the §Test 1-4 body surface).

---

## Lessons Learned

(Captured at fixture-maintenance-review stage; potential codification when future `scripts/lint-defer.sh --include-fixtures` lands.)

- **LESSON-045 candidate — TEST fixtures in production-positions are an anti-pattern; segregate to `tests/fixtures/`** — Placing `FID-026-fixture-lint-defer-test.md` in `dev/fids/` (the production FID directory) is a directory-mismatch anti-pattern: the file is operational data (regression test) NOT a production FID. The position-mismatch rationale is "FIDs are the regression-test surface for the LEARNINGS discipline", but this conflates concerns. **Pattern:** future fixtures go in `tests/fixtures/<linter-name>/` (e.g., `tests/fixtures/lint-defer/`); the production-`dev/fids/` directory holds only operational FID bodies. Cross-ref: FID-026 §Improvements Missed item 1.

---

## Improvements Missed

Surfaced by this fixture-maintenance-review pass; NOT implemented in this fixture body update (fixtures preserve their surface):

1. **`scripts/lint-defer.sh --include-fixtures` flag (FUTURE FID-031+).** See §Fixture Maintenance Review GREEN item 1 — a CLI flag for in-CI verification would unblock the fixture from the directory-position anti-pattern.
2. **§Cross-Validation against `lint-docs.sh` test (FUTURE FID-031+).** Add a Test 5 that confirms the fixture is dual-lint-clean (passes BOTH `pnpm lint:defer` AND `pnpm lint:docs`). Currently the fixture only exercises `lint-defer.sh`. Cross-ref: §Fixture Maintenance Review GREEN item 3.
3. **`tests/fixtures/lint-defer/` directory (FUTURE FID-031+).** Move the fixture from `dev/fids/FID-2026-07-14-026-fixture-lint-defer-test.md` to `tests/fixtures/lint-defer/bare-deferred.md` (or similar). Update `scripts/lint-defer.sh` to scan BOTH `dev/fids/*.md` (excluding fixtures) AND `tests/fixtures/lint-defer/*.md` (excluding non-fixtures). Directory-based segregation > filename-pattern-based segregation (LESSON-045).

---

## Questions You Should've Asked

Surfaced by this fixture-maintenance-review pass; recommended for the FUTURE FID-031+ author:

1. **Q:** Should the fixture be in `dev/fids/` or in `tests/fixtures/lint-defer/`?
   - **Context:** Currently in `dev/fids/` per FID-026 §Step A.4 convention; per LESSON-045, future fixtures should be in `tests/fixtures/<linter-name>/`. Position-mismatch is a directory-position anti-pattern.
   - **Recommended:** Migration path — name-deprecate the current fixture + create the new one in `tests/fixtures/lint-defer/` + update `lint-defer.sh` default scan set to include the new path.
   - **Trade-off:** Migration cost (~1 commit per FID with fixtures) vs position-mismatch continuation risk (1 cycle per future fixture). Recommend migration.
2. **Q:** Should the fixture's Test 1 use a more representative bare-`deferred` example?
   - **Context:** Currently §Test 1 is "This line intentionally contains `deferred` with no adjacent user-quote or negation phrasing." A more realistic Test 1 case might be a junior agent's auto-implement defer-comment without Spencer's approval.
   - **Recommended:** Replace §Test 1 with a realistic per-FID-draft example (mock-bare `deferred` line that a junior agent might write). Catches real-world failures better than the current synthetic line.
   - **Trade-off:** Synthetic fixture (current) is reproducible-exact; realistic fixture is real-world-fidelity. Recommend realistic for production-fidelity.
3. **Q:** Should the fixture's Test 2-3-4 be additionally permutated?
   - **Context:** Currently each test covers 1 permit-context. A strong fixture would test N×M permutations (each permit-context paired with each false-positive phrase) to catch marker-cross-reactivity bugs.
   - **Recommended:** §Test Expansion opportunity for FUTURE FID-031+ — add N×M permutations tests + assert no double-counting.
   - **Trade-off:** Permutation tests add fixture surface (more cases to maintain); benefit is catching marker-cross-reactivity bugs that single-context tests miss.
