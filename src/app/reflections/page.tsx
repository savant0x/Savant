"use client";

// FID-017 — /reflections page (Reflections viewer).
//
// Single unified stream layout (per Spencer 2026-07-13):
// 1. Control bar — consciousness state badge + start/stop daemon +
//    active model display + Force Reflection button
// 2. Reflection stream — full-width entries with proper date
//    header + Markdown-rendered body. The streaming entry appears
//    inline at the top with a "Reflecting…" indicator; once the
//    LLM call completes it joins the timeline via refresh().
//
// Design notes:
// - The lens rotation happens INTERNALLY (the mock IPC maintains
//   its own index in localStorage; the LLM prompt angle is picked
//   from the 19-entry LENSES array). The UI does NOT show the
//   active lens because the output stream is a single continuous
//   journal, not a per-lens partition (per Spencer: "all lenses
//   are supposed to be a single stream, not separated by lenses
//   but all joined together"). Showing the lens in the UI would
//   imply per-entry lens tagging, which the user explicitly
//   rejected.
// - "Live reflection" used to be a separate section above the
//   timeline. It was redundant: the empty box was always blank
//   between reflections, and the user would see a massive gap
//   before the timeline. The streaming indicator is now inline
//   at the top of the stream — the same visual real-estate, no
//   wasted space.
// - Each reflection box: `rounded-sm` (not `rounded-md` — less
//   pillowy, more journal-like), full-width, natural height (no
//   `max-h` + `overflow-y-auto` per entry), prose prose-invert for
//   Markdown rendering (so **bold** / *italic* / lists actually
//   render).
// - The whole stream scrolls as a single area, not per-entry.
// - The Force Reflection button is disabled when `!modelId` so
//   the user gets a clear visual signal that they need to set a
//   model in Settings before the click does anything.

import { useCallback, useEffect, useState } from "react";
import { Card } from "@heroui/react";
import { DashboardShell } from "@/components/dashboard-shell";
import {
  startConsciousness,
  stopConsciousness,
  getConsciousnessState,
  triggerReflection,
  type ConsciousnessState,
} from "@/lib/ipc";
import { useLoadedConfig } from "@/lib/hooks/use-loaded-config";
import { useReflections, type ReflectionEntry } from "@/lib/hooks/use-reflections";
import { formatFullTimestamp } from "@/lib/format-relative-time";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { logger } from "@/lib/logger";

export default function ReflectionsPage() {
  const [daemonState, setDaemonState] = useState<ConsciousnessState>("IDLE");
  const [daemonRunning, setDaemonRunning] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { reflections, loading, error: reflectionsError, refresh } = useReflections();
  const { modelId } = useLoadedConfig();

  // Poll the daemon state every 2s while running.
  useEffect(() => {
    if (!daemonRunning) return;
    const id = window.setInterval(async () => {
      try {
        const s = await getConsciousnessState();
        setDaemonState(s);
      } catch (e) {
        logger.warn("getConsciousnessState failed", {}, e);
      }
    }, 2000);
    return () => window.clearInterval(id);
  }, [daemonRunning]);

  const onStart = useCallback(async () => {
    setError(null);
    try {
      const initial = await startConsciousness();
      setDaemonState(initial);
      setDaemonRunning(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const onStop = useCallback(async () => {
    setError(null);
    try {
      await stopConsciousness();
      setDaemonState("IDLE");
      setDaemonRunning(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const onTrigger = useCallback(async () => {
    setError(null);
    setStreaming(true);
    try {
      // Pass the user-selected model from useLoadedConfig. The mock
      // throws a clear "No model configured" error if the user
      // hasn't picked one in Settings yet — we never override the
      // user's choice with a random default.
      await triggerReflection(undefined, modelId ?? undefined);
      // Refresh the stream to pick up the new entry from localStorage
      // (in browser preview) or the new REFLECTIONS.md write (in
      // Tauri runtime, would need a re-fetch).
      refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setStreaming(false);
    }
  }, [modelId, refresh]);

  return (
    <DashboardShell>
      <div className="flex h-full flex-col gap-4">
        {/* ── Control Bar ─────────────────────────────────────────── */}
        <Card className="rounded-none border border-default/30 px-5 py-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex flex-wrap items-center gap-3">
              <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
                Reflections
              </span>
              <span
                className={[
                  "rounded-sm border px-2 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-[0.2em]",
                  daemonState === "THINKING"
                    ? "border-accent/60 bg-accent/15 text-accent"
                    : daemonState === "WONDERING"
                      ? "border-success/60 bg-success/10 text-success"
                      : "border-default/40 bg-surface/30 text-muted",
                ].join(" ")}
                aria-live="polite"
              >
                {daemonState}
              </span>
              {daemonRunning ? (
                <button
                  type="button"
                  onClick={() => void onStop()}
                  className="flex items-center gap-2 rounded-sm border border-default/60 px-2.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.2em] text-muted transition-colors hover:border-danger hover:text-danger"
                >
                  <i className="fas fa-stop" aria-hidden /> Stop
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => void onStart()}
                  className="flex items-center gap-2 rounded-sm border border-accent bg-accent/15 px-2.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.2em] text-accent transition-colors hover:bg-accent/25"
                >
                  <i className="fas fa-play" aria-hidden /> Start Daemon
                </button>
              )}
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <span
                className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted"
                title={
                  modelId
                    ? `Using model: ${modelId}`
                    : "No model configured \u2014 set one in Settings before reflecting"
                }
              >
                {modelId ?? "no model set"}
              </span>
              <button
                type="button"
                onClick={() => void onTrigger()}
                disabled={streaming || !modelId}
                className="flex items-center gap-2 rounded-sm border border-accent bg-accent/15 px-3 py-0.5 font-mono text-[10px] uppercase tracking-[0.2em] text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {streaming ? (
                  <>
                    <i className="fas fa-spinner fa-spin" aria-hidden /> Reflecting…
                  </>
                ) : (
                  <>
                    <i className="fas fa-bolt" aria-hidden /> Force Reflection
                  </>
                )}
              </button>
            </div>
          </div>
          {error && (
            <p
              className="mt-2 font-mono text-[10px] uppercase tracking-[0.2em] text-danger"
              role="status"
            >
              {error}
            </p>
          )}
        </Card>

        {/* ── Reflection Stream (unified; scrolls as a whole) ─────── */}
        <div className="flex-1 overflow-y-auto pr-1">
          <div className="space-y-3 pb-8">
            {/* Streaming indicator — inline at top while an LLM call is in flight */}
            {streaming && (
              <div className="border border-accent/40 bg-accent/5 p-4">
                <div className="mb-2 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.2em] text-accent">
                  <i className="fas fa-spinner fa-spin" aria-hidden />
                  Reflecting…
                </div>
                <p className="font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
                  The new entry will appear here when the call completes.
                </p>
              </div>
            )}

            {/* Loading state — only on first load, not on refresh */}
            {loading && reflections.length === 0 && !streaming && (
              <div className="border border-default/30 bg-surface/20 p-4">
                <p className="font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
                  Loading reflections\u2026
                </p>
              </div>
            )}

            {/* Empty state — no config, no entries, not streaming */}
            {!loading && reflections.length === 0 && !streaming && !reflectionsError && (
              <div className="border border-default/30 bg-surface/20 p-4">
                <p className="font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
                  No reflections yet. {modelId
                    ? "Click Force Reflection to write the first entry."
                    : "Set a model in Settings \u2192 OpenRouter, then click Force Reflection."}
                </p>
              </div>
            )}

            {/* Error loading reflections — distinct from the control-bar error */}
            {reflectionsError && (
              <div className="border border-danger/40 bg-danger/5 p-4">
                <p className="font-mono text-[10px] uppercase tracking-[0.2em] text-danger">
                  {reflectionsError}
                </p>
              </div>
            )}

            {/* Past reflections — each at natural height, full width */}
            {reflections.map((r, idx) => (
              <article
                key={`${r.ts}-${idx}`}
                className="border border-default/30 bg-surface/20 p-4 transition-colors hover:border-accent/30"
              >
                <header className="mb-3 flex items-center gap-2 border-b border-default/20 pb-2 font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
                  <i className="far fa-clock text-[10px]" aria-hidden />
                  <time dateTime={r.ts}>{formatFullTimestamp(r.ts)}</time>
                </header>
                <MarkdownRenderer content={r.content} />
              </article>
            ))}
          </div>
        </div>
      </div>
    </DashboardShell>
  );
}
