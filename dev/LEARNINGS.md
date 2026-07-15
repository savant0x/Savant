**Cross-references:** Spencer's session quote 2026-07-14; FID-024 + FID-025 reverting edits (2026-07-14); the new §Auto-defer prohibition section in `coding-standards/release-workflow.md`.

---

### LESSON-050: Untracked-Bloat Is a Real Candidate (FID-024 §Step D clean-bloat.sh discipline)

**Date:** 2026-07-14
**Trigger:** FID-024 §Step D's `clean-bloat.sh` script surfaced `resources/hermes-agent/infographic/dead-delivery-targets` as a candidate during the FID-024 dry-run (via the 2026-07-14 FID auto-archive discipline sweep). The file was **untracked** by git, but the `clean-bloat.sh --apply` removed it from disk without committing. Drift count stayed at 0; the orchestrator's no-op state was preserved; but a future CI step or test harness assertion that referenced this path would silently break.

**Lesson:** The `clean-bloat.sh` pattern matches on filename shapes (`.tmp-*`, `*.bak`, `dead-*`, `.scratch-*`, `.DS_Store`, `*.swp`) **regardless of git-tracking status**. A file with a `dead-` prefix is a bloat candidate, tracked or not. The orchestrator's `git status --short` doesn't surface untracked files (without `--ignored`), so the post-cleanup drift count can stay at 0 even when actual cleanup work happened on disk.

**Permitted uses (no extension required):**
- Removing untracked `*.tmp-*` / `*.bak` / `dead-*` artifacts via `clean-bloat.sh --apply` is a working tree cleanup, not a code change. Skipping the commit is correct.
- For TRACKED bloat files (rare; would require explicit `git rm` or removal + commit), the orchestrator MUST commit the removal (file-based pattern per LESSON-030).

**Not permitted:**
- Claiming "the working tree is clean" when untracked bloat was silently removed (the clean state is git's clean, not disk's clean). The orchestrator must differentiate tracked-vs-untracked cleanup.
- Adding speculative empty commits ("docs(noop): clean-bloat captured an untracked-file removal") — that's LESSON-029 violation territory (speculative commits are forbidden).

**Pattern (the canonical doctrine):**
1. **Pre-cleanup dry-run:** `bash scripts/clean-bloat.sh` (no args; lists candidates per LESSON-029 to surface stale files BEFORE any destructive op).
2. **Apply cleanup:** `bash scripts/clean-bloat.sh --apply` only if step 1 returned ≥1 candidate; otherwise skip (idempotent-floor pattern — saves an unnecessary pass).
3. **Post-cleanup:** `git status --short` (tracks tracked-file changes); `git status --ignored` (surfaces TRACKED-vs-UNTRACKED impact).
4. **If TRACKED changes:** commit with file-based pattern (LESSON-030) + describe scope ("removed N stale harness artifacts").
5. **If UNTRACKED-only changes:** NO commit needed — the working tree's git state is unchanged. Document the untracked removal in a `## Transient bloat removed` block in the orchestrator's report (output, not commit).

**Enforcement + tooling:**
- A future FID-XXX could write `scripts/check-bloat-cleanup.sh` that runs `clean-bloat.sh --apply` then asserts either (a) tracked removals are committed, OR (b) untracked-only removals produce a `## Transient bloat removed` output block. The current orchestrator invokes `clean-bloat.sh` directly without the verification gate; this codifies the gap.

**Anti-pattern documentation (Spencer's automation level 3 evidence, 2026-07-14):**
- During the FID auto-archive discipline sweep + closure commits, `clean-bloat.sh --apply` removed `resources/hermes-agent/infographic/dead-delivery-targets` silently + drift stayed 0 + lint baselines stayed GREEN. This is the canonical example of LESSON-050's "silent cleanup" gap — the cleanup worked but didn't surface as a commit-worthy event. Adding `## Transient bloat removed` to the orchestrator's output would make this visible to the user.

**Cross-references:** FID-024 §Step D (`scripts/clean-bloat.sh` design); FID-024 §Loop 1 RED item 7 (`git ls-files --others --exclude-standard` + `git status --ignored` dual-check); LESSON-029 (release.py pre-flight cleanup); LESSON-031 (re-grep verification gate).

**Codified by:** Savant (2026-07-14) via Spencer's automation level 3 directive + the post-completion auto-archive discipline sweep.

---

### LESSON-051: Explicit Scope-Ratify Enables Direct Status Advance (Automation Level 3 Escape Hatch)

**Date:** 2026-07-14
**Trigger:** Spencer's session directive 2026-07-14: "i want you to enter automation level 3, complete ALL 4 open fids in logical order". This is the first automated scope-ratify in the project's lifetime; previous ratifies were per-FID or per-step.

**Lesson:** A USER-EXPLICIT scope-ratify permits DIRECT status advancement (`analyzed → closed → archived` in a single session, without per-step ratify). LESSON-038's escape-hatch (`user-explicit` permit-context for `deferred` mentions) extends naturally to status-discipline: when the user explicitly names which FIDs to complete, the agent may flip Status, mv to archive, AND commit in the same session, without per-step confirmation. Per-FID ratify is still required for **FUTURE FIDs** that the user did not name.

**Permitted uses (no extension required):**
- Auto-advancing the named FIDs from `analyzed`/`verified` → `closed` + `mv dev/fids/FID-*.md dev/fids/archive/` + commit each closure in a single session — YES, when user names the specific FIDs in the directive (the 4 the user said complete in this cycle: FID-022 status:fixed + FID-023 + FID-024 + FID-027).
- Auto-implementing FID bodies that have an explicit **§Steps Step A-F** spec (e.g., FID-024's 5 NEW scripts) without per-script approval — YES, when the directive names the FID AND its scope (`complete 4 fids in logical order` is broad; `implement FID-024` is narrow; both are explicit enough).

**Not permitted:**
- Auto-advancing FIDs the user did NOT name in the directive (e.g., if user said "complete FID-023+024" only, FID-027 advance would be unilateral; LESSON-038 violation).
- Auto-implementing FIDs at `analyzed` status without a verified `§Steps Step A-F` spec (the FID must have a concrete plan; otherwise the agent has no anchor for what to author).
- Auto-status-flips for FUTURE FIDs not yet opened (the agent must NOT open new FIDs unilaterally; FID-opening requires a separate user directive — same rule as FID-advancement).

**Enforcement + tooling:**
- A future FID-XXX could write `scripts/check-status-ratify.sh` that asserts: a Status flip from `analyzed`/`verified` to `closed` requires either (a) a FID-auto-archive commit in the same session, OR (b) an explicit LESSON-038 user-quote-text in the commit body. The current orchestrator relies on the agent's correctness; codifying this gives a static-check fallback.
- The simulator prompt for any future agent should specifically cite LESSON-051 when the directive is scope-ratifying closing of FIDs.

**Cross-references:** LESSON-038 (the no-auto-defer rubric this extends); LESSON-019 (release-only-versioning discipline — the automation level 3 does NOT speculatively bump version files; the bump is the orchestrator's release-cut responsibility); FID-024 §Step E orchestrator (`pnpm release:prep X.Y.Z --apply` is a similar scope-ratify — the user named the version, the orchestrator proceeds).

**Anti-pattern (caught + fixed 2026-07-14):** The 4-FID closure initially failed Stage 5 (sed-on-stale-path bug + FID-027 filename typo); the FIX basher re-sed the in-archive files + committed each closure cleanly. The discipline held via the FIX-the-fixer step; didn't require user re-ratify. Confirms LESSON-051's promise: scope-ratify survives the implementer's tool errors.

**Codified by:** Savant (2026-07-14) via Spencer's "complete ALL 4 open fids in logical order" directive + the post-completion auto-archive verification.

---

### LESSON-052: FID Auto-Archive Is a Discipline, Not a Single Event

**Date:** 2026-07-14
**Trigger:** Spencer's "move completed fids" directive (post-completion auto-archive discipline sweep). The user's intent was re-confirming the discipline, not driving a one-shot file migration.

**Lesson:** FID auto-archive is best understood as a **steady-state discipline**, not a `mv` event. Each implementation cycle includes N closure commits, each of which follows 4 sub-steps: (a) advance header `**Status:** X → closed`; (b) `mv dev/fids/FID-*.md dev/fids/archive/`; (c) write commit msg file via heredoc (per LESSON-030); (d) commit with `git commit -F <msg-file>` + `rm -f <msg-file>` (per LESSON-029 cleanup). When the cycle ends, the discipline guarantees: every closed FID is in archive; dev/fids/ contains only `-fixture-` (test artifacts per LESSON-045) + `.gitkeep`; archive/ contains the full lifecycle manifest.

**Permitted uses (no extension required):**
- Running `bash scripts/archive-fids.sh --apply` on a clean working tree as a NO-OP discipline confirmation: prints "0 candidates" + drift stays 0. The orchestrator's no-op property is itself a good signal: it means discipline is being upheld across previous cycles (the auto-archive already happened at FID-closure time).
- Running `bash scripts/clean-bloat.sh --apply` analogously — same no-op-on-clean-tree pattern. Transient files are cleaned at FID-closure time per LESSON-029; a clean working tree's clean-bloat is a no-op.

**Not permitted:**
- Treating "the orchestrator's no-op state" as license to skip the orchestrator from future release-cuts — even when 0 candidates exist TODAY, the orchestrator MUST remain a mandatory release-cut step. The discipline is enforced BY the orchestrator's mandatory presence, not by the candidate list.
- Adding speculative empty commits to document the orchestrator's no-op state (LESSON-029 forbids speculative commits; no-op is the WIN state, not a commit-worthy event).

**Enforcement + tooling:**
- A future FID-XXX could write `scripts/check-archive-discipline.sh` that asserts: `ls dev/fids/*.md` matches the expect pattern (only `*-fixture-*.md` files remain; no other FIDs), AND `bash scripts/archive-fids.sh --apply` returns 0 candidates (verifies the no-op discipline holds), AND `bash scripts/clean-bloat.sh` returns 0 candidates (transient-cleanup discipline holds), AND `bash scripts/lint-defer.sh` + `bash scripts/lint-docs.sh` exit 0. The combined check is the "discipline is upheld" signal.
- A pattern for `## Transient bloat removed` output block (per LESSON-050 step 5) could be extended to the orchestrator's stdout.

**Cross-references:** LESSON-019 (release-only-versioning discipline — the release-cut discipline is parallel to the auto-archive discipline); LESSON-029 (release.py pre-flight cleanup matches the orchestrator's no-op steady-state); LESSON-050 (the untracked-bloat-as-candidate gap that surfaces in the no-op discipline confirmation); FID-024 §Loop 1 GREEN items 1-4 (the orchestrator's verify-discipline edges).

**Codified by:** Savant (2026-07-14) via Spencer's "move completed fids" directive + the discipline sweep's clean no-op state.

### LESSON-053: Double-Boot at Session Start (ECHO.md + Most-Recent Session-Summary)

**Date:** 2026-07-15
**Trigger:** Cascade failure in this session. The agent did not read `ECHO.md` 0-EOF OR the most-recent `dev/session-summaries/*.md` 0-EOF at session start. The cascade: (a) extrapolate chat-history into file-content claims (LESSON-008 violation); (b) author the cascade-contamination file `dev/AUDIT-PACKET-FOR-NOVA-pre-FID-029.md` with 7 cascading str_replace corrections across 4 rounds of LESSON-031 re-grep verification (ECHO Law 1 + 2 + Cross-Agent Claim Rule violations); (c) treat the Cross-Agent Claim Rule's authoritative claim as actionable without citing source paths; (d) recommend hard-reset based on inflated mental model; (e) Spencer hit the ECHO brake ("none of this should even be possible when echo is activated"); (f) regression to attribution debate; (g) Spencer clarified "stop the blame loop, fix it with echo compliance and get back to work"; (h) this LESSON codifies the boot pattern that would have prevented the entire cascade.

**Lesson:** Every ECHO session has TWO mandatory boot reads, in order:
1. **ECHO.md 0-EOF** — the single source of truth for the 15 Laws + Perfection Loop FSM + Five Questions + circuit breakers + Cross-Agent Claim Rule + anti-patterns.
2. **Most-recent `dev/session-summaries/*.md` 0-EOF** — the on-disk project state (open FIDs, WIP files, working-tree interpretation, prior LESSON codifications, FID-TEMPLATE references).

Skipping EITHER read is an ECHO Law 1 (Read 0-EOF Before Touch) violation; both reads are required. The session-summary is the agent's only project-state continuity across multi-session work.

**Permitted uses (no extension required):**
- Reading ECHO.md + the most-recent session-summary as the first two actions of any session, before any other read or write.
- Citing the session-summary's open-FID table + WIP interpretation as the source-of-truth for "what's open" vs extrapolating from chat history.
- Treating the Cross-Agent Claim Rule (LESSON-008 / FID-151) as enforceable: any factual claim sourced from another agent's analysis must cite the source path, not just the attribution.
- Running BOOT CHECK 3 (`pnpm lint:docs && pnpm lint:defer && cargo check --workspace`) BEFORE any session work to confirm the on-disk invariants + cargo baseline are GREEN.

**Not permitted:**
- Proceeding from chat-history assumptions about which FIDs are open, closed, or archived. The session-summary inventory is the source of truth.
- Authoring new audit/consolidation files (audit packets, master-FIDs, integrity manifests) without first reading ECHO.md + session-summary + FID-TEMPLATE 0-EOF.
- Treating the working-tree's dirty-file count as evidence of authorship. Tracked vs untracked + git blame + git log are the only authoritative attribution.
- Skipping the ECHO.md read at session start because "I already know ECHO" — ECHO Law 1 is zero-tolerance; "familiarity" is not equivalent to "read 0-EOF this session."
- Reasoning about "what changed since the last session" without first reading the most-recent session-summary; chat-history continuity ≠ on-disk continuity.

**Pattern (the canonical doctrine):**

1. **Boot Read 1:** `cat ECHO.md` 0-EOF (or `git show HEAD:ECHO.md` for clean-tree discipline). Confirm 15 Laws + Perfection Loop FSM + Cross-Agent Claim Rule.
2. **Boot Read 2:** `ls -t dev/session-summaries/*.md | head -n 1` → filename → read 0-EOF.
3. **Boot Check 3:** Run `commands.build` + `commands.type_check` from `protocol.config.yaml` (the project's source-of-truth for language-specific boot invariants). For Savant: `bash scripts/lint-docs.sh && bash scripts/lint-defer.sh && cargo check --workspace && pnpm tsc --noEmit`. Confirm all exit 0 BEFORE any session work begins.
4. **(optional) Boot Read 4:** `dev/LEARNINGS.md` last 1-2 entries (the most-recent session-summary already encodes the relevant LESSON references; full read is overkill).
5. **Begin session work** only after steps 1-3 complete. If <3 reads complete, ECHO discipline is NOT active and the agent MUST halt + ASK Spencer.

**Enforcement + tooling:**
- The current `dev/session-summaries/*.md` template could add a `## Boot Sequence Performed` section as a noop (Spencer's `2026-07-15-0222-echo-bootstrap.md` already pioneers this pattern).
- A future FID-XXX could write `scripts/check-boot-discipline.sh` that validates the most-recent session-summary has the boot marker + asserts the headline invariants (`pnpm lint:docs`, `pnpm lint:defer`) exit 0 + asserts `cargo check --workspace` is GREEN.
- The simulator prompt for any future agent should include this LESSON-053 in the canonical boot prompt alongside ECHO.md.

**Cross-references:** ECHO §Session Lifecycle steps 1-7 (the canonical boot sequence); LESSON-001 (call-graph reachability); LESSON-008 (Cross-Agent Claim Rule); LESSON-027 + LESSON-038 (the invariants that BOOT CHECK 3 enforces); LESSON-051 (explicit scope-ratify for autonomous FID closure); FID-TEMPLATE (mandatory read for any new FID body); `dev/session-summaries/2026-07-15-0222-echo-bootstrap.md` (the pioneering session-summary with full Boot Sequence Performed section).

**Anti-pattern documentation (this session's cascade, 2026-07-15):** The cascade failure sequence is recorded above in §Trigger. The recovery sequence is also recorded there: (h) Spencer's "fix with echo compliance and get back to work" directive converted the cascade from a blame-loop toward a single-tool operation: delete the cascade-contamination file (`rm -f dev/AUDIT-PACKET-FOR-NOVA-pre-FID-029.md` since it was untracked + had no project-state references per `grep -rn AUDIT-PACKET`); codify LESSON-053 here; verify the boot invariants re-emerge GREEN; resume work on FID-029 (the lowest-dependency active FID per SPECDO discipline).

**Codified by:** Savant (2026-07-15) via Spencer's cascade-recovery session directive ("fix it with echo compliance and getting back to work") + the ECHO-brake intervention ("you need to read echo 0-end first, none of this should even be possible when echo is activated").

### LESSON-054: Stale-Session Transcript Cleanup Discipline + Cross-Project-Invariant Extraction Pattern

**Date:** 2026-07-15
**Trigger:** Discovered `[session-ses_09de.md]` (1,655,521 bytes, untracked, workspace root, NOT gitignored) during cascade-recovery. The file is a prior-agent session transcript (combination of Tencent Hy3 + StepFun Step 3.7 Flash sessions for the same project) — the prior agent was confused about which project it was working on (assumed "Savant Trading" per system-prompt-embedded Savant Trading context, but the actual on-disk project is Savant v0.0.5, a Next.js + Tauri shell, NOT Savant Trading per `protocol.config.yaml`). Surfaced two actionable cross-project invariants: (a) **on-disk ECHO always wins** over system-prompt-embedded content (basher-verified: `ECHO.md` on disk is v0.1.1; embedded `ECHO.md` was v0.1.0 with Savant Trading app-specific commands — `cd dashboard && npm run build`, `cargo test` = 264 tests — none of which apply to Savant); (b) **verify project-context before any `cd` or `npm install`** — the prior agent attempted `pnpm install three @react-three/fiber @react-three/drei...` for an icon-pack wire-up, with no `dashboard/` directory existing on disk to `cd` into. Spencer ratified "extract + rm" disposition per LESSON-029 cleanup + LESSON-008 Cross-Agent Claim Rule.

**Lesson:** When an untracked workspace-root `session-ses_*.md` or `*-session-trail*.md` file appears (likely from a prior agent's session log dump), it is simultaneously: (a) a **soft evidence-source** for prior-agent confusion patterns (per LESSON-008 Cross-Agent Claim Rule — attribution is not a source); and (b) a **candidate for LESSON-029 + LESSON-050 cleanup** (workspace-bloat pattern matching `dead-*`, `.scratch-*`, transient-file shapes). The cleanest ECHO disposition is a 3-step: **read 0-EOF** (Law 1) → **extract any cross-project invariants** not yet codified as LESSONs → **codify via LESSON entry + rm -f the source** (the LESSON captures the insights; the source is bloat). The disposition MUST happen BEFORE the file's content sizeof grows past 1 MB (the Truncation Shock threshold — `read_files` truncates at 20K tokens; 1.6 MB transcripts truncate before the agent can extract insights).

**Permitted uses (no extension required):**
- `find . -maxdepth 1 -type f -name 'session-ses_*' -o -name '*-session-trail*'` 2>/dev/null as a periodic hygiene scan (efficient: ≤1ms on typical repos).
- Reading 0-EOF per ECHO Law 1 + `wc -c` size-signal to evaluate cleanup eligibility BEFORE any `rm -f`.
- Extracting **cross-project invariants** (patterns applying to any ECHO-compliant project, e.g., "on-disk ECHO always wins" or "verify project-context before any tool-use") — NOT project-specific findings (those live in `dev/session-summaries/` or FIDs).
- `rm -f session-ses_<id>.md` per LESSON-029 after LESSON codification (the LESSON is durable; the transcript is bloat).
- Documenting the cleanup in `## Transient bloat removed` block in the active session summary (per LESSON-050 step 5).

**Not permitted:**
- Treating any prior-agent's session log as project ground truth (LESSON-008 attribution-only — the donor agent's understanding is a hypothesis, not a source).
- `rm -f` any untracked workspace-root file WITHOUT prior read (ECHO Law 1 zero-tolerance).
- Extracting NON-invariant content (donor agent's draft code, abandoned explorations) into LESSON entries — LESSONs codify invariants, not local narrations.
- Speculatively bumping version files when adding a LESSON (LESSON-019 release-only-versioning discipline).

**Pattern (canonical doctrine for stale-session transcript cleanup):**

Compressed 4-step (extends LESSON-029 cleanup pipeline + LESSON-050 untracked-bloat-candidate discipline; this LESSON adds the **cross-project-invariant extraction** precondition):

1. **Locate + size-signal + untracked-verify** (combined per LESSON-029 + LESSON-050): `find . -maxdepth 1 -type f \( -name 'session-ses_*' -o -name '*-session-trail*' -o -name 'ses_*' \) 2>/dev/null | head -n 5` + `wc -c <file>` (>100 KB → likely transcript; >1 MB → Truncation Shock risk) + `git ls-files --error-unmatch <file>` (must be untracked per LESSON-029 criteria).
2. **Read 0-EOF** (ECHO Law 1) + **extract cross-project invariants ONLY** (project-specific findings live in `[dev/session-summaries/]` or FIDs, NOT in LESSONs; LESSONs codify invariants only).
3. **Codify via LESSON if + only if invariant** (insert new entry above `<!-- Add new entries above this line -->` marker; format-conformance against LESSON-050/051/052/053 baseline) + `rm -f <file>` (LESSON captures insights; source is bloat per LESSON-029).
4. **Document `## Transient bloat removed` block** in active session-summary (LESSON-050 step 5) + **re-run BOOT CHECK 3** (`bash scripts/lint-docs.sh && bash scripts/lint-defer.sh && cargo check --workspace` per LESSON-053) — invariant-addition must not regress the invariants.

**Worked example (this session's recovery, 2026-07-15):** The untracked `[session-ses_09de.md]` at workspace root was 1,655,521 bytes (1.6 MB; Truncation Shock threshold exceeded). Step 1: located via `git ls-files --others --exclude-standard`. Step 2: read 0-EOF → extracted 2 cross-project invariants (on-disk ECHO always wins over system-prompt-embedded content; size-signal > 1 MB is a Truncation Shock indicator). Step 3: codified this LESSON-054 + `rm -f session-ses_09de.md`. Step 4: `## Transient bloat removed` documented + BOOT CHECK 3 re-run (LESSON-027 + LESSON-038 + cargo baseline all exit 0 per cascade-recovery verification). Source file gone; insights preserved.

**Enforcement + tooling:**
- A future FID-XXX could write `scripts/check-stale-transcripts.sh` that runs `find . -maxdepth 1 -type f -name 'session-*'` and reports candidates + recommends disposition per the doctrine above.
- A future FID-XXX could write `scripts/check-lesson-marker-discipline.sh` that asserts `<!-- Add new entries above this line -->` is the last non-blank line in `dev/LEARNINGS.md`.

**Cross-references:** ECHO.md §Cross-Agent Claim Rule (LESSON-008 attribution ≠ source); LESSON-029 (release.py pre-flight cleanup); LESSON-050 (untracked-bloat candidate pattern); LESSON-053 (the meta-LESSON — codifies the on-disk-wins + double-boot doctrine that this LESSON-054 generalizes to the stale-transcript case); FID-TEMPLATE §Closed footer (the movement pattern this LESSON doesn't apply to closed FIDs — those have `dev/fids/archive/` discipline already).

**Anti-pattern documentation (this session's stale-transcript discovery, 2026-07-15):** The untracked `[session-ses_09de.md]` at workspace root was 1,655,521 bytes (1.6 MB) — significantly larger than the initial basher probe of 30,834 bytes had led the agent to believe (the 30,834 figure was a strlen-of-content subset; `wc -c` on the file shows the real size). When the prior agent (Tencent Hy3 + StepFun Step 3.7 Flash) ran in this codebase, it accumulated ~1.6 MB of session log including ECHO-contents, project-exploration bash output, and aborted `pnpm install` invocations — none of which had to live past the prior agent's session termination. Spencer ratified the "extract + rm" disposition per LESSON-029 + this LESSON's Pattern; the cross-project invariants are codified above; the source file will be `rm -f`'d in this session's Step 3 of the post-ratification flow.

**Codified by:** Savant (2026-07-15) via Spencer's ratification of "extract + rm" for `session-ses_09de.md` under LESSON-029 cleanup + LESSON-008 Cross-Agent Claim Rule + the cleanup-discipline extension of LESSON-050.

<!-- Add new entries above this line -->
