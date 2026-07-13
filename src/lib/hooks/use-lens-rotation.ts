"use client";

// FID-017 — useLensRotation selector hook.
//
// Given a lens index, returns the active lens from the 19-entry
// LENSES array plus next/prev previews. This is a pure selector
// (not a state machine) — the index lives in Tauri AppState (Rust)
// and is mirrored to React via the renderer's local state.
//
// Distinct from `use-derived-rotation` (which is the daily LS_DERIVED
// cron for session key rotation, per FID-0003 OQ-4). This hook is
// for the 12-lens cognitive rotation only.
//
// Path note: this hook imports from `@/lib/reflections/lenses` (renamed
// from `inner-monologue` on 2026-07-13 per Spencer: the dashboard feature
// is called "reflections", not "monologue" — "monologue" is the
// savant-orig Rust terminology that stays in the vendored code).

import {
  LENSES,
  EMERGENT_LENSES,
  OPERATIONAL_LENSES,
} from "@/lib/reflections/lenses";

export type LensType = "EMERGENT" | "OPERATIONAL" | "UNKNOWN";

export type LensView = {
  name: string;
  prompt: string;
  type: LensType;
  index: number;
  nextName: string;
  prevName: string;
  rotationPosition: number;
  rotationTotal: number;
};

export function useLensRotation(index: number): LensView {
  const total = LENSES.length;
  const safeIndex = ((index % total) + total) % total;
  const current = LENSES[safeIndex];
  const next = LENSES[(safeIndex + 1) % total];
  const prev = LENSES[(safeIndex - 1 + total) % total];
  const type: LensType = EMERGENT_LENSES.has(current[0])
    ? "EMERGENT"
    : OPERATIONAL_LENSES.has(current[0])
      ? "OPERATIONAL"
      : "UNKNOWN";
  return {
    name: current[0],
    prompt: current[1],
    type,
    index: safeIndex,
    nextName: next[0],
    prevName: prev[0],
    rotationPosition: safeIndex + 1,
    rotationTotal: total,
  };
}
