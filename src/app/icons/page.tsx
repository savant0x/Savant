"use client";

import { useMemo, useState } from "react";
import { Card } from "@heroui/react";
import { iconRegistry, iconNames } from "@/components/icons";

// Dashboard showcase for the installed Hover animated icon pack
// (https://www.itshover.com/icons). The filterable grid below renders
// all 273 icons. Hover any icon to trigger its animation.
//
// First section ("Dashboard Mapping Candidates") previews the proposed
// FontAwesome → Hover swap for `src/components/dashboard-shell.tsx` —
// current FA + 1-3 Hover alternatives side-by-side per dashboard icon.
// Click any Hover candidate to mark it selected; the first candidate
// in each row is the proposed default. The `Copy selections` button
// writes a JSON map (label → selected icon name) to the clipboard so
// Spencer can paste it back instead of typing each icon individually.

// Mapping source-of-truth for the dashboard-shell.tsx sweep below.
// Each row: {fa: current FA class, hover: candidate Hover component names}.
// The first name in `hover` is the proposed default in dashboard-shell.tsx.
const DASHBOARD_MAPPING_CANDIDATES: ReadonlyArray<{
  label: string;
  fa: string;
  hover: ReadonlyArray<string>;
}> = [
  // ─── Left rail nav (13 items) ─────────────────────────────────────
  { label: "Chat (id: chat)", fa: "fa-message", hover: ["MessageCircleIcon"] },
  { label: "Swarm Broadcast (id: swarm)", fa: "fa-tower-broadcast", hover: ["RadioIcon", "SatelliteDishIcon"] },
  { label: "Manifest Soul (id: manifest)", fa: "fa-wand-magic-sparkles", hover: ["SparklesIcon"] },
  { label: "Evolution (id: evolution)", fa: "fa-dna", hover: ["DinoIcon", "TreeIcon", "LayersIcon", "KeyframesIcon"] },
  { label: "Fine-Tuning (id: tune)", fa: "fa-sliders", hover: ["SlidersHorizontalIcon"] },
  { label: "Changelog (id: changelog)", fa: "fa-clipboard-list", hover: ["LibraryIcon", "FileDescriptionIcon", "UnorderedListIcon"] },
  { label: "Settings (id: settings)", fa: "fa-gear", hover: ["GearIcon"] },
  { label: "Marketplace (id: marketplace)", fa: "fa-store", hover: ["CartIcon", "ShoppingCartIcon"] },
  { label: "MCP (id: mcp)", fa: "fa-plug", hover: ["PlugConnectedIcon"] },
  { label: "Health (id: health)", fa: "fa-heart-pulse", hover: ["ScanHeartIcon", "HeartIcon", "GaugeIcon"] },
  { label: "FAQ (id: faq)", fa: "fa-circle-question", hover: ["QuestionMark", "InfoCircleIcon"] },
  { label: "Browser (id: browser)", fa: "fa-globe", hover: ["GlobeIcon", "WorldIcon"] },
  { label: "Reflections (id: reflections)", fa: "fa-brain", hover: ["BrainCircuitIcon"] },
  // ─── Left rail footer (theme toggle) ──────────────────────────────
  { label: "Theme (collapsed, dark→light)", fa: "fa-sun", hover: ["BulbSvg"] },
  { label: "Theme (collapsed, light→dark)", fa: "fa-moon", hover: ["MoonIcon"] },
  { label: "Theme (expanded, current=dark)", fa: "fa-moon", hover: ["MoonIcon"] },
  { label: "Theme (expanded, current=light)", fa: "fa-sun", hover: ["BulbSvg"] },
  // ─── Fold chevrons (left rail + right inspector) ──────────────────
  { label: "Fold-right (collapse left-rail)", fa: "fa-chevron-left", hover: ["ArrowNarrowLeftIcon", "ArrowBigLeftIcon", "ArrowBackIcon"] },
  { label: "Fold-left (expand left-rail)", fa: "fa-chevron-right", hover: ["ArrowNarrowRightIcon", "ArrowBigRightIcon"] },
  { label: "Inspector fold-left (collapse)", fa: "fa-chevron-right", hover: ["ArrowNarrowRightIcon", "ArrowBigRightIcon"] },
  { label: "Inspector fold-right (expand)", fa: "fa-chevron-left", hover: ["ArrowNarrowLeftIcon", "ArrowBigLeftIcon", "ArrowBackIcon"] },
  // ─── Right inspector icons (4 unique) ─────────────────────────────
  { label: "Inspector header (collapsed)", fa: "fa-magnifying-glass-chart", hover: ["MagnifierIcon", "ChartCovariateIcon"] },
  { label: "Vault section heading", fa: "fa-vault", hover: ["LockIcon", "ShieldCheck"] },
  { label: "Activity section heading", fa: "fa-wave-square", hover: ["HistoryCircleIcon", "StackIcon", "ChartLineIcon"] },
  { label: "Empty-state / info indicator", fa: "fa-circle-info", hover: ["InfoCircleIcon", "TriangleAlertIcon"] },
];

function buildDefaultSelections(): Record<string, string> {
  return Object.fromEntries(
    DASHBOARD_MAPPING_CANDIDATES.map(({ label, hover }) => [label, hover[0]]),
  );
}

// Format the clipboard payload as a JSON map (label → selected icon
// name) with a leading comment so it's pasteable into chat for
// round-tripping back to the assistant. JSON-with-comments is valid
// in modern runtimes; we trim the comment if a parser needs pure JSON.
function buildClipboardPayload(
  selections: Record<string, string>,
): string {
  const body = DASHBOARD_MAPPING_CANDIDATES
    .map(({ label }, i) => {
      const value = selections[label] ?? DASHBOARD_MAPPING_CANDIDATES[i].hover[0];
      const comma = i < DASHBOARD_MAPPING_CANDIDATES.length - 1 ? "," : "";
      return `  "${label}": "${value}"${comma}`;
    })
    .join("\n");
  return (
    `// Savant Dashboard Hover-Icon Selections — paste this back to me.\n` +
    `// Format: { "label (id: X)" | "label (no nav-id)": "HoverIconName" }\n` +
    `// To apply: rows with "(id: X)" map to the X icon field in\n` +
    `//   SYSTEM_NAV_ITEMS / PAGE_NAV_ITEMS in dashboard-shell.tsx;\n` +
    `//   other rows are inline JSX in dashboard-shell.tsx (theme toggle,\n` +
    `//   fold chevrons, inspector icons).\n` +
    `{\n${body}\n}\n`
  );
}

// Module-level component (hoisted per code-reviewer feedback — avoids
// inner-fn-per-render identity churn).
function DashboardMappingCandidates() {
  // Initial state: every row's first candidate (the proposed default).
  const [selections, setSelections] = useState<Record<string, string>>(
    buildDefaultSelections,
  );
  const [status, setStatus] = useState<{
    kind: "ok" | "err" | "info";
    message: string;
  } | null>(null);

  const selectOption = (label: string, name: string): void => {
    setSelections((prev) => ({ ...prev, [label]: name }));
  };

  const resetToDefaults = (): void => {
    setSelections(buildDefaultSelections());
    setStatus({
      kind: "info",
      message: "Reset all rows to their first-candidate defaults.",
    });
  };

  const copySelections = async (): Promise<void> => {
    const payload = buildClipboardPayload(selections);
    try {
      await navigator.clipboard.writeText(payload);
      const count = Object.keys(selections).length;
      setStatus({
        kind: "ok",
        message: `✓ Copied ${count} selections to clipboard.`,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setStatus({
        kind: "err",
        message: `✗ Clipboard copy failed (${msg}). Use Reset + manual selection on dashboard-shell.tsx instead.`,
      });
    }
  };

  // Cache the payload string so re-renders don't recompute it; only
  // invalidates when `selections` changes.
  const payloadPreview = useMemo(
    () => buildClipboardPayload(selections),
    [selections],
  );

  return (
    <section className="mb-10 rounded-lg border border-default bg-content1 p-6">
      <header className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="font-mono text-lg font-bold tracking-tight">
            Dashboard Mapping Candidates
          </h2>
          <p className="text-xs text-muted">
            Click any Hover tile to mark it selected. First candidate in
            each row is the dashboard-shell.tsx default.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={copySelections}
            className="rounded-md border border-accent/60 bg-accent/10 px-3 py-1.5 font-mono text-[10px] font-semibold uppercase tracking-[0.15em] text-accent transition-all hover:border-accent hover:bg-accent/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
            title="Copy all selections as JSON to clipboard"
          >
            Copy selections
          </button>
          <button
            type="button"
            onClick={resetToDefaults}
            className="rounded-md border border-default/60 bg-surface px-3 py-1.5 font-mono text-[10px] font-semibold uppercase tracking-[0.15em] text-muted transition-all hover:border-default hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
            title="Reset every row to its first-candidate default"
          >
            Reset
          </button>
        </div>
      </header>

      {status && (
        <p
          role="status"
          aria-live="polite"
          className={[
            "mb-4 rounded-md border px-3 py-2 font-mono text-[11px]",
            status.kind === "ok"
              ? "border-accent/40 bg-accent/10 text-accent"
              : status.kind === "err"
                ? "border-danger/40 bg-danger/10 text-danger"
                : "border-default/40 bg-surface-secondary/30 text-muted",
          ].join(" ")}
        >
          {status.message}
        </p>
      )}

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
        {DASHBOARD_MAPPING_CANDIDATES.map(({ label, fa, hover }) => (
          <div
            key={label}
            className="flex flex-wrap items-center gap-4 rounded-md border border-default/40 bg-content2 p-3"
          >
            <div className="flex shrink-0 flex-col items-center gap-1">
              {/*
                The FA '<i>' here is a plain HTML element (not <img>),
                so @next/next/no-img-element does NOT fire. Tooltip on
                '<i>' is valid HTML. We keep it as a literal '<i>' for
                parity with the original FA markup.
              */}
              <i
                className={`fas ${fa} text-2xl text-foreground`}
                aria-hidden
                title="Current FontAwesome"
              />
              <code className="text-[9px] text-muted">fas {fa}</code>
            </div>
            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-3">
              {hover.map((name) => {
                // CHECK: tsconfig.json noUncheckedIndexedAccess. Currently
                // OFF — this lookup uses a generic string key (from the
                // DASHBOARD_MAPPING_CANDIDATES iteration), so if the flag
                // is ever flipped, `Icon` becomes IconComponent | undefined
                // here and the JSX below breaks. Either flip with a
                // `Icon!` non-null assertion OR add a runtime
                // `name in iconRegistry` guard.
                const Icon = iconRegistry[name];
                const isSelected = selections[label] === name;
                return (
                  <button
                    key={name}
                    type="button"
                    onClick={() => selectOption(label, name)}
                    aria-pressed={isSelected}
                    title={
                      isSelected
                        ? `Selected: ${name}`
                        : `Click to select ${name}`
                    }
                    className={[
                      "flex shrink-0 flex-col items-center gap-1 rounded-md p-2 transition-all cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
                      isSelected
                        ? "ring-2 ring-accent bg-accent/15 scale-105"
                        : "hover:bg-surface-secondary/40 hover:ring-1 hover:ring-default/60",
                    ].join(" ")}
                  >
                    <Icon size={28} className="text-foreground" />
                    <code
                      className={[
                        "max-w-[110px] truncate text-center text-[9px]",
                        isSelected ? "text-accent font-semibold" : "text-muted",
                      ].join(" ")}
                    >
                      {name}
                    </code>
                  </button>
                );
              })}
            </div>
            <div className="hidden shrink-0 font-mono text-[10px] uppercase tracking-wider text-muted sm:block">
              {label}
            </div>
          </div>
        ))}
      </div>

      {/* Clipboard payload preview — gives Spencer a visual confirmation
          of what will land in their clipboard. Hidden when identical
          to the default payload (saves space; the first render shows
          the post-default preview). */}
      <details className="mt-4">
        <summary className="cursor-pointer font-mono text-[10px] uppercase tracking-wider text-muted hover:text-foreground">
          View clipboard payload preview
        </summary>
        <pre className="mt-2 max-h-48 overflow-auto rounded-md border border-default/40 bg-surface px-3 py-2 text-[10px] leading-snug text-muted">
          {payloadPreview}
        </pre>
      </details>
    </section>
  );
}

export default function IconsPage() {
  const [query, setQuery] = useState("");

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return iconNames;
    return iconNames.filter((name) => name.toLowerCase().includes(q));
  }, [query]);

  return (
    <main className="mx-auto max-w-6xl p-6">
      <DashboardMappingCandidates />
      <header className="mb-6">
        <h1 className="font-mono text-2xl font-bold tracking-tight">
          Hover Icons
        </h1>
        <p className="mt-1 text-sm text-muted">
          Animated icon pack from itshover.com — {iconNames.length} icons
          installed. Hover any icon to play its animation.
        </p>
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter icons…"
          className="mt-4 w-full max-w-sm rounded-lg border border-default bg-content1 px-3 py-2 text-sm text-foreground outline-none focus:ring-2 focus:ring-primary"
        />
      </header>

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 md:grid-cols-6 lg:grid-cols-8">
        {filtered.map((name) => {
          const Icon = iconRegistry[name];
          return (
            <Card
              key={name}
              className="flex flex-col items-center gap-2 p-4"
            >
              <Icon size={28} className="text-foreground" />
              <span className="break-all text-center text-[10px] leading-tight text-muted">
                {name}
              </span>
            </Card>
          );
        })}
      </div>

      {filtered.length === 0 && (
        <p className="mt-8 text-center text-sm text-muted">
          No icons match &ldquo;{query}&rdquo;.
        </p>
      )}
    </main>
  );
}
