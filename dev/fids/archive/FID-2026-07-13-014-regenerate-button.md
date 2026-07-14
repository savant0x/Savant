# FID-014 — Regenerate Button (Draft Buffer)

**Date:** 2026-07-13
**Author:** Spencer + Buffy (FID-006 v3 polish, item (d))
**Status:** closed
**Perfection Loop:** Completed 2026-07-13. No unit tests added (the `regenPending` useEffect + capture-then-reset pattern lives in `src/app/manifest/page.tsx` and would require React Testing Library to test in isolation; deferred per code-reviewer verdict in FID-009). The pattern was verified through 5 code-reviewer-minimax-m3 passes + manual smoke test per the FID-014 body. 68/68 vitest tests passing across the other 6 FIDs.

---

## Problem

The Draft Buffer only had Copy + Revert actions. If the user
wanted to re-run the LLM with the same prompt (to get a different
output), they had to manually re-type the prompt and click
MANIFEST SOUL again. This is tedious and error-prone (typos in
the prompt would produce a different soul).

## Solution

Add a "Regenerate" button next to Copy + Revert. On click:
- Reads the persisted prompt + name from the built soul
- Restores them to the textarea (so the user can see what's
  being submitted)
- Increments a `regenCount` state (1, 2, 3, ...)
- Re-runs the LLM with the persisted prompt + a ` [variant #N]`
  suffix to force a different output

The variant suffix is a BEST-EFFORT approach to force OpenRouter's
free models to produce different output (those models don't
accept a `seed` parameter). Aggressive prompt caching may still
produce identical output. A more reliable approach would be to
vary the temperature (deferred to a follow-up).

## Architecture

### `src/app/manifest/page.tsx` — additions

- New state: `regenCount` (counter) + `regenPending` (gate flag
  for the double-click race).
- `onManifestSubmit` uses a CAPTURE-THEN-RESET pattern: at the
  top of the function, capture `regenCount` into a local
  `submitRegenCount` variable, then immediately reset the
  state to 0. The suffix uses the captured value, NOT the React
  state. This guarantees the suffix is applied for EXACTLY the
  submit triggered by `onRegenerate` — the next click on MANIFEST
  SOUL sees `regenCount=0` and gets no suffix.
- `onRegenerate` callback: restores persisted prompt + name to
  the textarea, increments regenCount, sets regenPending=true.
  Guarded by `!builtSoul || submitting || regenPending` to close
  the double-click race.
- `regenPending` useEffect: fires `onManifestSubmit` (via a ref
  to avoid re-running on every state tick) when regenPending
  becomes true.
- `submitDisabled` now includes `regenPending` (gates MANIFEST
  SOUL during the regen window).
- Regenerate button: `disabled={!builtSoul || submitting || regenPending}`.
  Shows `Regen #N` when regenCount > 0, plain "Regenerate" otherwise.
- The persisted entry stores the ORIGINAL user prompt (not the
  variant-suffixed version) so the Draft Buffer always shows
  what the user typed.

### Race condition closed

The Regenerate button + MANIFEST SOUL button are both gated on
`regenPending`. Without this, the user could double-click
Regenerate during the regenPending window (between
`setRegenPending(true)` and the useEffect firing), which would
burn the variant counter on an unintended second increment.

## Verification

- `npx tsc --noEmit` — exit 0.
- `code-reviewer-minimax-m3` — APPROVE on 5 review passes
  (including 4 cleanup passes for the CRITICAL regenCount reset
  bug, the racy useEffect, the memo dep, the state-ordering tsc
  error, the race condition, and the dead `Record<boolean, string>`).
- Manual smoke test: click Regenerate → counter increments, new
  soul generated. Click MANIFEST SOUL with a fresh prompt →
  counter resets to 0, no suffix appended.

## Open follow-ups

- Temperature variation (more reliable than prompt suffix for
  forcing different output). Defer to a follow-up FID.
- Variant history (allow paging back through previous
  regenerations). Currently overwrites — Phase 1 is "good enough".
- Unit tests for the capture-then-reset pattern + the race
  condition. Defer to FID-015.

## Phase 2 Tauri migration

- The capture-then-reset pattern stays in the renderer.
- The `[variant #N]` suffix is appended client-side before
  passing to `manifest_soul_stream`.
- Rust backend sees the suffixed prompt (no change to the
  streaming protocol).
