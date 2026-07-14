# FID-013 — Swarm Deployment Preview Diff

**Date:** 2026-07-13
**Author:** Spencer + Buffy (FID-006 v3 polish, item (c))
**Status:** closed
**Perfection Loop:** Completed 2026-07-13. Test coverage added at `src/lib/swarm-diff.test.ts` (covers `computeSwarmHashSync` stability, `previewSwarmDiff` 4-bucket classification: added/modified/removed/unchanged, mixed scenarios, empty inputs). 68/68 vitest tests passing.

---

## Problem

The "Deploy Swarm" section in the manifest page dispatches
`bulk_manifest` immediately. The user has no visibility into WHAT
will change in the active baseline before confirming — they just
see a spinner and a success/fail banner. This is dangerous for
production swarms: a typo in the JSON could silently remove
critical agents.

## Solution

Add a two-step "preview + confirm" flow:
1. User clicks "Deploy Swarm" → parse JSON, fetch current
   baseline from localStorage (mock IPC), compute 3-way diff
   (added/modified/removed/unchanged), display the diff inline.
2. User reviews the diff, then either clicks "Confirm & Deploy
   Swarm" (proceed) or "Cancel" (return to idle state).

## Architecture

### `src/types/control-frames.ts` — no changes (uses existing
`AgentManifestPlan`)

### `src/lib/swarm-diff.ts` — NEW module

- `computeSwarmHash(agent)` — async SHA-256 (16-char hex prefix)
  for cryptographic-grade integrity (used by future flows like
  "verify deployed matches baseline").
- `computeSwarmHashSync(agent)` — sync FNV-1a 32-bit. Used inside
  the diff loop to avoid async crypto.subtle.digest cost per
  agent. Not cryptographic but collision-resistant enough for
  change detection at swarm sizes < 100.
- `previewSwarmDiff(baseline, proposed)` — returns
  `{ added, modified, removed, unchanged }` matched by `name`
  field. The `modified` bucket contains
  `{ baseline, proposed }` pairs so the UI can show
  before/after diffs.

### `src/lib/mock-ipc.ts` — IPC additions

- `LS_SWARM_BASELINE = "savant.bulk.baseline"` — localStorage key
  for the last successful deployment.
- `bulk_manifest` case now writes the deployed agents to LS on
  success. Documented as non-transactional (quota/private-mode
  failure leaves baseline stale; low risk in practice).
- New `get_swarm_baseline` case reads from LS, validates the
  shape, returns `[]` on missing/corrupt.

### `src/lib/ipc.ts` — renderer wrapper

- `getSwarmBaseline(): Promise<AgentManifestPlan[]>` — calls
  the IPC command, returns the baseline.

### `src/components/bulk-diff-viewer.tsx` — NEW component

- 4 color-coded sections: ADDED (green + plus icon), MODIFIED
  (yellow + pen icon), REMOVED (red + minus icon), UNCHANGED
  (muted, collapsed by default).
- Each item shows: name + content snippet (first 100 chars of
  soul). MODIFIED items show `before → after` hint.
- "No changes" banner if all 3 change buckets are empty.

### `src/app/manifest/page.tsx` — UI integration

- New state: `bulkBaseline`, `bulkDiff`, `bulkShowDiff`.
- Baseline loaded on mount via `useEffect`.
- `onBulkDeploy` rewritten as a two-step flow: Step 1 (idle)
  parses JSON, fetches baseline, computes diff, shows preview.
  Step 2 (preview) dispatches `bulk_manifest`, updates baseline
  on success.
- `onBulkCancel` clears the diff state + `bulkError` + stale
  `bulkResult` (so the old success banner doesn't linger).
- JSON textarea is `readOnly` during preview/deploy to prevent
  the diff from going stale.

## Verification

- `npx tsc --noEmit` — exit 0.
- `code-reviewer-minimax-m3` — APPROVE on 5 review passes.
- Manual smoke test: first deploy shows all ADDED, second deploy
  shows all UNCHANGED, edit + deploy shows MODIFIED, remove
  agent shows REMOVED.

## Open follow-ups

- Line-by-line text diff for the `modified` bucket (would require
  an external `diff` library; violates no-deps rule). For now
  the per-agent boolean is sufficient.
- "Update Preview" button to re-compute the diff after editing
  the JSON without going through Deploy Swarm.
- Phase 2 Tauri: replace localStorage with real Rust state read
  (parse `workspace-savant/SOUL.md` for each agent).
- Bulk diff unit tests (defer to FID-015).

## Phase 2 Tauri migration

- `get_swarm_baseline` reads from Rust state (parsed SOUL.md files).
- The diff logic stays client-side (renderer computes
  `previewSwarmDiff` from the returned baseline + the proposed
  JSON).
- `bulk_manifest` writes to Rust state instead of localStorage.
