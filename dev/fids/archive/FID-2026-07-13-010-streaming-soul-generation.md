# FID-010 — LLM Response Streaming (Soul Generation)

**Date:** 2026-07-13
**Author:** Spencer + Buffy (FID-006 v3 polish, item (a))
**Status:** closed
**Phase 2 Tauri migration:** See §Phase 2 Tauri migration below
**Perfection Loop:** Completed 2026-07-13. `parseSSEStream` exported from `src/lib/manifest-mock.ts` for testability. Test coverage added at `src/lib/manifest-mock.test.ts` (TCP fragmentation, [DONE] sentinel, malformed chunks, abort, single/multiple events). 68/68 vitest tests passing.

---

## Problem

The `/manifest` page's `manifest_soul` IPC dispatch blocks the UI for
**10-30 seconds** while the LLM call completes. The current flow:

1. User clicks "MANIFEST SOUL".
2. Button shows "MANIFESTATION SEQUENCE IN PROGRESS…" + spinner.
3. UI is **frozen** — no progress, no cancel, no preview.
4. Suddenly the full SOUL.md body appears in the Draft Buffer.

This is unacceptable for a 10-30s operation. Industry standard is
**Server-Sent Events (SSE) streaming** with progressive rendering.

## Solution

Stream the LLM response via OpenRouter's `stream: true` mode. Yield
`preamble` / `chunk` / `complete` / `error` events to the renderer.
Display a **live preview card** that updates as tokens arrive. Add
a **Cancel button** that aborts the in-flight fetch.

### Channel-shaped IPC contract (Tauri v2 compatible)

The IPC contract is a **Channel** that mirrors Tauri v2's
`Channel<T>` API:

```typescript
interface ManifestStreamChannel {
  onmessage(handler: (event: ManifestStreamEvent) => void): () => void;
  send(event: ManifestStreamEvent): void;  // internal — IPC-side
}

interface ManifestStreamHandle {
  cancel(): void;
  done: Promise<void>;
}

async function manifestSoulStream(
  payload: SoulManifestPayload,
  channel: ManifestStreamChannel,
): Promise<ManifestStreamHandle>;
```

**Phase 1 (browser mock):** the channel is a plain object passed
by reference (mockIPC is an in-process function call, no
serialization).

**Phase 2 (Tauri v2):** the channel is `new Channel<ManifestStreamEvent>()`
from `@tauri-apps/api/core`. The Rust command receives it as
`tauri::ipc::Channel<ManifestStreamEvent>` and calls `channel.send(event)`.
The renderer's `onmessage(handler)` API stays identical.

This design was **critical** — an earlier draft used a custom
`AsyncIterable` handle, which would have required a full renderer
rewrite for Phase 2 (Tauri's IPC cannot serialize/return JS
iterators from Rust). Caught by the FID-010 design review.

## Architecture (4 layers)

### Layer 1 — `src/lib/manifest-mock.ts`

Added `generateSoulStream()` async generator + `parseSSEStream()` helper:

- **`parseSSEStream(body, signal)`** — buffered SSE parser that handles
  **TCP fragmentation** correctly. A single TCP packet can split
  mid-JSON (e.g., `data: {"cho` in one chunk, `ices": ...}` in the
  next). The parser maintains a string buffer and only processes
  events that have a complete `\n\n` boundary. Uses `TextDecoder`
  with `{ stream: true }` for multi-byte UTF-8 splits.

- **`generateSoulStream(prompt, name, tier, masterKey, model, signal)`** —
  yields `ManifestStreamEvent` objects as the LLM produces tokens.
  Pre-yields a static `# SOUL.md` header (preamble) before the LLM
  call so the user sees the file shape immediately even on a slow
  TTFB. If `masterKey` is empty (template fallback), yields one
  chunk with the full template + a complete event.

### Layer 2 — `src/lib/mock-ipc.ts`

Added the `manifest_soul_stream` IPC command + `cancel_manifest_stream`:

- `activeStreams: Map<streamId, AbortController>` — tracks in-flight
  streams for explicit cancellation by ID.
- `manifest_soul_stream` returns `{ cancel, done }` handle. The
  background loop consumes the async generator and forwards events
  to the channel.
- `cancel_manifest_stream` aborts by streamId (safety net for
  cross-scope cancellation).

### Layer 3 — `src/lib/ipc.ts`

Added the renderer-facing API:

- `ManifestStreamChannel` interface (duck-typed, Phase 2 compatible).
- `createManifestStreamChannel()` factory.
- `ManifestStreamHandle` interface (`{ cancel, done }`).
- `manifestSoulStream(payload, channel)` wrapper.

### Layer 4 — `src/app/manifest/page.tsx`

Rewrote `onManifestSubmit` to use the streaming API. Added:

- **rAF-throttled state** — `streamBufferRef` (useRef) + `setStreamBuffer`
  via `requestAnimationFrame`. Avoids triggering a React re-render
  on every SSE token (OpenRouter can emit 50-200 chunks/sec).
- **Elapsed time ticker** — `setInterval(100ms)` updates
  `streamElapsedSec` while a stream is in flight.
- **Auto-cancel on resubmit** — if a previous stream is in flight,
  cancel it before starting a new one. Prevents chunk interleaving.
- **Cleanup on unmount** — cancels the stream + clears the rAF tick
  to prevent memory leaks.
- **Live preview card** — new Section 1.5, visible only while
  `streamBuffer` is non-empty. Shows:
  - Pulsing dot + "Streaming Soul Manifestation" label
  - Char count + elapsed seconds
  - Cancel button (red, danger styling)
  - Live preview in a scrollable mono-font box with blinking cursor
- **`onCancelStream` callback** — fire-and-forget abort + state reset.

## Phase 2 Tauri migration

1. Replace `createManifestStreamChannel()` with:
   ```typescript
   import { Channel } from "@tauri-apps/api/core";
   const channel = new Channel<ManifestStreamEvent>();
   channel.onmessage = (event) => { /* same handler as today */ };
   ```
2. Update `manifest_soul_stream` Rust command to accept
   `tauri::ipc::Channel<ManifestStreamEvent>` and call
   `channel.send(event)` from the SSE parse loop.
3. The renderer's `manifestSoulStream` wrapper stays unchanged
   (the channel is just passed through to `invoke`).
4. Drop `_streamId` + `_channel` payload fields — Tauri handles
   channels natively.

## Verification

- `npx tsc --noEmit` — exit 0 (TypeScript types align).
- `code-reviewer-minimax-m3` — APPROVE on all 5 design criteria
  (Channel contract, TCP fragmentation, rAF throttling, auto-cancel,
  cleanup-on-unmount).
- Manual smoke test (Phase 1 browser mock):
  - Click "MANIFEST SOUL" → live preview card appears in <500ms
  - Chunks stream in, char count + elapsed time update
  - Click "Cancel" → stream stops, card disappears, status resets
  - Submit a new prompt while old stream is in flight → old stream
    is auto-cancelled, new stream starts cleanly
  - No-key fallback (no API key) → static template streams as one
    chunk + complete event

## Open questions / follow-ups

- **First-chunk timing** — OpenRouter can take 500ms-2s to send the
  first chunk (model load + cold start). The current UX shows the
  preamble immediately (good), but the gap between preamble and
  first chunk is silent. Consider adding a "Connecting..." state.
- **Token-level vs chunk-level** — OpenRouter chunks are
  variable-length (sometimes single tokens, sometimes multi-word
  phrases). The current implementation yields them as-is. If we
  want typewriter-style animation, we'd need to buffer + animate
  client-side. Out of scope for FID-010.
- **Error recovery** — if the stream errors mid-way (e.g. network
  drop), the user sees the partial buffer + an error. Consider
  adding a "Retry" button that re-runs the LLM call with the same
  prompt. Out of scope for FID-010.
- **Rate limiting** — OpenRouter returns 429 on rate limit. The
  current error path shows the 429 message. Consider exponential
  backoff retry. Out of scope for FID-010.

## Code-reviewer verdict (1st pass)

> **APPROVE.** The Channel-based contract is the right abstraction
> for Phase 2 Tauri migration. The SSE parser handles TCP
> fragmentation correctly. The rAF throttling avoids React render
> thrashing on high-frequency SSE updates. The auto-cancel on
> resubmit + cleanup-on-unmount prevent memory leaks. The live
> preview card is well-positioned (between Section 1 and Section 2)
> and the Cancel button is visually distinct from the primary
> submit button.
>
> **Non-blocking nit:** The `manifest_soul_stream` mock IPC command
> is ~60 lines (the largest in the switch). Consider extracting
> the background loop into a `runManifestStream(...)` helper for
> testability. Defer to a follow-up FID.

## Files changed

- `src/lib/manifest-mock.ts` — added `ManifestStreamEvent` type,
  `parseSSEStream` helper, `generateSoulStream` async generator
  (~150 lines).
- `src/lib/mock-ipc.ts` — added `activeStreams` Map,
  `manifest_soul_stream` + `cancel_manifest_stream` commands
  (~70 lines).
- `src/lib/ipc.ts` — added `createManifestStreamChannel` +
  `manifestSoulStream` wrapper (~80 lines).
- `src/app/manifest/page.tsx` — added streaming state/refs,
  elapsed-time ticker, cleanup useEffect, `onCancelStream`,
  rewrote `onManifestSubmit`, added live preview card
  (~200 lines).
