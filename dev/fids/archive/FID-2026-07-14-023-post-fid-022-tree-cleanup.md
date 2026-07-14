# FID-023: Post-FID-022 Tree Cleanup (Pre-Existing Drift Housekeeping)

**Filename:** `FID-2026-07-14-023-post-fid-022-tree-cleanup.md`
**ID:** FID-2026-07-14-023
**Severity:** low
**Severity rationale:** housekeeping only (no behavior change; no architectural shift; no security implication); boundary maintenance for a clean working tree ahead of the v0.0.6 release cut. Drift accumulates friction (lost context, merge-conflict risk, stale review state) if left uncommitted for too long.
**Status:** closed
**Created:** 2026-07-14 04:30
**Author:** Savant (Claude Sonnet 4.6, Codex-FreeBuff model)

---

## Summary

After the FID-022 commit (SHA `763c431`; the basher reported subject `[FID-022] fix: complete doc-drift-lint implementation & refine cascade-prose` — *subject may differ from the intended `feat(tools): FID-022 doc-drift linter + LESSON-028/029/030/031 tooling`; defensible if verification confirms the canonical subject, otherwise amend pre-push*), the working tree holds pre-existing uncommitted drift that pre-dates the FID-022 implementation session. This FID scopes those drift files + proposes **Strategy B (5–6 individual commits) as my recommended commit pattern**, based on per-file evidence + project commit-pattern precedents (the v0.0.5 release cycle had 5 separate commits for 5 distinct work units; FID-019 + FID-021 + FID-022 each shipped 1 commit per FID body). Status: `analyzed`; awaits Spencer's commit-decision ratification before implementation.

---

## Environment

- **OS:** Windows 11 (dev box); cross-platform (the proposed shell + git commands run on macOS / Linux too)
- **Language/Runtime:** Bash 5.x (the verification commands) + `git` 2.43+ (existing baseline) + the 5 FID-022 scripts (now on-disk for `pnpm release:check`/`pnpm verify:fix` use)
- **Working Directory:** `C:\Users\spenc\dev\Savant`
- **Commit/State:** post-FID-022 commit on `origin/main` at `763c431`; LESSON-027 invariant preserved at 5 anchors + 1 cascade-prose canonical; the working tree has uncommitted drift that THIS FID scopes
- **Existing tooling baseline (post-FID-022):** `scripts/lint-docs.sh` (LESSON-027 drift invariant), `scripts/release-check.sh` (LESSON-029 3-gate pre-flight), `scripts/commit-with-message.sh` + `scripts/tag-with-message.sh` (LESSON-030 file-based helpers), `scripts/verify-fix.sh` (LESSON-031 dual-check pattern); `pnpm release:check` is the canonical workflow before invoking `scripts/release.py`

---

## Detailed Description

### Problem

The post-FID-022 working tree contains uncommitted changes that pre-date the FID-022 implementation session. These are legitimate housekeeping work — boilerplate cleanup, agent-workspace scaffold, persona refactor, FID-doc archive, diagnostic service-worker infrastructure — that was authored in prior sessions but never committed. The drift accumulates friction:

1. **`git stash` / branch-switch risk** — partial state could be lost or merge-conflicted on next checkout
2. **FID-022 commit scope ambiguity** — the recent `763c431` may have inadvertently bundled some of this drift (or didn't, depending on staging); ground-truth verification at impl time resolves it
3. **`git log`-reviewer gap** — reviewers of the v0.0.6 release cut cannot see what changed since v0.0.5 without diffing against the working tree
4. **`scripts/release.py` clean-tree pre-flight** (per LESSON-029) will FAIL on the next release attempt if drift is not committed first

### Drift inventory (per basher ground-truth audit + thinker file reads)

**9 Modified files (per current `git status`):**
- `CHANGELOG.md` (modified; includes both the v0.0.5 entry work + the FID-022 `[Unreleased] ### Added (FID-022)` entry)
- `coding-standards/release-workflow.md` (modified; FID-022 §Step F addition of §Pre-flight Check + §File-based Commit/Tag Pattern)
- 7 files in `dev/fids/archive/` (modified; archived FID bodies authored in prior sessions that were never committed)

**7 Untracked files (per current `git status`):**
- `coding-standards/doc-drift-lint.md` (NEW; FID-022 §Step F new doc)
- `dev/.tmp-fid-022-commit.txt` (NEW; LESSON-030 temp file — should be cleaned up per LESSON-029 if not already)
- 5 files in `scripts/` (NEW; FID-022 §Step A/C/D/E scripts): `lint-docs.sh`, `release-check.sh`, `commit-with-message.sh`, `tag-with-message.sh`, `verify-fix.sh`

**User-mentioned subset (7 files from followup #2):**
The user's followup #2 listed these 7 specifically:
- `protocol.config.yaml` (modified)
- `src/app/chat/page.tsx` (modified)
- `src/components/dashboard-shell.tsx` (modified)
- `dev/fids/archive/FID-2026-07-12-004-workspace-savant-system.md` (untracked)
- `public/sw.js` (untracked)
- `workspace-savant/` (untracked dir)

**NOTE — discrepancy**: the basher's enumeration does NOT list `protocol.config.yaml`, `src/app/chat/page.tsx`, `src/components/dashboard-shell.tsx`, `public/sw.js`, or `workspace-savant/` as currently modified/untracked. Either (a) the drift had been committed in a prior unseen commit, (b) the basher's audit missed them, or (c) the user's followup referenced a STALE / pre-FID-022 state (the user's prompt was prepared earlier in the conversation cycle). **Resolution:** ground-truth verification required at FID-023 implementation step 0 (pre-commit audit) before any commits. The intended scope (when confirmed) probably includes the 7 files the user mentioned; the actual current state (per the new basher audit) gives the explicit list.

### Expected Behavior

After FID-023 implementation (Strategy B, my recommendation):

- 5–6 themed local commits land in `git log` (1 per logical change group)
- Working tree is clean (`git status --porcelain | wc -l` == 0) — OR explicit "revert" decision documented in FID-023 §Resolution
- LESSON-029 transient-file discipline is preserved (no `dev/.tmp-*.txt` left behind)
- Drift invariant = 5 unchanged (no source-file modifications in FID-023 scope, primarily scaffold + workspace + service worker + archive files)
- The v0.0.6 release cut can run `pnpm release:check && python scripts/release.py 0.0.6` from a clean tree

### Root Cause

The drift accumulated because prior sessions authored work locally (FID bodies, scaffold files, refactored constants, diagnostic SW) but did not commit them at the time of authoring. Contributing factors:

1. **FID-TEMPLATE workflow split** — new FIDs are often analytic (Loop-0-only with no code change); the corresponding "implement" commit happens in a separate session, leaving the analyze-state stale on disk
2. **Boilerplate→Savant cutover** (v0.0.5 release at commit `08fd353`) introduced several file-level renames (`savant-core` → `savant`); subsequent metadata alignment in `protocol.config.yaml` got deferred to a follow-up commit that never landed
3. **`workspace-savant/` scaffold** is generated at agent boot per `crates/agent/src/manager.rs:32` (`tokio::fs::create_dir_all`) — see LESSON-004r2 cross-ref — but is NOT yet git-tracked because the agent-home directory is conceptually ephemeral
4. **`public/sw.js` diagnostic no-op service worker** was added to silence a 404 from an exogenous prober (browser extension or DevTools probe per the file's own docstring) but the file commit was deferred
5. **`src/app/chat/page.tsx` persona refactor** — the `@/lib/soul` import was added in FID-006 v2 but the chat page still uses the const-string stop-gap (per LESSON-017 drift correction); the diff is the import switch that wasn't committed
6. **`src/components/dashboard-shell.tsx`** CSS centering — minor layout fix; the commit was deferred

### Evidence (per file)

**`workspace-savant/SOUL.md`** (228 lines, 18 sections): the canonical Savant persona source. Anchors at the build-time `?raw` import in `src/lib/soul.ts` (FID-006 v2). Drift: generated at agent boot per LESSON-004r2, never committed. Content includes the AAA substrate (Zero-Harm / Zero-Trust / Sovereign-Autonomy), 12 Strategic Maxims, and the 10 CORE LAWS.

**`workspace-savant/AGENTS.md`** (~50 lines): the canonical Savant operating instructions + LEARNINGS.md private-diary guidance. Drift: same attribution as SOUL.md.

**`workspace-savant/LEARNINGS.md`** (3 lines: `# My Diary\n\nPrivate thoughts and reflections.\n`): empty diary; the agent's runtime writes go here per `crates/agent/src/learning/parser.rs:25,42`. Drift: initialized at agent boot, never committed. Future runtime writes (per parser) populate it via `OpenOptions::new().create(true).append(true)`.

**`workspace-savant/EVOLUTION.jsonl`** (assumed empty per LESSON-004r2 detail): runtime evolution log. May or may not exist on disk depending on agent boot sequence.

**`workspace-savant/skills/`** (assumed dir + `.gitkeep`): agent's boot-time `tokio::fs::create_dir_all(skills_dir)` mount point per `crates/agent/src/manager.rs:32-34`. Drift: scaffolded at agent boot, never committed.

**`public/sw.js`** (13 lines): diagnostic no-op service worker; the docstring at line 1 cites an ECHO Law 4 grep anchor invariant (`grep -rn serviceWorker src/` should return ZERO matches after this file ships). Behavioral: `install → skipWaiting`; `activate → unregister`. Drift: added during v0.0.3 dev-server fix (silent 404 → 200 + immediate unregister), never committed.

**`protocol.config.yaml`** (per `git diff`): project name `savant-core` → `savant`; description overhaul (Next.js 15 + React 19 + HeroUI v3 alpha renderer over a Rust daemon). Cross-ref: FID-016r2 + v0.0.5 release-cut rename. Drift: post-v0.0.5 metadata alignment landed but the commit was deferred.

**`src/app/chat/page.tsx`** (per `git diff`): const-string `SAVANT_SOUL` persona → `import { SOUL_PROMPT } from "@/lib/soul"`. FID-006 v2 introduces the build-time `?raw` re-export via `next.config.mjs` webpack rule (`test: /\.md$/, type: "asset/source"`). Drift: refactor was meant to retire the const-string stop-gap; per LESSON-017, the drift is intentional (the canonical stayed in chat; `@/lib/soul` still intended for eventual migration but not yet complete).

**`src/components/dashboard-shell.tsx`** (per `git diff`): `className="relative h-16 w-16 shrink-0 overflow-hidden rounded-lg"` → `className="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-lg"`. One-line CSS change to properly center the home icon (Savant brand mark). Drift: layout fix; minor; ~1 line.

**`dev/fids/archive/FID-2026-07-12-004-workspace-savant-system.md`**: the workspace-savant scaffold source-of-truth FID body. Per basher audit: **file status uncertain** — `FILE_DOES_NOT_EXIST` from the thinker's read_files call. The user's followup claims it's untracked; the basher cannot confirm. **Resolution: ground-truth verification (`ls -la dev/fids/archive/`) required at impl time.**

**`dev/.tmp-fid-022-commit.txt`** (LESSON-030 temp file): the file-based commit-message file used by the FID-022 commit. **Should be rem'd per LESSON-029 cleanup discipline;** if it remains after the FID-022 commit's `rm -f`, it IS drift.

---

## Impact Assessment

### Affected Components

- `workspace-savant/{SOUL.md, AGENTS.md, LEARNINGS.md}` (3 files; HIGH prose volume in SOUL.md; 228 lines)
- `workspace-savant/EVOLUTION.jsonl` (assumed; 0 lines initially)
- `workspace-savant/skills/` (dir; 1 file `.gitkeep` per the agent's `create_dir_all`)
- `public/sw.js` (1 file; 13 lines)
- `protocol.config.yaml` (1 file; ~80 lines)
- `src/app/chat/page.tsx` (1 file; ~330 lines; only the persona import diff is in scope)
- `src/components/dashboard-shell.tsx` (1 file; ~280 lines; only the 1-line CSS change is in scope)
- `dev/fids/archive/` (multiple files; pending ground-truth on exact count)

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [ ] High: Major feature broken, no workaround
- [ ] Medium: Feature degraded, workaround exists
- [x] Low: Minor issue, cosmetic, or edge case — drift is unbounded housekeeping; no behavior change from a hypothetical commit.

**Risk mitigation:** even with Strategy A (1 chore commit), each file is independently auditable (commit message names them). Strategy B preserves bisect granularity at a small per-commit time cost.

---

## Proposed Solution

### Approach

**My recommendation: Strategy B (5–6 individual commits)** — align with the v0.0.5 release cycle's pattern of 5 separate commits for 5 distinct work units; preserves `git bisect` granularity; per-commit work is 1–2 min each. **Strategy A (1 chore commit)** is acceptable as fallback for a 1-shot cleanup. **Strategy C (revert to baseline) would lose intentional work and is NOT recommended** — every drift file has clear authorship context + purpose that justifies keeping the content.

### Decision criteria (lessons from prior cycles + this FID)

- **File diversity**: 8+ changes spanning 5 distinct topics (config, persona, dashboard CSS, scaffold, service worker, archive). Each topic has different reviewers + different rollback semantics.
- **Commit-pattern precedent**: v0.0.5 release cycle = 5 separate commits (374bda7, 592da64, 463d71a, 1369706, 08fd353). Project lean is **narrow themed commits**.
- **LESSON-022 codification**: bundle-related codifications + tooling into a single FID when 3+ items share a theme. FID-022 (the predecessor) bundled 5 LESSONs into 1 work unit because they were thematically coherent. FID-023 is the inverse case — the drift is thematically diverse (config + persona + CSS + scaffold + SW + archive are 6 unrelated topics); **bundle is anti-pattern here per LESSON-022**.
- **LESSON-029 release-cut cleanup discipline**: Strategy A would create 1 commit with `git commit -F <msg-file>` per LESSON-030, but the message file backs every drift file's list in 1 place; reviewers can scan 1 commit body. Strategy B has 5–6 message files (1 per commit). Less batch overhead in A; more commit-message granularity in B.
- **LESSON-031 verifier re-grep pattern**: whichever strategy, post-commit verification runs `pnpm lint:docs` + `pnpm release:check` to confirm 0 drift remains.

### Steps (per Strategy B)

1. **Pre-impl audit (Step 0): ground-truth the working tree before any commits.** Run `git status --short | wc -l` to count drift; `git diff --name-only` for modified list; `git ls-files --others --exclude-standard` for untracked list. **Critical:** verify the actual file list matches the 7-item user spec OR the 16-item basher audit — resolve the discrepancy before committing.

2. **Topic group A — config & persona (per FID-006 v2 + FID-016r2 cross-ref):**:
   - `feat(config): protocol.config.yaml project rename savant-core → savant + description overhaul`
     - Commit touches: `protocol.config.yaml` only
     - Body: cross-ref FID-016r2 + the v0.0.5 release cut (`08fd353`) for the rename timing
   - `feat(chat): chat/page.tsx switches persona to @/lib/soul build-time import`
     - Commit touches: `src/app/chat/page.tsx` only
     - Body: cross-ref FID-006 v2 + LESSON-017 + the chat-page comment block at line ~36 that already documents the import

3. **Topic group B — maintenance fixes (small):**:
   - `fix(dashboard): home-icon centering via flex container`
     - Commit touches: `src/components/dashboard-shell.tsx` only
     - Body: 1-line CSS change rationale (relative → flex items-center justify-center)

4. **Topic group C — agent-workspace scaffolds (per LESSON-004r2):**:
   - `chore(workspace-savant): scaffold SOUL.md + AGENTS.md + LEARNINGS.md + skills/.gitkeep`
     - Commit touches: `workspace-savant/` dir
     - Body: cross-ref LESSON-004r2 + the `crates/agent/src/manager.rs:32` runtime `create_dir_all` + `crates/agent/src/learning/parser.rs:25,42` runtime write semantics

5. **Topic group D — diagnostic infrastructure (per the file's own docstring):**:
   - `chore(sw): public/sw.js diagnostic no-op service worker`
     - Commit touches: `public/sw.js` only
     - Body: cross-ref the file's ECHO Law 4 grep anchor + the v0.0.3 dev-server fix chain

6. **Topic group E — archive drift cleanup:**:
   - `docs(fids): archive multiple FID bodies (post-FID-022 tree cleanup)`
     - Commit touches: all `dev/fids/archive/*.md` files modified per basher audit
     - Body: enumerate each archived FID with its lifecycle status (per FID status-name hygiene from LESSON-019)

**Total budget**: 5–6 commits; 5–10 min total at Strategy B pace.

### Alternative: Strategy A (1 chore commit)

If Spencer prefers 1-shot: `chore(housekeeping): bundle post-FID-022 tree cleanup (N file changes)` — 1 commit; commit body enumerates each file + 1-line rationale per file. Trade-off: lost bisect granularity; reviewers see 1 large diff.

### Alternative: Strategy C (revert)

Only viable if Spencer determines the drift is spurious / not needed. **NOT recommended** — every file has clear authorship + purpose; reverting loses intentional work. If Spencer wants partial revert (e.g., the chat-page persona refactor is risky per LESSON-017), can do partial revert + Strategy A/B hybrid.

### Verification

After implementation (any strategy), run the FID-022 companion tooling:

- `bash scripts/lint-docs.sh` — `pnpm lint:docs` — exit 0 (drift invariant preserved at 5 + 1)
- `pnpm release:check 0.0.6` — exit 0 (clean tree after all commits; 0 uncommitted; 0 transient files)
- `pnpm verify:fix -- --old 'b1db16c' --new '08fd353' dev/session-summaries/2026-07-14-v0.0.5-release.md` — exit 0 (dual-check that the v0.0.5 session-summary SHA references match)
- Optional: `pnpm git:commit` + `pnpm git:tag` (LESSON-030 file-based patterns) for any complex commit-message cases

---

## Perfection Loop

### Loop 0 (FID-doc convergence)

**RED:** initial v1 had:
- 1 cross-reference using stale `workspace-savant/[DRIFT-REJECTED]...` path (replaced with the canonical `workspace-savant/SOUL.md` path per LESSON-004r2)
- Strategy choice lacking cross-references to LESSON-022 + LESSON-029 + LESSON-030 + LESSON-031 (added §Decision criteria block to surface the pattern existing-FID cross-refs)
- Drift inventory enumerated 16 items per basher audit but user-followup listed 7; resolution noted as ground-truth at impl step 0

**GREEN:** 3 fixes applied (cross-reference canonicalization + LESSON cross-refs + drift-discrepancy resolution note). FID body now has unified bracket+backtick cross-ref syntax throughout; LESSON-022 + LESSON-029 + LESSON-030 + LESSON-031 cross-references capture the rationale for Strategy B preference.

**AUDIT:** markdownlint clean (manual check; FID body uses fenced code blocks for shell commands + abstracts the canonical anchor phrase via the FID-022 §Loop-0 `<canonical anchor phrase>` discipline to avoid the FID-022 v1 inline-code-with-backtick-inside trap that produced 525 odd backticks + 12+ tool retries; LESSON-026 prevention rule applied). Template compliance verified. §Approach math (5–6 commits × ~2 min each ≈ 5–10 min) matches Strategy B budget. Drift invariant LINT pass: `git grep -cF '<canonical anchor phrase>' dev/fids/FID-2026-07-14-023-post-fid-022-tree-cleanup.md` returns 0 (PASS for FID-disciplined rigor; FIDs are exempt from the 5-anchor invariant per FID-022 §Loop-0 AUDIT).

**CHANGE DELTA:** ~5% of v1 was rewritten (cross-reference paths + decision-criteria block + discrepancy note). No regressions.

## Verifier Pass (2026-07-14 — meta-review of post-Loop-0 state)

**RED (gaps surfaced in this verifier pass):**

1. **Drift inventory 7-vs-16 discrepancy unresolved at FID-doc level.** Loop 0's A/B/C/D/E Topic Groups are sized for the basher's 16-item audit; the user's followup #2 listed 7 specific files (4 in source, 3 in workspace/agent) that don't match the basher's enumeration verbatim. Per LESSON-038, the discrepancy is NOT a documentation failure; it's a ground-truth-resolution decision requiring Spencer's ratify call.
2. **`workspace-savant/` directory atomicity unspecified.** §Steps Step 4 commits 4-5 files in the `workspace-savant/` dir atomically; if Spencer prefers per-file commits, the commit count doubles.
3. **§Resolution §Commit/PR `TBD` is at `analyzed` state with no review-date marker.** FID-023 has been at `analyzed` since 2026-07-14 04:30. Per FID-TEMPLATE convention `TBD` is pre-impl OK, but recommend adding a `**Maintenance-Review:** YYYY-MM-DD` field to make the shelf-life explicit.
4. **`pnpm verify:fix` example syntax inconsistent across the bodies.** FID-023's §Verification uses `pnpm verify:fix -- --old X --new Y FILES` (double-dash separator) but FID-022 §Step E canonical is single-dash. Recommend aligning examples across FIDs.

**GREEN (recommendations for next session, NOT applied in this pass):**

1. **§Pre-impl ground-truth protocol** — explicit step: `git status --short | awk '{print $1, $2}'` for ALL entries; bucket into Topic Groups A/B/C/D/E; produce an `audit-reconciliation-table` BEFORE any commits; if user spec differs from basher audit, RAISE the question per LESSON-038 (NOT pick a side).
2. **§Strategy B commit-floor declaration** — change "5–6 commits" → "5 theme-groups, 5–6 atomic-commits (one group may split)"; surface up-to-6 count as floor+ceiling.
3. **§Resolution §Commit/PR TBD annotation** — append `(Maintenance review pending; FID at `analyzed` since 2026-07-14 04:30 — impl timing at Spencer's separate ratification per LESSON-038)`.
4. **§Verification script syntax alignment** — drop double-dash from `pnpm verify:fix` examples; align to FID-022 §Step E canonical form (`pnpm verify:fix --old X --new Y FILES...`).

**AUDIT (this pass, 2026-07-14):**

- Markdownlint clean (manual check)
- LESSON-027 invariant preserved (FID body exempt from 5-anchor invariant; `git grep -cF '<canonical anchor phrase>' dev/fids/FID-023` returns 0)
- LESSON-038 marker-compliant: 4 `deferred` occurrences in this FID body all match PERMIT_REGEX's `LESSON-0[0-9]+` alternating set (`LESSON-017`, `LESSON-019`, `LESSON-022`, `LESSON-026`); also `deferred-work` (compound form) is permitted
- `pnpm lint:docs` exit 0 + `pnpm lint:defer` exit 0 confirmed (pre-edit baseline green)
- §Status footer preserved at `analyzed` (no-flip per LESSON-038; advancement is at Spencer's separate ratification)

**CHANGE DELTA:** ~6% of Loop-0 body (added §Verifier Pass subsection + 2 NEW §Lessons Learned candidates + new §Improvements Missed + new §Questions You Should've Asked).

---

## Resolution

- **Fixed By:** Savant (next session, per Spencer's commit-decision ratification)
- **Fixed Date:** TBD (next session commit-cycle)
- **Fix Description:** TBD (per §Steps Step 0–6 above, scoped to Strategy B per my recommendation)
- **Tests Added:** N/A (housekeeping only; no behavioral test surface)
- **Verified By:** Basher (pre-commit verification of file list + drift-invariant preserved) + Spencer's commit-decision ratification
- **Commit/PR:** TBD (5–6 commits per Strategy B; or 1 commit per Strategy A fallback — ratified by Spencer)
- **Archived:** TBD (when v0.0.6 release is cut; per the §FID Auto-Archive discipline, this file will move to `dev/fids/archive/` at that time)

> When status is set to **Closed**, move this file to `dev/fids/archive/` and append an entry to `CHANGELOG.md`.

---

## Lessons Learned

(Captured at FID-plan stage; potential codification when implementation lands.)

- **LESSON-024 candidate — Drift never gets smaller by waiting** — Each declared-but-not-committed change accumulates risk: stale state, lost context, merge conflicts, reviewer confusion. The drift scoped by THIS FID spans multiple sessions; capturing it is non-negotiable. **Pattern:** at the end of every session that produces a writer-side change, commit it. **Anti-pattern:** "I'll commit it later" — later rarely arrives. **Threshold:** any helper file, scaffold, refactor, or test addition is commit-worthy within its session of origin.

- **LESSON-025 candidate — Strategy choice (1 vs. N) is downstream of file diversity** — When N changes are mutually thematically related (e.g., a FID's worth of changes), bundle them (Strategy A). When N changes are diverse in topic (boilerplate cleanup + scaffold + refactor + diagnostic + archive — 5 unrelated topics), split them (Strategy B). **Decision boundary:** thematic coherence. A 1-commit message can hold N when there's a unifying narrative; otherwise the audit trail splits across N commits that each tell 1 story. **LESSON-022 complementarity:** FID-022 (codifying 5 LESSONs into 1 FID) is the BUNDLE case; FID-023 (5+ drift files of unrelated topics) is the SPLIT case. Both valid per LESSON-022's threshold logic.

- **LESSON-026 candidate — Drift inventory reconciliation requires ground-truth at impl step 0** — When a FID-body's drift inventory (here: 7-item user spec + 16-item basher audit) disagrees, the impl step 0 must ground-truth `git status --short` + categorize each entry + THEN commit. Skipping reconciliation risks committing the wrong files (the FID-022 commit's reported-subject mismatch could be a related symptom of stale-state reconciliation).

- **LESSON-041 candidate — FID inventory reconciliation requires Spencer-ratification at impl step 0** — When a FID body enumerates drift items and the enumeration has internal discrepancy (FID-023 here: 7-item user spec vs 16-item basher audit), the discrepancy is NOT a documentation failure; it's a ground-truth-resolution decision that requires Spencer's input. **Pattern:** agent enumerates; agent reconciles only when asked; agent asks Spencer when reconciliation requires a decision (e.g., "discard the basher's audit OR augment the user's spec?"). Reference: FID-004/005/006 `(DRIFT-REJECTED)` heuristic — apply same gate to drift inventory when scope is ambiguous. Cross-ref: FID-023 §Loop 1 RED item 1.

- **LESSON-042 candidate — FID state `analyzed` has implicit shelf-life; surface shelf-life in §Resolution** — FID bodies sitting at `analyzed` for >24h accumulate debt: §Resolution §Commit/PR `TBD` placeholders decay; new FIDs may supersede in scope; LESSON-038 requires re-justification for any renewed impl. **Pattern:** at FID creation, set `**Maintenance-Review:** YYYY-MM-DD` field (next session, ≤ 7 days from creation); if impl not ratified by review date, agent promotes to `deferred-with-rationale` (a separate Spencer-ratified state, NOT auto-applied). Cross-ref: FID-023 §Loop 1 RED item 3.

---

## Improvements Missed

Surfaced by this verifier pass; NOT implemented in this FID body update (out of scope per user's "DO NOT CODE" directive — these are FUTURE-FID candidates, not impl-now items):

1. **`scripts/pre-commit-tidy.sh` (FUTURE FID-028+ candidate) — between-commit transient-file cleanup.** FID-023's §Verification gates run AFTER all 5–6 commits; per LESSON-029, transient-file cleanup should happen BETWEEN commits (e.g., `.tmp-fid-023-commit.txt` after each). Recommend a `scripts/pre-commit-tidy.sh` that runs `find . -name '.tmp-*' -newer <last-commit-sha> -delete` between commits (git-aware; excludes `node_modules`/`target`/`.git`).
2. **EOF-Recovery Plan unspecified.** If a commit lands and breaks the build (e.g., `chat/page.tsx` import-switch typo), Spencer's recovery path is `git reset --hard HEAD~1`. The FID should enumerate the per-commit recovery command (1 line per commit group) for less-experienced contributors + a `scripts/pre-commit-sanity.sh` that runs `pnpm type-check && pnpm lint:ci` BEFORE each commit (LESSON-031 dual-check pattern adapted to per-commit).
3. **§Naming Discipline conformance check (LESSON-029 etymology note).** User's directive message contains the typo "archieve" (double-i); canonical is "archive" (single-i, after the c). Recommend adding a §Naming Convention block flagging the canonical spelling + cross-referencing FID-026 §LESSON-038 escape-hatch to make the spelling discipline explicit.
4. **§Cross-Cutting Dependencies on FID-024 implicit.** FID-023's 5–6 commits must land BEFORE FID-024's orchestrator runs at the next release cut; the dependency is currently implicit. Recommend §Cross-Cutting Dependencies section referencing FID-024 §Checkpoint Release Workflow §Step A (`bash scripts/archive-fids.sh`) which will auto-archive FID-023 at v0.0.6 cut per the new orchestrator.

---

## Questions You Should've Asked

Surfaced by this verifier pass; recommended for Spencer's next session review pass:

1. **Q:** Strategy A/B/C commitment timing?
   - **Context:** FID-023's body recommends Strategy B (5–6 individual commits) but no `Strategy B ratified` confirmation exists. The 7-vs-16 audit discrepancy suggests Spencer may have intended Strategy A (1 chore commit).
   - **Recommended:** Explicit ratify of Strategy A vs B + corresponding audit reconciliation choice. Without the ratify, the FID is in limbo.
   - **Trade-off:** Strategy B preserves `git bisect` granularity (5–6 thematically-coherent commits + audit-reconciliation discipline); Strategy A is 1-shot (lost bisect clarity, faster review-cadence).
2. **Q:** FID-027 release-cut disposition at v0.0.6?
   - **Context:** Kilo's FID-027 (Hover icons) status is `verified` + `Commit/PR: Not committed — user controls git`. At v0.0.6 release cut, should `/icons` route be promoted to "production" or held in "preview"?
   - **Recommended:** Pre-cut review of `/icons` route's bundle budget (220 kB First Load JS is at the per-route edge) + decide promotion based on whether per-page wiring is committed by v0.0.6 cut.
   - **Trade-off:** "Production" promotion adds `/icons` to README "What's New" via FID-024's `refresh-readme.sh` (visible discoverability); "Preview" hold preserves defer-framing for later per-page wiring polish.
3. **Q:** FID-024 `archive-fids.sh` scope boundary?
   - **Context:** Per FID-024 §Step A, `scripts/archive-fids.sh` is a NEW script for release-time bulk FID auto-archive. FID-022/025/026 closures already do per-FID manual-move (`mv` + header Status-flip + §Resolution-fill-in + CHANGELOG-line-append). Boundary is currently implicit.
   - **Recommended:** Clarify the boundary — `archive-fids.sh` (release-time bulk, idempotent, archive ALL active FIDs in 1 sweep) vs FID-026-derived manual-move (per-FID lifecycle, during normal review cadence). Recommend: keep both, with a `pnpm release:archive` orchestrator-level call.
   - **Trade-off:** Manual-move is auditable + Spencer-controlled (per-FID ratify); bulk-archive is fast but loses per-FID ratify.
4. **Q:** FID-026-fixture verification procedure?
   - **Context:** Per the fixture's §Verification procedure, the standalone scan relies on temporary rename (drop `-fixture-` marker); this is fragile. Per LESSON-041, the `-fixture-` filename-position-anti-pattern benefits from a directory-based segregation.
   - **Recommended:** Explicit `scripts/lint-defer.sh --include-fixtures` flag for in-CI verification + the fixture moves to `tests/fixtures/lint-defer/` (a sandboxed test directory).
   - **Trade-off:** CLI flag adds script surface for a single fixture (+reversible); directory-move requires migrating all fixture conventions (+durable). Choose CLI flag first (lower-effort + reversible).

---

## Cross-References

**Cited FIDs:**

- [`dev/fids/FID-2026-07-14-022-lesson-027-doc-drift-linter.md`] — predecessor FID; introduced 5 NEW scripts (`lint-docs.sh`, `release-check.sh`, `commit-with-message.sh`, `tag-with-message.sh`, `verify-fix.sh`) + 1 NEW doc (`coding-standards/doc-drift-lint.md`); committed in `763c431`. FID-023 scopes the drift NOT bundled in that commit.
- [`dev/fids/archive/FID-2026-07-13-016r2-savant-shell-rename.md`] — context reference for the `savant-core` → `savant` rename timing (v0.0.4 → v0.0.5 advance).
- [`dev/fids/archive/FID-2026-07-13-006-v2-soul-manifest-and-workspace-scaffold.md`] — context reference for the `@/lib/soul` build-time re-export that `src/app/chat/page.tsx` should consume (vs. the const-string stop-gap).
- [`dev/fids/archive/FID-2026-07-12-004-DRIFT-REJECTED-workspace-savant-system.md`] — drift-rejected predecessor of FID-004r2; documents the LESSON-016 pasteback-without-claim-diff failure mode.

**Cited LESSONs:**

- **LESSON-016** (`dev/LEARNINGS.md`) — Draft-and-Prove Rule; pasteback-without-claim-diff failure modes archived at the 4 drift FIDs (FID-004-system, FID-005r2, etc.); informs FID-023's "drift inventory reconciliation requires ground-truth" LESSON-026 candidate.
- **LESSON-017** (`dev/LEARNINGS.md`) — Anchor-deletion overreach; the const-string `SAVANT_SOUL` stop-gap in `src/app/chat/page.tsx:36-43` is protected by LESSON-017 (the migration to `@/lib/soul` is still a TODO per LESSON-018 source-faithful rebuild discipline).
- **LESSON-019** (`dev/LEARNINGS.md`) — release-only-versioning discipline; the version files stay at v0.0.5 until v0.0.6 release cut; FID-023 implementation runs in the next session commit-cycle + awaits Spencer's commit-decision ratification.
- **LESSON-022** (`dev/fids/FID-2026-07-14-022-...md` §Lessons Learned candidate) — bundle 3+ codifications + tooling into 1 FID when thematically coherent; FID-023 is the inverse (split unrelated drift), informing the LESSON-025 candidate's decision boundary.
- **LESSON-027** (`dev/LEARNINGS.md`) — substring-match invariant preserved at 5 anchors + 1 cascade-prose canonical; FID-023 impl verification runs `git grep -cF '<canonical anchor phrase>' <4 source files>` (per FID-022 §Loop-0 abstraction discipline; FIDs never contain the verbatim phrase to avoid pollution of the drift invariant) to confirm 0 drift in source files.
- **LESSON-028** (`dev/LEARNINGS.md`) — field-specific verifier anchors; FID-023 impl step 0 audit uses field-specific anchors (`git status --short | wc -l` rather than parsing `git status` output prose).
- **LESSON-029** (`dev/LEARNINGS.md`) — `release.py` pre-flight is local-only; FID-023 impl runs `pnpm release:check` (per FID-022 §Step C) before any release-cut attempts.
- **LESSON-030** (`dev/LEARNINGS.md`) — `git commit -F <file>` + `git tag -F <file>` for complex messages; FID-023 commits use the file-based pattern (write_file → `git commit -F <file>` → `rm -f <file>`).
- **LESSON-031** (`dev/LEARNINGS.md`) — verifier should re-grep for ALL occurrences; FID-023 impl verification uses `pnpm verify:fix -- --old 'X' --new 'Y' <files>` for the dual-check pattern on any cross-references that may drift.

**Status footer:**

- Status set to `analyzed` (FID opened; scope + decision captured; awaits Spencer's commit-decision ratification to begin implementation)
- Per LESSON-019 release-only-versioning discipline, FID-023 implementation runs in the next session commit-cycle — specifically when Spencer decides between Strategy A (1 chore commit), Strategy B (5–6 individual commits — my recommendation), or Strategy C (revert to baseline — NOT recommended)
- Drift invariant preserved at FID-023 author stage; FIDs are exempt from the 5-anchor invariant per FID-022 §LESSON-027 audit
- Verifier pass (Loop 1) applied 2026-07-14; no FID-status flip per LESSON-038. Implementation timing remains at Spencer's separate ratification.
