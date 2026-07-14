# FID: [Short Description]

**Filename:** `FID-YYYY-MMDD-NNN-[short-description].md`
**ID:** FID-YYYY-MMDD-NNN
**Severity:** critical | high | medium | low
**Status:** created | analyzed | fixed | verified | closed
**Created:** YYYY-MM-DD HH:MM
**Author:** [Agent/Human Name]

---

## Summary

One-paragraph description of the finding.

## Environment

- **OS:** [OS and version]
- **Language/Runtime:** [Language and version]
- **Tool Versions:** [Relevant tool versions]
- **Commit/State:** [Git SHA or state description]

## Detailed Description

### Problem

What is the issue? What behavior was observed?

### Expected Behavior

What should happen instead?

### Root Cause

What is the underlying cause?

### Evidence

Include logs, screenshots, code snippets, or test output.

```text
[Paste evidence here]
```

## Impact Assessment

### Affected Components

- [Component 1]
- [Component 2]

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [ ] High: Major feature broken, no workaround
- [ ] Medium: Feature degraded, workaround exists
- [ ] Low: Minor issue, cosmetic, or edge case

## Proposed Solution

### Approach

How should this be fixed?

### Steps

1. [Step 1]
2. [Step 2]
3. [Step 3]

### Verification

How will we confirm the fix works?

## Perfection Loop

### Loop 1

- **RED:** [Issues identified]
- **GREEN:** [Fixes applied]
- **AUDIT:** [Verification results]
- **CHANGE DELTA:** [Percentage of code changed]

### Loop 2 (if needed)

- **RED:** [Remaining issues]
- **GREEN:** [Additional fixes]
- **AUDIT:** [Verification results]
- **CHANGE DELTA:** [Percentage]

## Verifier Pass

(Optional. Add this section only if a verifier pass has been performed on
the FID-doc itself. Top-level sibling of `## Perfection Loop`; distinct from
impl-iteration events.)

Content shape:

- **RED (gaps surfaced in this verifier pass):** —
  Gap-survey items the verifier surfaced (issue-specific,
  not impl-iteration). Number them; cite specific anchors / LESSONs /
  scripts in each item.
- **GREEN (recommendations for next session, NOT applied in this pass):** —
  Verifier's recommendations for what the next session is suggested to do,
  listed with rationale. These are NOT applied during the verifier pass
  itself; per LESSON-038 they require Spencer's separate ratification.
- **AUDIT (this pass):** — verbatim invariants verified during the
  verifier pass (e.g., "lint-defer.sh exit 0", "LESSON-027 invariant
  preserved at 5+1", "FID status field unchanged"). These are NOT
  recommendations; they are reported facts.
- **CHANGE DELTA:** — percentage of the FID body that was rewritten or
  appended during this verifier pass. Use `~X%` for approximate changes.

**Convention (LESSON-049):** A verifier pass is the agent's META-REVIEW
of a previously authored FID body — gap-survey + recommendation-surfacing.
It is distinct from the `## Perfection Loop` impl-iteration events (RED →
GREEN → AUDIT on CODE changes).

- DO use top-level `## Verifier Pass (YYYY-MM-DD — meta-review description)`
  for meta-review content.
- DO NOT sub-section verifier-pass content as `### Loop N (Verifier Pass ...)`
  inside `## Perfection Loop`. The sub-section pattern conflates impl-iteration
  with meta-review + breaks the convention.
- Implementation sub-iterations (e.g., `### Loop 1 (TypeScript fix)` in
  FID-027) STAY under `## Perfection Loop`. The same `### Loop N` header
  does NOT shift between impl and meta contexts in the same FID body.

**When to skip / include this section:**
- MUST omit this section for pure-impl FIDs (no agent-verifier step performed yet) — those are tracked only via `## Perfection Loop`.
- MUST include this section for any FID that has been verifier-passed (gap-survey applied), including test fixtures + production FIDs + closed FIDs with re-opens.
- The discriminator is: if the agent has performed a meta-review pass that surfaced RED/GREEN/AUDIT findings, the FID has a verifier pass + this section is mandatory.

**Resolution-amendment discipline (FID-TEMPLATE consistency):** If the
verifier pass's AUDIT phase surfaces gaps in the FID's `## Resolution`
section (e.g., TBD placeholders that need filling, missing tests), the
verifier MUST flag them in the AUDIT block + recommend them as GREEN
items + cite a FUTURE-FID for the amendment. The verifier MUST NOT
amend `## Resolution` inline during the verifier pass — inline amendment
breaks the FID shape discipline (Resolution is updated only at impl + at
close, not at verifier-meta-review). Future-amendment FID must be a
sibling FID OR a §Resolution §Closed-footer amendment at FID-closure
time.

---

## Resolution

- **Fixed By:** [Agent/Human Name]
- **Fixed Date:** YYYY-MM-DD HH:MM
- **Fix Description:** [What was changed]
- **Tests Added:** [Yes/No — describe]
- **Verified By:** [Verification method]
- **Commit/PR:** [Reference]
- **Archived:** YYYY-MM-DD HH:MM (set when moved to `dev/fids/archive/`)

> When status is set to **Closed**, move this file to `dev/fids/archive/` and
> append an entry to `CHANGELOG.md`.

## Lessons Learned

What can we learn from this finding? How can we prevent similar issues?

---

## Questions You Should've Asked

(Optional but recommended. Surface the verifier's / reviewer's
"questions-I-wish-I'd-asked-before-authoring" prompts as a structured
Q&A. Codifies the decision points surfaced during the FID's analysis +
Implementation. Especially valuable for FIDs at `analyzed` or `verified`
state awaiting Spencer's separate ratification on implementation timing
per LESSON-038.)

**4-field template per item:**

Each numbered item follows this exact structure:

    N. **Q:** [Single-line question phrase — the gap to surface]
       - **Context:** [1-2 sentences explaining the background / why this matters / the LESSON-anchor that flagged it]
       - **Recommended:** [1 sentence — the verifier's recommended action for Spencer]
       - **Trade-off:** [1 sentence — the alternative path's cost]

**Convention (LESSON-049):** The 4-field template makes Q&A items
scannable + diff-able + parallel across FIDs. Future-FID authors filling
out this section should use the template consistently.

**Anti-pattern:** Open-prose items (one big paragraph per question) are
unscannable + hard to diff against future agent replies + inconsistent
across FIDs. Avoid prose; use the template.

**Length guidance (strict):** Aim for 3-5 items per FID. Each sub-bullet
(`Context` + `Recommended` + `Trade-off`) MUST be **1 sentence** (max 2
for `Context` only). If an item needs more, split it into multiple items.
The rule is: **keep Q&A scannable; longer explanations belong in `## Lessons
Learned` or `## Improvements Missed` instead.**

**Template spec (copy-paste-safe)** — use this block when authoring
Q&A items in a new FID:

    N. **Q:** [Single-line question phrase — the gap to surface]
       - **Context:** [1-2 sentences explaining the background / why this matters / the LESSON-anchor that flagged it]
       - **Recommended:** [1 sentence — the verifier's recommended action for Spencer]
       - **Trade-off:** [1 sentence — the alternative path's cost]

> Note: live FIDs render the 4-space-indented template block as a code
> block, which is intentional — the template body is spec, not live
> content. When copy-pasting, the indentation is part of the template,
> so preserve it as you would for any code block.

**Examples of well-formed Q&A items** (live Q&A bodies in actual FIDs use
1 sentence per sub-bullet):

1. **Q:** Which compliance audit threshold applies here?
   - **Context:** The 3-tier thresholds diverge by jurisdiction; current FID cites only the canonical source.
   - **Recommended:** Pick the higher threshold + document the rationale in §Compliance.
   - **Trade-off:** Higher threshold costs ~30 LoC; lower threshold risks audit-finding.

2. **Q:** Should the orchestrator support `--resume-from-step=N`?
   - **Context:** If the orchestrator aborts mid-run, partial state may leave the working tree inconsistent.
   - **Recommended:** Ship `--resume-from-step=N` as a FUTURE FID (NOT in this FID's primary scope).
   - **Trade-off:** Adds session-state complexity; benefit is recoverable-from-mistakes.

**Cross-section consistency:** When a question's recommended action
references a specific file path or FUTURE-FID candidate, mirror the
same reference in the `## Improvements Missed` section below the Q&A
section. Future agents reading both surfaces should see consistent
forward-pointers.

**Cross-references:** LESSON-049 + the `## Verifier Pass` section above
together codify the verifier-pass + Q&A convention. See also `dev/LEARNINGS.md`
LESSON-049 entry for the canonical lesson-form documentation.
