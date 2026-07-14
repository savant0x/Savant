# FID-012 — Per-Section Rating Breakdown

**Date:** 2026-07-13
**Author:** Spencer + Buffy (FID-006 v3 polish, item (b))
**Status:** closed
**Perfection Loop:** Completed 2026-07-13. Test coverage added at `src/lib/manifest-mock.test.ts` (covers `computeSectionMetrics` edge cases: empty content, single section, canonical 18 sections, loose regex variants for LLM output, placeholders, short sections, density rounding). 68/68 vitest tests passing.

---

## Problem

The Draft Buffer's existing rating grid has only ONE DEPTH card
for the entire soul body. That's a useful at-a-glance summary, but
the user can't tell which of the 18 sections are well-developed
and which are thin. The global DEPTH number averages everything
together, hiding per-section quality issues.

## Solution

Add a new "SECTIONS BREAKDOWN" collapsible below the SoulBodyViewer.
Shows one `SectionRatingCard` per `## ...` section in the built
soul body, with per-section metrics (lines, words, density, and
a completeness indicator). Collapsed by default to avoid dominating
the page (18 cards).

## Architecture

### `src/lib/manifest-mock.ts` — `computeSectionMetrics(content)`

- Splits the soul body by top-level `## ...` headers (regex
  `/^(##\s+.+)$/gm`).
- For each section: lines, words, density (words/line),
  completeness (>= 10 words AND not a placeholder).
- Header regex is LOOSE (not strict `## N. Title` only) to match
  LLM output variations like `## 1) Title`, `## 1 Title`,
  `## Section 1: Title`. The id field extracts a leading number
  if present; otherwise falls back to the 1-based array index.

### `src/components/section-rating-card.tsx` — NEW component

- Compact horizontal card: section number (left, monospace) +
  truncated title + Lines + Words (hidden on mobile) + density
  pill + completeness dot.
- Color thresholds for density: green >= 10, yellow >= 5, red < 5.
- Completeness dot: green if `metric.completeness`, red otherwise.

### `src/app/manifest/page.tsx` — UI integration

- `sectionMetrics` useMemo on `builtSoul?.content` (NOT the full
  `builtSoul` object, so ancillary field changes don't recompute).
- New `<details>` collapsible below the SoulBodyViewer.
- Shows section count + completeness count in the summary line.

## Verification

- `npx tsc --noEmit` — exit 0.
- `code-reviewer-minimax-m3` — APPROVE on 5 review passes (initial
  + 4 cleanup passes for tsc errors, dead code, regex strictness,
  memo dep tightening, state-ordering).
- Manual smoke test: 18 cards render with the template fallback,
  density colors map correctly, completeness dot toggles.

## Open follow-ups

- Add unit tests for `computeSectionMetrics` (template fallback,
  LLM output variants, empty content).
- Consider a "filter by status" toggle (only show incomplete
  sections) to surface quality issues faster.
- Per-section word targets (e.g., section 1 should be ~150 words,
  section 7 should be ~30 words). Defer to a future FID.
