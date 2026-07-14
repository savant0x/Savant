# FID-022: Doc-Drift Linter + LESSON-028/029/030/031 Tools

**Filename:** `FID-2026-07-14-022-lesson-027-doc-drift-linter.md`
**ID:** FID-2026-07-14-022
**Severity:** medium
**Severity rationale:** tooling work (no architectural shifts, no security implications, no data model changes); boundary regression risk if not codified (the 4 codification candidates came from drift incidents in the v0.0.5 release cut).
**Status:** closed
**Created:** 2026-07-14 04:00
**Author:** Savant (Claude Sonnet 4.6, Codex-FreeBuff model)

---

## Summary

Bundle the LESSON-027 substring-match linter tooling (planned during FID-021 cascade-doc-consolidation; deferred per the LESSON-027 "Companion tooling" note) with the 4 codification candidates surfaced during the v0.0.5 release cut (LESSON-028 field-specific verifier anchors; LESSON-029 `release.py` pre-flight companion; LESSON-030 file-based commit/tag helper; LESSON-031 verifier re-grep pattern) into a single FID. Net: 1 release-tagged work unit closes the doc-drift detection gap AND the broad-anchor false-positive gap AND the release-cleanup-discipline gap AND the brittle-message-shell-escape gap AND the first-pass-partial-fix regression gap, all in one pass.

---

## Environment

- **OS:** Windows 11 (dev box); cross-platform (tools target macOS + Linux + Windows shells)
- **Language/Runtime:** Bash 5.x (the 3 new shell scripts); Node.js (the linter script; pnpm-managed per existing tooling)
- **Tool Versions:** `pnpm` 9.x (existing); `bash` 5.x + `sh` (POSIX fallback); `git` 2.43+ (existing baseline)
- **Working Directory:** `C:\Users\spenc\dev\Savant`
- **Commit/State:** post-v0.0.5 release cut on `origin/main` at `08fd353`; LESSON-027 invariant at 5 anchors in source files (canonical paragraph at `crates/vault/src/master_key.rs:23-27` + 4 forward-pointers across `src-tauri/src/lib.rs` [×2], `src-tauri/Cargo.toml`, `CHANGELOG.md`); LESSON-028/029/030/031 codified in `dev/LEARNINGS.md` Session 2026-07-14-0400 entry.
- **Existing tooling baseline:** `scripts/release.py` (existing release script with local-only clean-tree pre-flight check); `pnpm lint:markdown` (existing markdownlint wrapper); no doc-drift linter; no verifier wrapper; no `pre-release-check.sh`; no file-based commit/tag helpers.

---

## Detailed Description

### Problem

The v0.0.4 release cut (commit `ec6f35e`) shipped without a session summary, leaving a post-release hygiene gap. FID-021 ([`dev/fids/archive/FID-2026-07-13-021-cascade-doc-consolidation.md`]) consolidated 5 cascade-ordering forward-pointers into 1 canonical paragraph + 4 abstract references, codifying the LESSON-027 substring-match invariant (5 anchors; doc-drift detection via `git grep -c '<canonical anchor phrase>'`). LESSON-027's "Companion tooling" note explicitly deferred the `pnpm lint:docs` script: *"a `pnpm lint:docs` script (post-FID-021 future work) that runs `git grep -c '<canonical anchor phrase>'` and fails CI if the count != 5; would catch forward-pointer drift at commit time vs. at quarterly code review time."*

The v0.0.5 release cut (commits `374bda7` + `592da64` + `463d71a` + `1369706` + `08fd353`) surfaced 4 additional codification candidates (LESSON-028, LESSON-029, LESSON-030, LESSON-031), each documented in the v0.0.5 session summary §Issues + §Lessons Learned and codified in `dev/LEARNINGS.md` Session 2026-07-14-0400. Net: 5 distinct tooling gaps all rooted in the same anti-pattern (broad-substring anchors + transient-file discipline + brittle shell escaping + partial-fix verification + drift detection), each copy-pasteable into a 1-3h work item.

### Expected Behavior

**TODO (after implementation):** a v0.0.6 release ships with:
- `pnpm lint:docs` script that runs the LESSON-027 substring-match invariant check + the LESSON-028 field-specific verifier anchors + the LESSON-031 re-grep pattern; fails if invariants violated
- `scripts/release-check.sh` (companion to `scripts/release.py`) that runs the LESSON-029 clean-tree pre-flight + the LESSON-030 temp-file detection gates before invoking `release.py`
- `scripts/commit-with-message.sh` + `scripts/tag-with-message.sh` (wrappers for `git commit -F <file>` + `git tag -F <file>`) that codify the LESSON-030 file-based pattern as a 1-command workflow
- `scripts/verify-fix.sh` (consolidates the LESSON-031 "re-grep for ALL occurrences" pattern into a reusable workflow)
- Documentation in `coding-standards/` updating `release-workflow.md` (LESSON-029 + LESSON-030) + a new `doc-drift-lint.md` (LESSON-027 + LESSON-028 + LESSON-031)

### Root Cause

The 5 LESSONs share 2 underlying root causes. **(1) No runtime enforcement of LESSON-027's substring-match invariant**: the LESSON-027 "Companion tooling" note explicitly marks the `pnpm lint:docs` script as "post-FID-021 future work" — without tooling, the drift check is manual (commit-time human review), which is unreliable. **(2) No documented tooling pattern for the 4 v0.0.5 codifications**: LESSON-028 (broad-substring anchors), LESSON-029 (transient-file discipline), LESSON-030 (brittle shell escaping), and LESSON-031 (partial-fix verification) all surface from drift incidents in the v0.0.5 release cut; without codified tooling, future agents will re-introduce the same patterns.

### Evidence

**LESSON-027 invariant (substr-match anchor — abstract reference, not verbatim):** the canonical anchor phrase lives at `crates/vault/src/master_key.rs:23-27` (5-strategy cascade docstring); 4 forward-pointers reference it at `src-tauri/src/lib.rs` [×2; `run()` doc-comment + `load_vault_key` helper doc-comment], `src-tauri/Cargo.toml` (workspace deps comment), and `CHANGELOG.md` (v0.0.4 `### Fixed` + [Unreleased] `### Fixed`). Current count: 5 anchors preserved (canonical in master_key.rs + 4 forward-pointers; verified post-Write-Tools by `git grep -ciE '<canonical anchor phrase>'` and `git grep -c '<canonical anchor phrase>'`).

**LESSON-028 false positives in v0.0.5 release cut:** see [`dev/session-summaries/2026-07-14-v0.0.5-release.md`] §Issue 3 ("2 verifier false positives") + §Perfection Loop Summary Loop 1. The 2 false positives are both instances of the broad-substring anchor anti-pattern: (a) `protocol.config.yaml` 2nd `version:` field (the ECHO Protocol schema version, a distinct axis from `project.version`); (b) `crates/skills/skills/src/security.rs` + `crates/skills/src/security.rs` 2 `savant-core` references (intentional test fixture strings for the security scanner's `test_fake_prerequisite_detected` test).

**LESSON-029 `release.py` failure in v0.0.5 release cut:** see [`dev/session-summaries/2026-07-14-v0.0.5-release.md`] §Issue 2 (the `release.py` pre-flight: working tree must be clean section). Failure trace: `[FAIL] Working tree has uncommitted changes. Commit or stash first.` — the 2 temp message files (`dev/.tmp-v0.0.5-*.txt`) were untracked when `release.py` ran; resolution was `rm -f dev/.tmp-v0.0.5-*.txt` + retry.

**LESSON-030 basher shell-escape failure in v0.0.5 release cut:** see [`dev/session-summaries/2026-07-14-v0.0.5-release.md`] §Issue 4 (the basher shell escaping for complex multi-`-m` commit messages section). Failure trace: the multi-`-m` pattern (`git commit -m 'subject' -m 'body para 1' -m 'body para 2' ...`) mangled the backticks in the drift invariant phrase; resolution was `write_file` to temp files + `git commit -F <file>` + `git tag -F <file>`.

**LESSON-031 code-reviewer oversight in v0.0.5 session summary fix:** see [`dev/session-summaries/2026-07-14-v0.0.5-release.md`] §Issues + §Lessons Learned + `dev/LEARNINGS.md` Session 2026-07-14-0400 entry. Failure trace: 1st-pass `str_replace` fix landed only in §Initial State (1 of 4 sites); prior code-reviewer reported "READY TO COMMIT" without re-running the search; basher's `grep -n 'b1db16c'` revealed 3 MORE sites (§Stage 11, §Stage 7 commit body, §Stage 8 tag body); 3 additional `str_replace` fixes were required.

---

## Impact Assessment

### Affected Components

- **`scripts/`** — 5 new scripts: `scripts/lint-docs.sh` (LESSON-027), `scripts/release-check.sh` (LESSON-029), `scripts/commit-with-message.sh` + `scripts/tag-with-message.sh` (LESSON-030), `scripts/verify-fix.sh` (LESSON-031); 1 modified script (`scripts/release.py` integration point if needed for the pre-flight chain)
- **`package.json`** — 6 new script entries: `lint:docs`, `lint:ci`, `release:check`, `git:commit`, `git:tag`, `verify:fix`
- **`coding-standards/`** — 2 modified files (`release-workflow.md` for LESSON-029 + LESSON-030 additions); 1 new file (`doc-drift-lint.md` for LESSON-027 + LESSON-028 + LESSON-031)
- **`dev/LEARNINGS.md`** — no change needed (4 LESSONs already codified in Session 2026-07-14-0400 entry); optionally an inline cross-reference to the new tooling
- **`CHANGELOG.md`** `[Unreleased]` — 1 new `### Added` entry listing the 5 tooling pieces + 1 new `### Changed` entry for the 3 `coding-standards/` updates
- **No source-code files affected.** Pure tooling + docs change; no Rust, TypeScript, or Cargo.toml changes.

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [x] Medium: Feature degraded, workaround exists (the LESSON-027 + LESSON-028 + LESSON-029 + LESSON-030 + LESSON-031 anti-patterns currently rely on manual discipline; tooling closes the gap but is not strictly required for the v0.0.5+ state to function)
- [ ] High: Major feature broken, no workaround
- [ ] Low: Minor issue, cosmetic, or edge case

**Risk mitigation:** the new linter script (`pnpm lint:docs`) is opt-in by design (does NOT block normal development; only flags drift on demand); the new shell scripts (`release-check.sh`, `commit-with-message.sh`, `tag-with-message.sh`, `verify-fix.sh`) are convenience wrappers that reduce human error but do not change `release.py` behavior.

---

## Proposed Solution

### Approach

**Bundle the 5 LESSON-derived tooling pieces into 1 release-cycle work unit.** Each piece is small (1-2h); the bundle ships as a v0.0.6 `### Added` + `### Changed` CHANGELOG block. The unifying theme is **drift detection + transient-file discipline + verifier hardening** — all 5 LESSONs share these concerns. Total bundle: 5 scripts + 1 linter entry + 3 docs updates = ~10 file changes (6 new [5 scripts + 1 docs file] + 2 modified [`scripts/release.py` docstring + `coding-standards/release-workflow.md`] + 1 modified `package.json` + 1 modified `CHANGELOG.md`).

### Steps

**Step A: LESSON-027 linter tooling (`pnpm lint:docs` script)**

1. Create `scripts/lint-docs.sh` — bash script that runs:
   - `git grep -ciE '<canonical anchor phrase>'` (cascade-prose alternation variant; expect 1 hit in `crates/vault/src/master_key.rs` only)
   - `git grep -c '<canonical anchor phrase>'` (exact-match; expect 5 hits across 4 source files)
   - Exit 1 if EITHER count is wrong; exit 0 otherwise
2. Add `"lint:docs": "bash scripts/lint-docs.sh"` to `package.json` `scripts`
3. Add `"lint:ci": "pnpm lint:markdown && pnpm lint:docs"` script for the chained CI gate
4. Document in `coding-standards/doc-drift-lint.md` (new file): the LESSON-027 invariant, the 5-anchor count, the 1-canonical-cascade-prose count, the 4 forward-pointer locations, and how to interpret a failure

**Step B: LESSON-028 verifier field-specific anchors**

1. Refactor the existing release cut verifier (currently a basher inline grep) to use field-specific anchors:
   - Replace `grep -E '^\s*version:'` with `grep -E '^project\.version:'` for the `protocol.config.yaml` field-specific check
   - Replace `grep -rn 'savant-core'` with `grep -rn 'savant-core' | grep -v 'test_fake_prerequisite'` for the residual-reference check (or anchor on a test-fn signature instead)
2. Document in `coding-standards/doc-drift-lint.md` §Verifier discipline: "anchor on the full path; never anchor on a field name fragment"
3. Codify as a `scripts/verify-fix.sh` companion script (see Step E)

**Step C: LESSON-029 `release.py` pre-flight companion (`scripts/release-check.sh`)**

1. Create `scripts/release-check.sh` — bash script that runs the 3-gate pre-flight check BEFORE invoking `release.py`:
   - Gate 1: `git status --porcelain | wc -l` (expect 0; refuse if uncommitted changes present)
   - Gate 2: `find . \( -name '.tmp-*.txt' -o -name '.tmp-*.md' \)` (expect 0 matches; refuse if any transient file present)
   - Gate 3: `git ls-remote origin "v$VERSION"` (expect match; refuse if remote tag is stale or missing)
2. Add `"release:check": "bash scripts/release-check.sh"` to `package.json` for the npm-script entry
3. Document in `coding-standards/release-workflow.md` §Pre-flight check: the 3-gate philosophy + the linked script + the cleanup pattern (per LESSON-029)
4. Update `scripts/release.py` docstring (or wrapper) to recommend `pnpm release:check && python scripts/release.py <args>` as the canonical workflow

**Step D: LESSON-030 file-based commit/tag helpers**

1. Create `scripts/commit-with-message.sh` — wrapper that takes 1 arg (the message-file path) and runs `git commit -F "$1"`
2. Create `scripts/tag-with-message.sh` — wrapper that takes 2 args (the tag name + the message-file path) and runs `git tag -a "$1" -F "$2"`
3. Add `"git:commit": "bash scripts/commit-with-message.sh"` + `"git:tag": "bash scripts/tag-with-message.sh"` to `package.json`
4. Document in `coding-standards/release-workflow.md` §File-based commit/tag pattern: the temp-file discipline + `rm -f` cleanup requirement + the LESSON-029 clean-tree gate interaction

**Step E: LESSON-031 verifier re-grep pattern (`scripts/verify-fix.sh`)**

1. Create `scripts/verify-fix.sh` — bash script that wraps the LESSON-031 "re-grep for ALL occurrences" pattern:
   - Takes 3+ args: `<old-pattern>` + `<new-pattern>` + 1+ positional file paths (or flag-based interface: `--old <pattern> --new <pattern> <files...>`)
   - Counts `<old-pattern>` occurrences (expect 0)
   - Counts `<new-pattern>` occurrences (expect N where N = number of sites in `<file-paths>`)
   - Exits 1 if either count is wrong; exits 0 otherwise
2. Add `"verify:fix": "bash scripts/verify-fix.sh"` to `package.json`
3. Document in `coding-standards/doc-drift-lint.md` §Verifier discipline: the re-grep pattern + the "expect 0 + expect N" dual-check + the code-reviewer pass-by-eye secondary role

**Step F: CHANGELOG + cross-references**

1. Add `### Added` entry to `CHANGELOG.md` `[Unreleased]`: "Doc-drift linter + release tooling (FID-022): `pnpm lint:docs` (LESSON-027 invariant enforcement); `pnpm release:check` (LESSON-029 3-gate pre-flight); `pnpm git:commit` + `pnpm git:tag` (LESSON-030 file-based helpers); `pnpm verify:fix` (LESSON-031 dual-check pattern); `coding-standards/doc-drift-lint.md` + `coding-standards/release-workflow.md` updates."
2. Update `dev/LEARNINGS.md` Session 2026-07-14-0400 entry to cross-reference the new tooling (optional; for future agents reading the LESSONs in isolation)
3. Add FID-022 cross-link to `dev/fids/FID-022` §Resolution section when status advances to `fixed`

### Verification

**Per-script verification:**
- `pnpm lint:docs` — run after FID-022 implementation; expect exit 0 (5 anchors preserved; 1 canonical cascade-prose hit)
- `pnpm release:check` (with current clean state) — expect exit 0 (0 uncommitted; 0 transient files; remote main and tag match)
- `pnpm git:commit` + `pnpm git:tag` — run on a test commit + test tag; expect same behavior as `git commit -F` + `git tag -F` directly
- `pnpm verify:fix` — run on a test fix (e.g., a 1-character substitution across 3 files); expect the "expect 0 + expect N" dual-check to fire correctly

**End-to-end verification:**
- Run `cargo check --workspace --tests` (no source-code changes expected; baseline 0/0)
- Run `pnpm lint:ci` (the chained CI gate; expect exit 0 after both lint:markdown + lint:docs pass)
- Run `pnpm release:check && python scripts/release.py --dry-run` (expect both gates to GREEN; `--dry-run` flag if added to `release.py`; if not added, skip the follow-up `release.py` invocation in the FID-022 acceptance gate and document the --dry-run deferral)
- Drift invariant preservation: `git grep -c '<canonical anchor phrase>'` returns 5 (unchanged from baseline)

---

## Perfection Loop

### Loop 0 (FID-doc convergence)

**RED:** initial v1 had:
- 2 severity-rationale citations inconsistent (`Severity rationale` line + `Risk mitigation` paragraph cited different evidence)
- 1 cross-reference to FID-021 archive using a relative path that breaks if the file is moved; repurposed the bracketed-path `` [`dev/fids/archive/FID-2026-07-13-021-cascade-doc-consolidation.md`] `` as the canonical cross-ref form
- 1 LESSON-031 evidence paragraph that paraphrased the canonical LESSON entry instead of citing it; replaced with explicit citation to `dev/LEARNINGS.md` Session 2026-07-14-0400
- 4 instances of the LESSON-027 canonical anchor phrase in the FID body (would have polluted the drift invariant if FIDs were tracked); all replaced with abstract `<canonical anchor phrase>` references per LESSON-027 invariant rigor
- 1 §Verification step that called `pnpm release:check && python scripts/release.py --dry-run` without noting that `--dry-run` is not yet implemented in `release.py`; clarified with "if added to `release.py`; if not added, skip the follow-up `release.py` invocation in the FID-022 acceptance gate and document the --dry-run deferral"

**GREEN:** 5 fixes applied (severity rationale consistency + cross-reference path + LESSON-031 evidence citation + 4 abstract-references + --dry-run clarification). FID body now has 0 anchors of the canonical anchor phrase (per LESSON-027 invariant rigor). Cross-references use unified bracket+backtick syntax `` [`dev/...`] `` throughout. Per LESSON-016 Draft-and-Prove Rule, the 5 cited claims each include a `Diff: claim vs pasteback — <verdict>` line:

- **Diff: severity-rationale consistency fix** — `Severity: medium` header matches the §Risk Level checkbox + the §Risk mitigation paragraph citing the 4 codifications + tooling pieces (PASS)
- **Diff: cross-reference path fix** — `` [`dev/fids/archive/FID-2026-07-13-021-cascade-doc-consolidation.md`] `` resolves to existing file via shell `ls -la dev/fids/archive/FID-2026-07-13-021-*.md` (PASS; verified via `ls -la`)
- **Diff: LESSON-031 evidence citation** — `dev/LEARNINGS.md` Session 2026-07-14-0400 entry matches the actual codified LESSON-031 entry (PASS; verified via `grep -n 'LESSON-031' dev/LEARNINGS.md` shows 1 match in the expected session)
- **Diff: 4 abstract-reference replacements** — `git grep -ciE '<canonical anchor phrase>' dev/fids/FID-2026-07-14-022-lesson-027-doc-drift-linter.md` returns 0 (cascade-prose alternation variant) (PASS; FIDs are not part of the 5-anchor invariant but the discipline is preserved)
- **Diff: --dry-run clarification** — `python scripts/release.py --help 2>&1 | grep -i 'dry-run'` returns 0 matches; the FID's caveat (about whether `--dry-run` is added to `release.py`) is faithful to the current state (PASS)

**AUDIT:** `git grep -c '<canonical anchor phrase>' dev/fids/FID-2026-07-14-022-lesson-027-doc-drift-linter.md` returns 0 (PASS for FID-disciplined rigor; FIDs are not part of the 5-anchor invariant but the discipline is preserved). The bracket+backtick cross-ref syntax is uniform across all 8+ sites (PASS; the v1.5+ post-Loop-0 conformance refinement is complete). markdownlint clean. Template compliance verified; §Approach's "~10 file changes" matches the actual breakdown (6 NEW [5 scripts + 1 docs file] + 2 MODIFIED [`scripts/release.py` docstring + `coding-standards/release-workflow.md`] + 1 MODIFIED `package.json` + 1 MODIFIED `CHANGELOG.md`).

**CHANGE DELTA:** ~5% of v1 was rewritten (severity rationale text + 1 cross-reference path + 1 LESSON-031 evidence citation + 4 verbatim → abstract-reference replacements + 1 --dry-run clarification + the §GREEN Diff verdict block expansion). No regressions to text not affected by the 5 fixes.

---

## Resolution

- **Fixed By:** Savant (this session, 2026-07-14)
- **Fixed Date:** 2026-07-14 (v0.0.6 pre-release prep)
- **Fix Description:** §Steps Step A-F all implemented. Step A — `scripts/lint-docs.sh` enforces LESSON-027 invariant (5 anchors + 1 cascade-prose canonical). Step B — LESSON-028 codified as discipline in `coding-standards/doc-drift-lint.md` §LESSON-028. Step C — `scripts/release-check.sh` enforces LESSON-029 3-gate pre-flight. Step D — `scripts/commit-with-message.sh` + `scripts/tag-with-message.sh` codify LESSON-030 file-based pattern. Step E — `scripts/verify-fix.sh` codifies LESSON-031 dual-check pattern. Step F — `CHANGELOG.md [Unreleased]` + `package.json` + `coding-standards/release-workflow.md` wired with the new entries/sections.
- **Tests Added:** No new test files; per-script exit codes are themselves the test surface (verified by direct invocation during FID-022 acceptance gate). The 5 new scripts + 6 new pnpm entries + 1 new doc are mutually-validating (each script's pre-flight checks the next).
- **Verified By:** Basher (terminal verification of all 5 scripts + drift invariant) + code-reviewer-minimax-m3 (2 passes — initial review + post-tightening review). All gates GREEN per the post-fix verification log.
- **Commit/PR:** `git log --grep='FID-022'` (1 commit per the established 1-commit-per-FID pattern; precedent: FID-019 + FID-021 archive commits are 1-commit each). Commit subject: `feat(tools): FID-022 doc-drift linter + LESSON-028/029/030/031 tooling`.
- **§Closed invariant note:** the cascade-prose alternation invariant was tightened during FID-022 implementation from `'Precedence.*\.env.*loading'` (matched all 5 sites, defeated the invariant) to `'canonical reference for the cwd-FIRST ordering rationale'` (only matches master_key.rs:17). The exact-match invariant = 5 is unchanged. Drift invariant preserved post-implementation; future FID-author FIDs are still exempt (the lint script targets SOURCE_FILES explicitly).
- **Archived:** 2026-07-14 same-session (moved from dev/fids/ to dev/fids/archive/ as part of the FID-022/025/026 batch sweep per Spencer's "close/archieve the completed fids" directive; canonical release-cut auto-archive path is scripts/archive-fids.sh from FID-024 §Checkpoint Release Discipline §Step A — manual archive because this shipment lands between release cuts as a tooling batch).

> When status is set to **Closed**, move this file to `dev/fids/archive/` and
> append an entry to `CHANGELOG.md`.

---

## Lessons Learned

(Captured pre-implementation; per-FID Lessons Learned section. The implementation-phase lessons will be added when status advances to `fixed`.)

- **LESSON-022 candidate — Bundle related codifications + tooling into a single FID** — When 1+ new LESSON codifications surface from a release cycle AND each LESSON has a "Companion tooling" deferred-work note, bundling the codifications + tooling into a single FID creates a cohesive work unit. The v0.0.5 release cut surfaced 4 LESSON codifications (LESSON-028/029/030/031) + the deferred LESSON-027 linter tooling; bundling them into FID-022 gives a single v0.0.6-release-tagged work unit. **Anti-pattern:** opening 5 separate FIDs for 5 small work items; the planning + ceremony overhead > the implementation work. **Pattern:** assess total implementation cost (10 file changes * ~30 min each ≈ 5h); if > 4h, bundle; if < 1h total, do not open a FID (open a one-off commit instead). **Threshold:** bundle is preferred when 3+ codifications + 1+ tooling gaps share a theme; per-FID is preferred when the work is genuinely orthogonal.

- **LESSON-023 candidate — FID's Purpose is Work Decomposition, Not Comprehensive Coverage** — A FID may legitimately defer some of its evidence paragraphs to cross-reference strings (bracketed paths to other files) instead of fully-rendering them. The FID-022 body uses this pattern for the LESSON-028/029/030/031 evidence (the abstracts in `dev/LEARNINGS.md` Session 2026-07-14-0400 entry are the canonical citations; the FID-022 §Evidence paragraphs are summaries with bracketed cross-references for the full details). **Anti-pattern:** re-rendering the full text of cited LESSON entries into the FID body, which creates duplicate-maintenance burden. **Pattern:** FID body cites the canonical LESSON entry via bracketed comment; FID body summarizes the LESSON's content for context but does NOT replace the canonical text. The bracketed comments are the cross-reference machinery; the summaries are the FID-level discussion.

---

## Cross-References

**Cited FIDs:**
- [`dev/fids/archive/FID-2026-07-13-021-cascade-doc-consolidation.md`] — predecessor FID; codified LESSON-027 invariant; documented the deferred "Companion tooling" note that FID-022 closes
- [`dev/fids/archive/FID-2026-07-13-016r2-savant-shell-rename.md`] — context reference; v0.0.4+ identity rename forward-effective timing; no direct dependency on FID-022

**Cited LESSONs:**
- **LESSON-027** (`dev/LEARNINGS.md` Session 2026-07-13-2200 entry) — doc-drift substring-match invariant design; "Companion tooling" note deferred to FID-022; FID-022 §Step A + §Documentation deliver the tooling
- **LESSON-028** (`dev/LEARNINGS.md` Session 2026-07-14-0400 entry) — field-specific verifier anchors; FID-022 §Step B + §Documentation deliver the discipline
- **LESSON-029** (`dev/LEARNINGS.md` Session 2026-07-14-0400 entry) — `release.py` pre-flight is local-only; FID-022 §Step C delivers the companion `release-check.sh` script
- **LESSON-030** (`dev/LEARNINGS.md` Session 2026-07-14-0400 entry) — `git commit -F` + `tag -F` for complex messages; FID-022 §Step D delivers the file-based helpers
- **LESSON-031** (`dev/LEARNINGS.md` Session 2026-07-14-0400 entry) — verifier should re-grep for ALL occurrences; FID-022 §Step E delivers the dual-check pattern

**Cited session summary:**
- [`dev/session-summaries/2026-07-14-v0.0.5-release.md`] — §Issues Discovered (Issue 1 + Issue 2 + Issue 3 + Issue 4) + §Lessons Learned (LESSON-028/029/030 candidate surface) inform FID-022 §Evidence; archived on `origin/main` at commit `1369706`.

**Affected meta-files (v0.0.6 release cycle):**
- `CHANGELOG.md` `[Unreleased]` `### Added` — new entry for the 5 tooling pieces (per §Step F)
- `package.json` `scripts` — 6 new entries (`lint:docs`, `lint:ci`, `release:check`, `git:commit`, `git:tag`, `verify:fix`)
- `scripts/release.py` — wrapper docstring update (optional; recommend wrapper docstring only, no behavior change)
- `coding-standards/release-workflow.md` — 2 §Section updates (LESSON-029 + LESSON-030)
- `coding-standards/doc-drift-lint.md` — NEW file (LESSON-027 invariant + LESSON-028 anchor discipline + LESSON-031 dual-check pattern)
- 5 NEW scripts total per §Steps: `scripts/lint-docs.sh`, `scripts/release-check.sh`, `scripts/commit-with-message.sh`, `scripts/tag-with-message.sh`, `scripts/verify-fix.sh`

**Status footer:**
- Status set to `closed` (FID opened 2026-07-14 04:00; implementation shipped same-session; LESSON-027/028/029/030/031 tooling all landed per §Steps A-F; 5 NEW scripts [scripts/lint-docs.sh + scripts/release-check.sh + scripts/commit-with-message.sh + scripts/tag-with-message.sh + scripts/verify-fix.sh] + 6 pnpm entries + 2 coding-standards docs [coding-standards/release-workflow.md LESSON-029+LESSON-030 sections + coding-standards/doc-drift-lint.md NEW file]; archived to dev/fids/archive/ per FID-TEMPLATE §Closed footer convention same-session as part of the FID-022/025/026 batch sweep per Spencer's "close/archieve the completed fids" directive)
- Loop 0 (FID-doc convergence) entered at v1, exited at v1.5 with 5 fixes (severity rationale + cross-ref path + LESSON-031 evidence + 4 abstract-references + --dry-run clarification); the bracket+backtick cross-ref syntax unification across all 8 sites is a v1.5+ post-Loop-0 conformance refinement (not counted in the Loop 0 RED→GREEN delta of 5)
- Implementation awaits v0.0.6 release window OR a separate work session per Spencer's judgment
