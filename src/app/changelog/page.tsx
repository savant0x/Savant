"use client";

// FID-028 + Spencer revision 2026-07-14 — Changelog page wired to
// the real engine (GitHub), with proper HeroUI scroll pattern.
//
// Per Spencer: "the changelog needs to come from github because
// that's the source of truth for the changelog, other people will
// be downloading this and will not have the changlog locally, the
// project on gh does."
//
// Per Spencer (same session): "The changelog page looks absolutely
// horrible. It does not even scroll. This needs to properly use
// hero." Fix: single HeroUI Card as the scroll container, with a
// sticky header (title + source label + Refresh button) and a
// flex-1 overflow-y-auto body for the markdown. The Card's
// `overflow-hidden` + body's `overflow-y-auto` is the canonical
// HeroUI scroll pattern (mirrors the reflections page's body
// scroll area; the changelog differs in that ONE Card wraps BOTH
// the header + body for visual cohesion — a single cohesive
// document rather than a feed of separate entries).
//
// Source: `https://raw.githubusercontent.com/savant0x/Savant/main/CHANGELOG.md`
// (the canonical repo per `scripts/release.py:33` + `package.json` +
// the v0.0.5 identity rename). Fetched at runtime by
// `src/lib/changelog.ts::fetchChangelog()`. The local `CHANGELOG.md`
// is for the developer's reference; it is NOT bundled with the
// shipped app.
//
// Error UX: a clear danger-colored error block with a "Retry"
// button in the scrollable body, plus a "Refresh" button in the
// always-visible header. The user does not have to refresh the
// whole page to recover from a transient GitHub outage.

import { useCallback, useEffect, useState } from "react";
import { Card } from "@heroui/react";
import { DashboardShell } from "@/components/dashboard-shell";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { getChangelog } from "@/lib/ipc";
import { CHANGELOG_SOURCE_LABEL } from "@/lib/changelog";
import { logger } from "@/lib/logger";

export default function ChangelogPage() {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const loadChangelog = useCallback(async (): Promise<void> => {
    setLoading(true);
    setError(null);
    try {
      const md = await getChangelog();
      setContent(md);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setContent(null);
      logger.warn("getChangelog failed", {}, e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadChangelog();
  }, [loadChangelog]);

  return (
    <DashboardShell>
      <div className="flex h-full flex-col">
        <Card className="flex flex-1 flex-col overflow-hidden p-0">
          {/* Sticky header — title + source label + Refresh */}
          <header className="flex flex-wrap items-center justify-between gap-3 border-b border-default/30 px-5 py-4">
            <div>
              <h2 className="font-mono text-sm font-semibold uppercase tracking-[0.18em] text-foreground">
                System Changelog
              </h2>
              <p className="mt-1 font-mono text-[9px] uppercase tracking-[0.2em] text-muted">
                source: {CHANGELOG_SOURCE_LABEL}
              </p>
            </div>
            <button
              type="button"
              onClick={() => void loadChangelog()}
              disabled={loading}
              className="flex items-center gap-2 rounded-sm border border-default/60 px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.2em] text-muted transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
              aria-label="Refresh changelog from GitHub"
            >
              <i className="fas fa-arrows-rotate" aria-hidden />
              {loading ? "Refreshing…" : "Refresh"}
            </button>
          </header>

          {/* Scrollable body — loading / error / content (mutually
              exclusive states). The Card's overflow-hidden +
              body's overflow-y-auto is the canonical HeroUI
              scroll pattern (the Card is the visual boundary; the
              body div is the scroll container). */}
          <div className="flex-1 overflow-y-auto px-6 py-4">
            {error && (
              <div
                className="mb-4 flex flex-wrap items-center gap-3 rounded-sm border border-danger/40 bg-danger/5 px-4 py-3"
                role="alert"
              >
                <p className="flex-1 font-mono text-[10px] uppercase tracking-[0.2em] text-danger">
                  {error}
                </p>
                <button
                  type="button"
                  onClick={() => void loadChangelog()}
                  disabled={loading}
                  className="flex items-center gap-2 rounded-sm border border-accent bg-accent/15 px-3 py-1 font-mono text-[10px] uppercase tracking-[0.2em] text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
                  aria-label="Retry fetching changelog"
                >
                  <i className="fas fa-arrows-rotate" aria-hidden />
                  {loading ? "Retrying…" : "Retry"}
                </button>
              </div>
            )}
            {content === null && !error && (
              <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
                Loading changelog from GitHub…
              </p>
            )}
            {content !== null && <MarkdownRenderer content={content} />}
          </div>
        </Card>
      </div>
    </DashboardShell>
  );
}
