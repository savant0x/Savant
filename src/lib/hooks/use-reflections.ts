"use client";

// FID-017 — useReflections hook.
//
// Boot-time reader for `workspace-savant/REFLECTIONS.md`. Parses the
// file into structured `ReflectionEntry` records using the journal-style
// `## [timestamp] <body>` format (no per-entry lens tag — the lens is
// internal to the prompt selection, not the output, per Spencer's
// correction on 2026-07-13: "all lenses are supposed to be a single
// stream, not separated by lenses but all joined together").
//
// Also exposes `SOUL.md` content (workspace-savant/SOUL.md) for the
// dashboard to display alongside the timeline.
//
// In browser preview (no Tauri runtime), falls back to the mock
// `MOCK_REFLECTIONS_KEY` localStorage entry written by
// mock-ipc.ts::trigger_reflection.
//
// Distinct from `useLoadedConfig` (which loads the AppConfig
// provider + model from the IPC vault). This hook is for the
// workspace files (SOUL.md + REFLECTIONS.md).

import { useCallback, useEffect, useState } from "react";

export type ReflectionEntry = {
  ts: string;
  content: string;
};

export type ReflectionsState = {
  soul: string | null;
  reflections: ReflectionEntry[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
};

const REFLECTIONS_PATH = "workspace-savant/REFLECTIONS.md";
const SOUL_PATH = "workspace-savant/SOUL.md";
// Shared with src/lib/mock-ipc.ts (single source of truth for the
// browser-preview REFLECTIONS.md stand-in). Renamed 2026-07-13 from
// `savant.monologue.reflections` for naming hygiene — see the
// one-time migration block in `loadReflections` below for the
// user-data preservation step.
const MOCK_REFLECTIONS_KEY = "savant.reflections.entries";
// Pre-rename localStorage key. The one-time migration in
// `loadReflections` reads this key (if present) and rewrites its
// data under the new key, then deletes it. Idempotent — only runs
// once per user, then this constant is unused at runtime (kept
// for the migration check; safe to remove in a future cleanup PR
// once we're confident all users have migrated).
const LEGACY_MOCK_REFLECTIONS_KEY = "savant.monologue.reflections";

/**
 * Parse a REFLECTIONS.md body into structured entries. Mirrors the
 * Rust consciousness daemon's narrative model: ONE continuous stream
 * of reflections, no per-entry lens tag (the lens is used internally
 * to pick the LLM prompt angle, not to partition the output).
 *
 * Format: `## YYYY-MM-DD HH:MM:SS UTC` header on its own line, followed
 * by the narrative body until the next `## ` header or EOF. Journal-style
 * (per Spencer's correction on 2026-07-13: "all lenses are supposed to
 * be a single stream, not separated by lenses but all joined together").
 */
function parseReflectionsMd(md: string): ReflectionEntry[] {
  const entries: ReflectionEntry[] = [];
  // Match `## ` at start-of-line OR after a newline. The captured
  // separator (1 char) is dropped from each part so the leading
  // header is at position 0 in its slice. We use a regex split
  // (not literal "### Learning (") so the format is lens-agnostic
  // and the consciousness daemon can write whatever timestamp
  // shape it wants (RFC3339, RFC3339 with nanos, etc.).
  const parts = md.split(/(^|\n)##\s+/);
  // parts[0] = content before first `## ` (skipped; pre-amble junk)
  // parts[1] = the `\n` or empty separator (dropped)
  // parts[2] = the body of the first `## ` header
  // parts[3] = separator
  // parts[4] = body of second header
  // ... interleaved [separator, body] pairs from index 1.
  for (let i = 2; i < parts.length; i += 2) {
    const part = parts[i];
    const newlineIdx = part.indexOf("\n");
    if (newlineIdx < 0) {
      // Header-only entry with no body — skip.
      continue;
    }
    const ts = part.slice(0, newlineIdx).trim();
    const body = part.slice(newlineIdx + 1).trim();
    if (!body) continue;
    entries.push({ ts, content: body });
  }
  return entries;
}

async function loadReflections(): Promise<{ soul: string | null; reflections: ReflectionEntry[]; error: string | null }> {
  if (typeof window === "undefined") {
    return { soul: null, reflections: [], error: null };
  }
  // One-time migration from the pre-rename localStorage key. Runs
  // exactly once per user: if the legacy key holds data AND the new
  // key is empty, copy the legacy data into the new key and delete
  // the legacy entry. Preserves any pre-rename reflections so the
  // user doesn't see an empty timeline after upgrading. The extra
  // `lens` field on legacy entries is preserved as-is — the new
  // `ReflectionEntry` type ignores it (TS allows extra fields), and
  // stripping would require a JSON round-trip + map that adds
  // complexity for a one-time data-cleanup pass.
  try {
    if (
      window.localStorage.getItem(LEGACY_MOCK_REFLECTIONS_KEY) !== null &&
      window.localStorage.getItem(MOCK_REFLECTIONS_KEY) === null
    ) {
      const legacy = window.localStorage.getItem(LEGACY_MOCK_REFLECTIONS_KEY);
      if (legacy !== null) {
        window.localStorage.setItem(MOCK_REFLECTIONS_KEY, legacy);
        window.localStorage.removeItem(LEGACY_MOCK_REFLECTIONS_KEY);
      }
    }
  } catch {
    /* quota / private-mode fail — leave the legacy entry in place;
       the next successful load will retry the migration. */
  }
  let reflections: ReflectionEntry[] = [];
  const mockRaw = window.localStorage.getItem(MOCK_REFLECTIONS_KEY);
  if (mockRaw) {
    try {
      const parsed = JSON.parse(mockRaw) as ReflectionEntry[];
      reflections = Array.isArray(parsed) ? parsed : [];
    } catch {
      reflections = [];
    }
  }
  let soul: string | null = null;
  // SOUL.md lives at `workspace-savant/SOUL.md` in the Tauri runtime
  // filesystem (not in the Next.js `public/` dir), so a `fetch()` from
  // the browser would always 404 and spam the dev console. Only attempt
  // the fetch when the Tauri runtime is present (the `__TAURI_INTERNALS__`
  // global is set by Tauri's webview shim — same check `mock-ipc.ts` uses
  // to decide whether to install the IPC mock). In Tauri runtime the
  // file is served by the Tauri asset protocol, not by Next.js. Phase 2
  // will replace this with a `read_soul_md` Tauri command for proper
  // permissioning + caching (FID-018+).
  if ("__TAURI_INTERNALS__" in window) {
    try {
      const soulRes = await fetch(SOUL_PATH);
      if (soulRes.ok) soul = await soulRes.text();
    } catch {
      /* transient network error — leave null */
    }
  }
  return { soul, reflections, error: null };
}

export function useReflections(): ReflectionsState {
  const [soul, setSoul] = useState<string | null>(null);
  const [reflections, setReflections] = useState<ReflectionEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      try {
        const result = await loadReflections();
        if (!cancelled) {
          setSoul(result.soul);
          setReflections(result.reflections);
          setError(result.error);
          setLoading(false);
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tick]);

  // Refresh — call after a successful triggerReflection to pick up
  // the new entry from localStorage. Cheap; just re-runs the load.
  const refresh = useCallback(() => {
    setTick((t) => t + 1);
  }, []);

  return { soul, reflections, loading, error, refresh };
}

// Re-export the parser for callers that want to re-parse a raw body.
export { parseReflectionsMd, REFLECTIONS_PATH, SOUL_PATH };
