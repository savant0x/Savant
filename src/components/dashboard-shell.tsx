"use client";

// DashboardShell — the 3-panel layout (foldable rail + center + inspector)
// shared by every route in src/app/*/page.tsx.
//
// Active nav detection: uses usePathname() to derive the active id from
// the current URL. Click a nav item and the URL changes; the active
// highlight follows automatically (no local setState needed).
//
// Right-edge active bar: each nav item is `border-r-2 border-accent` when
// active (was border-l-2 in earlier versions). The 2px accent bar sits
// flush against the rail's right border so the eye reads it as a
// "you are here" marker on the inside-right of the column.
//
// Iconography (2026-07-14 — phase-2 sweep per FID-027 Hover-pack install):
//   - Nav-row icons render from `@/components/icons` (Hover pack, animated
//     client components). The `icon` field on each nav item is a key into
//     `iconRegistry` (NOT a FontAwesome className). Best-fit mappings for
//     the 4 ambiguous icons (evolution, changelog, health, theme-sun) are
//     proposed defaults — tune per visual review at /icons §Dashboard
//     Mapping Candidates.
//   - Left-rail footer theme toggle + fold chevrons + right-inspector
//     icons similarly migrated from `fas fa-...` to named imports from
//     the Hover pack. Animated-on-hover is the upgrade; semantics preserved.
//
// Theme: dark-first. The <html> element ships with data-theme="dark"
// (server-rendered in layout.tsx). The footer theme toggle flips the
// html attribute client-side; HeroUI v3 swaps design tokens automatically.
//
// Logo: /img/logo.png (Next.js serves public/* at /).
// Children: each page provides its own center content below the header.

import {
  useEffect,
  useRef,
  useState,
  type ForwardRefExoticComponent,
  type PropsWithoutRef,
  type ReactNode,
  type RefAttributes,
} from "react";
import type { AnimatedIconHandle, AnimatedIconProps } from "@/components/icons/types";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { Separator } from "@heroui/react";
import { listProfiles, type ProfileSummary } from "@/lib/ipc";
import { logger } from "@/lib/logger";
import {
  iconRegistry,
  BulbSvg,
  MoonIcon,
  ArrowNarrowLeftIcon,
  ArrowNarrowRightIcon,
  MagnifierIcon,
  LockIcon,
  HistoryCircleIcon,
  InfoCircleIcon,
} from "@/components/icons";

type Theme = "dark" | "light";

// ForwardRef-typed component for the nav icons. The iconRegistry
// returns `ComponentType<AnimatedIconProps>` (no ref), so we need a
// separate type to attach a ref to call the imperative
// startAnimation() / stopAnimation() methods on each nav icon (see
// the onMouseEnter/onMouseLeave wiring on each <Link> below; this
// makes the menu TEXT hover trigger the icon's animation, not just
// hovering the SVG itself).
type NavIconComponent = ForwardRefExoticComponent<
  PropsWithoutRef<AnimatedIconProps> & RefAttributes<AnimatedIconHandle>
>;

// ─── Nav items (Savant-backup convention) ──────────────────────────────
// `icon` is a key into `iconRegistry` (273 entries from the Hover pack).
// Source order is conceptual (default-first, feature priority). The
// sidebar sorts alphabetically at runtime when building NAV_SECTIONS,
// so future agents can drop a new item anywhere in either array and the
// display order stays correct without a manual re-sort.
const SYSTEM_NAV_ITEMS = [
  { id: "chat", href: "/chat", label: "Chat with Savant", icon: "MessageCircleIcon" },
  { id: "swarm", href: "/", label: "Swarm Broadcast", icon: "RadioIcon" },
  { id: "manifest", href: "/manifest", label: "Manifest Soul", icon: "SparklesIcon" },
] as const;

const PAGE_NAV_ITEMS = [
  { id: "evolution", href: "/evolution", label: "Evolution", icon: "DinoIcon" },
  { id: "tune", href: "/tune", label: "Fine-Tuning", icon: "SlidersHorizontalIcon" },
  { id: "changelog", href: "/changelog", label: "Changelog", icon: "LibraryIcon" },
  { id: "settings", href: "/settings", label: "Settings", icon: "GearIcon" },
  { id: "marketplace", href: "/marketplace", label: "Marketplace", icon: "CartIcon" },
  { id: "mcp", href: "/mcp", label: "MCP", icon: "PlugConnectedIcon" },
  { id: "health", href: "/health", label: "Health", icon: "ScanHeartIcon" },
  { id: "faq", href: "/faq", label: "FAQ", icon: "QuestionMark" },
  { id: "browser", href: "/browser", label: "Browser", icon: "GlobeIcon" },
  // FID-017 — Reflections viewer (12-lens rotation + REFLECTIONS.md timeline)
  { id: "reflections", href: "/reflections", label: "Reflections", icon: "BrainCircuitIcon" },
] as const;

// Display order: items are sorted by label so the sidebar reads
// top-to-bottom alphabetically. `[...arr].sort()` copies before sorting
// so the underlying `as const` arrays stay untouched (other consumers
// like ALL_NAV_ITEMS keep their source order).
const NAV_SECTIONS = [
  {
    label: "System",
    items: [...SYSTEM_NAV_ITEMS].sort((a, b) => a.label.localeCompare(b.label)),
  },
  {
    label: "Pages",
    items: [...PAGE_NAV_ITEMS].sort((a, b) => a.label.localeCompare(b.label)),
  },
];

const ALL_NAV_ITEMS = [...SYSTEM_NAV_ITEMS, ...PAGE_NAV_ITEMS];

// ─── FoldToggleButton (extracted 2026-07-14) ──────────────────────────
// Single source-of-truth for the fold chevron on BOTH rails (left rail
// + right inspector). Renders the same ArrowNarrowRightIcon in both
// positions; CSS `transform: scaleX(-1)` mirrors the arrow on the
// right side so the design language is identical across rails. The
// icon orientation encodes the action ("expand into screen" /
// "collapse toward edge") consistently. Position (`-right-3` vs
// `-left-3`) is derived from the `side` prop.
function FoldToggleButton({
  side,
  isCollapsed,
  onToggle,
  label,
}: {
  side: "left" | "right";
  isCollapsed: boolean;
  onToggle: () => void;
  label: string;
}) {
  // Icon selection by (side, isCollapsed). Both icons share the same
  // SVG path; ArrowNarrowRightIcon is the canonical right-pointing
  // variant, ArrowNarrowLeftIcon is its left-pointing mirror. We
  // use the dedicated component instead of a CSS scaleX(-1)
  // transform: the dedicated component renders the SVG without the
  // inline-flex parent shift that the transform introduced (per
  // Spencer's 2026-07-14 "right sidebar icon is contained" feedback,
  // the transform made the right side feel stuck inside the
  // inspector gradient rather than protruding as a tab).
  const Icon =
    side === "left"
      ? isCollapsed
        ? ArrowNarrowRightIcon
        : ArrowNarrowLeftIcon
      : isCollapsed
        ? ArrowNarrowLeftIcon
        : ArrowNarrowRightIcon;
  return (
    <button
      onClick={onToggle}
      aria-label={isCollapsed ? `Expand ${label}` : `Collapse ${label}`}
      title={isCollapsed ? `Expand ${label}` : `Collapse ${label}`}
      className={[
        "absolute top-1/2 z-20 flex h-8 w-8 -translate-y-1/2 items-center justify-center rounded-md border border-default/60 bg-surface text-muted shadow-md transition-all duration-200 hover:scale-110 hover:border-accent hover:text-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
        side === "left" ? "-right-3" : "-left-3",
      ].join(" ")}
    >
      {/*
        `aria-hidden` is intentionally NOT passed here: the Hover-pack
        icon source destructures { size, color, strokeWidth, className }
        and drops everything else, so any `aria-hidden` we set would
        silently disappear at runtime. The parent <button>'s aria-label
        ("Expand menu" / "Collapse inspector") is the source of truth
        for assistive tech; the icon is decorative inline SVG.
      */}
      <Icon size={16} />
    </button>
  );
}

export function DashboardShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  // Derive active id from the URL: "/" → "swarm", "/evolution" → "evolution", etc.
  const activeId =
    pathname === "/" ? "swarm" : pathname.replace(/^\//, "").split("/")[0];
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [theme, setTheme] = useState<Theme>("dark");
  const [collapsed, setCollapsed] = useState(false);
  // Right Inspector — independent fold state. Same chevron-toggle
  // convention as the left rail (240px ↔ 64px → 320px ↔ 48px).
  // Persisted per-mount only; resets on refresh to match the left rail.
  const [rightCollapsed, setRightCollapsed] = useState(false);
  // Per-item ref to each nav icon's imperative handle. The Hover-pack
  // icon's `onHoverStart` only fires when the cursor is over the SVG
  // itself; we extend that to the surrounding <Link>'s onMouseEnter
  // so hovering the menu text also triggers the icon's hover
  // animation. Map keyed by `id` so each Link can look up its own
  // handle without per-render prop threading. Entries are added on
  // mount + removed on unmount via the ref callback.
  const navIconRefs = useRef<Map<string, AnimatedIconHandle>>(new Map());

  useEffect(() => {
    const current = (document.documentElement.dataset.theme as Theme) || "dark";
    setTheme(current);
  }, []);

  // ECHO Law 14 — surface the error to the user via UI state (not a
  // silent swallow). The Settings page also reads `vault_list_profiles`
  // and shows its own error path; this DashboardShell-level error is
  // for the right-rail Inspector's vault summary. If `profiles` fails
  // to load, we render an empty list + a `vaultError` indicator in
  // the Inspector (handled in the JSX below).
  const [vaultError, setVaultError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const p = await listProfiles();
        if (!cancelled) {
          setProfiles(p);
          setVaultError(null);
        }
      } catch (err) {
        if (!cancelled) {
          const msg = err instanceof Error ? err.message : String(err);
          logger.error("listProfiles failed", { scope: "DashboardShell" }, err);
          setProfiles([]);
          setVaultError(msg);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const toggleTheme = (): void => {
    const next: Theme = theme === "dark" ? "light" : "dark";
    setTheme(next);
    document.documentElement.dataset.theme = next;
  };

  const activeLabel =
    ALL_NAV_ITEMS.find((n) => n.id === activeId)?.label.toUpperCase() ??
    "SAVANT";

  // Path to the page file the developer should edit, shown as a hint in
  // the center header. "/" → src/app/page.tsx; "/evolution" → src/app/evolution/page.tsx.
  const filePath =
    pathname === "/" ? "src/app/page.tsx" : `src/app${pathname}/page.tsx`;

  return (
    <div
      className="grid h-screen w-screen overflow-hidden bg-background text-foreground transition-[grid-template-columns] duration-300 ease-out"
      style={{
        gridTemplateColumns: (() => {
          const left = collapsed ? "64px" : "240px";
          const right = rightCollapsed ? "48px" : "320px";
          return `${left} minmax(0, 1fr) ${right}`;
        })(),
      }}
    >
      {/* ─── Left Rail ─────────────────────────────────────────────── */}
      <aside className="relative flex flex-col border-r border-default/60 bg-surface/30">
        {/* Brand — logo doubled (40px → 80px) */}
        <header
          className={
            collapsed
              ? "flex items-center justify-center border-b border-default/60 p-3"
              : "flex items-center gap-4 border-b border-default/60 p-4"
          }
        >
          <Link
            href="/"
            aria-label="Savant home"
            className="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-lg"
          >
            {/* ECHO Law 13 (utility-first): we intentionally use the
                native <img> instead of `next/image` here. Reasons:
                1. `output: "export"` in next.config.mjs requires
                   `images: { unoptimized: true }` for `next/image`,
                   which defeats the optimization benefit.
                2. The onError handler hides the img on 404 — the
                   next/image component does not support an `onError`
                   prop the same way (it uses a fallback image
                   prop instead, which we don't want for a logo
                   that may simply be missing in dev).
                3. The asset is a local static file under /public
                   (no remote domain configuration needed).
                The eslint-disable is the documented escape hatch
                for this specific use case per the Next.js docs. */}
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img
              src="/img/logo.png"
              alt="Savant"
              className="h-12 w-12 object-contain"
              onError={(e) => {
                e.currentTarget.style.display = "none";
              }}
            />
          </Link>
          {!collapsed && (
            <div className="min-w-0 flex-1">
              <h1 className="truncate font-mono text-sm font-semibold uppercase tracking-[0.3em] text-foreground">
                Savant
              </h1>
              <p className="mt-0.5 font-mono text-[9px] uppercase tracking-[0.2em] text-muted">
                v0.0.1 · proactive
              </p>
            </div>
          )}
        </header>

        {/* Nav — 2 sections (System, Pages) with right-edge active bar */}
        <nav className="flex flex-1 flex-col gap-0.5 overflow-y-auto p-2">
          {NAV_SECTIONS.map((section, idx) => (
            <div key={section.label} className="flex flex-col gap-0.5">
              {idx > 0 && (
                <div
                  className="my-1.5 mx-3 h-px bg-default/60"
                  role="separator"
                  aria-hidden
                />
              )}
              {!collapsed && (
                <div className="mt-3 mb-1 mx-3 flex items-center gap-2 border-b border-default/60 pb-1.5">
                  <span
                    className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent shadow-[0_0_4px_var(--accent)]"
                    aria-hidden
                  />
                  <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.18em] text-foreground">
                    {section.label}
                  </span>
                </div>
              )}
              {section.items.map(({ id, href, label, icon }) => {
                const active = activeId === id;
                // Hover-pack icon: animated client component from
                // src/components/icons. The `icon` field is a key into
                // iconRegistry (NOT a FontAwesome class string). See
                // /icons page's mapping-candidates section for the
                // full candidate table.
                // CHECK: tsconfig.json noUncheckedIndexedAccess (currently
                // OFF; would require `iconRegistry[icon]!` or `?`+fallback
                // if the strict flag is ever flipped).
                //
                // Cast to the ref-typed component (iconRegistry's
                // IconComponent is `ComponentType<AnimatedIconProps>`
                // which strips the ref type). The ref exposes
                // startAnimation() / stopAnimation() via
                // useImperativeHandle, which the parent <Link>'s
                // onMouseEnter / onMouseLeave handlers below call so
                // hovering the menu text (not just the icon) triggers
                // the icon's hover animation. The aria-hidden prop is
                // intentionally NOT passed: the icon's destructure
                // strips it (see FoldToggleButton's note), and the
                // parent <Link>'s aria-label is the source of truth
                // for AT.
                const NavIcon = iconRegistry[icon] as NavIconComponent;
                const setIconRef = (handle: AnimatedIconHandle | null): void => {
                  if (handle) {
                    navIconRefs.current.set(id, handle);
                  } else {
                    navIconRefs.current.delete(id);
                  }
                };
                return (
                  <Link
                    key={id}
                    href={href}
                    title={label}
                    aria-label={label}
                    aria-current={active ? "page" : undefined}
                    onMouseEnter={() =>
                      navIconRefs.current.get(id)?.startAnimation()
                    }
                    onMouseLeave={() =>
                      navIconRefs.current.get(id)?.stopAnimation()
                    }
                    className={[
                      "group relative flex items-center gap-3 rounded-md px-3 py-2 font-mono text-[11px] uppercase tracking-[0.15em] transition-all duration-200 no-underline",
                      collapsed ? "justify-center" : "justify-start",
                      active
                        ? "border-r-2 border-accent bg-accent/10 text-accent shadow-[inset_4px_0_8px_-4px_var(--accent)]"
                        : "border-r-2 border-transparent text-muted hover:bg-surface-secondary/40 hover:text-foreground",
                    ].join(" ")}
                  >
                    <NavIcon
                      ref={setIconRef}
                      size={18}
                      className="shrink-0"
                    />
                    {!collapsed && <span className="truncate">{label}</span>}
                  </Link>
                );
              })}
            </div>
          ))}
        </nav>

        {/* Footer: status + theme toggle */}
        <footer className="border-t border-default/60 p-3">
          {collapsed ? (
            <div className="flex flex-col items-center gap-3">
              <div
                className="flex h-2 w-2 items-center justify-center"
                title="System online"
              >
                <span className="h-2 w-2 rounded-full bg-success shadow-[0_0_6px_var(--success)]" />
              </div>
              <button
                onClick={toggleTheme}
                title={theme === "dark" ? "Switch to light" : "Switch to dark"}
                aria-label="Toggle theme"
                className="flex h-6 w-6 items-center justify-center rounded-md border border-default/40 text-muted transition-colors hover:border-accent hover:text-accent"
              >
                {/* Collapsed: icon shows NEXT ACTION. Dark→light = BulbSvg
                    (lightbulb); light→dark = MoonIcon. */}
                {theme === "dark" ? (
                  <BulbSvg size={14} aria-hidden />
                ) : (
                  <MoonIcon size={14} aria-hidden />
                )}
              </button>
            </div>
          ) : (
            <div className="flex flex-col gap-3">
              <div className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.2em]">
                <span className="relative flex h-1.5 w-1.5">
                  <span className="absolute inset-0 animate-ping rounded-full bg-success/60" />
                  <span className="relative h-1.5 w-1.5 rounded-full bg-success shadow-[0_0_4px_var(--success)]" />
                </span>
                <span className="text-muted">System</span>
                <span className="ml-auto text-success">Online</span>
              </div>
              <button
                onClick={toggleTheme}
                className="flex items-center justify-between font-mono text-[10px] uppercase tracking-[0.2em] text-muted transition-colors hover:text-foreground"
                aria-label="Toggle theme"
              >
                <span>Theme</span>
                {/* Expanded: icon shows CURRENT theme visualization.
                    Dark = MoonIcon; light = BulbSvg. */}
                <span className="flex items-center gap-1.5 text-foreground">
                  {theme === "dark" ? (
                    <MoonIcon size={14} aria-hidden />
                  ) : (
                    <BulbSvg size={14} aria-hidden />
                  )}
                  <span>{theme === "dark" ? "Dark" : "Light"}</span>
                </span>
              </button>
            </div>
          )}
        </footer>

        {/* Fold toggle — middle of the right edge. Extracted to the
            shared <FoldToggleButton side="left" /> for parity with
            the right inspector fold (mirrored via CSS scaleX). */}
        <FoldToggleButton
          side="left"
          isCollapsed={collapsed}
          onToggle={() => setCollapsed((c) => !c)}
          label="menu"
        />
      </aside>

      {/* ─── Center Canvas ─────────────────────────────────────────── */}
      <main className="flex flex-col overflow-auto p-8">
        <header className="mb-8">
          <p className="font-mono text-[10px] uppercase tracking-[0.3em] text-muted">
            Active view
          </p>
          <h1 className="mt-1 font-mono text-2xl font-semibold uppercase tracking-[0.2em] text-foreground">
            {activeLabel}
          </h1>
          <p className="mt-3 font-mono text-xs text-muted">
            Edit{" "}
            <code className="rounded bg-surface px-1.5 py-0.5 text-[11px] text-accent">
              {filePath}
            </code>{" "}
            to add content.
          </p>
        </header>
        {children}
      </main>

      {/* ─── Right Inspector ───────────────────────────────────────── */}
      <aside
        className={
          rightCollapsed
      ? "relative flex flex-col items-center border-l border-default/60 bg-gradient-to-b from-surface/30 to-surface/10"
      : "relative flex flex-col border-l border-default/60 bg-gradient-to-b from-surface/40 to-surface/20"
        }
      >
        <header className="flex w-full items-center justify-center border-b border-default/60 p-4">
          {rightCollapsed ? (
            // Hover-pack MagnifierIcon's AnimatedIconProps does NOT
            // expose a `title` prop (parent class for SVG aria attrs is
            // SVGAttributes, but the icon component strips non-standard
            // SVG props). The tooltip goes on the wrapping <span>;
            // aria-label on an empty parent is redundant when the child
            // is `aria-hidden` decorative.
            // CHECK: tsconfig.json noUncheckedIndexedAccess (currently
            // OFF; flip → `iconRegistry[icon]` becomes `undefined` here).
            <span title="Inspector">
              <MagnifierIcon size={18} className="text-muted" aria-hidden />
            </span>
          ) : (
            <p className="font-mono text-[9px] uppercase tracking-[0.3em] text-muted">
              Inspector
            </p>
          )}
        </header>
        {!rightCollapsed && (
          // 2026-07-14 fix: the aside's `overflow-auto` was clipping
          // the absolutely-positioned <FoldToggleButton side="right">
          // at `-left-3`, hiding its 12px edge overlay (per Spencer's
          // "icon is contained within the sidebar" feedback). Mirrors
          // the left rail's pattern: keep overflow on the INNER
          // content wrapper, not the aside itself, so the fold
          // button's overflow-relative position is preserved.
          <div className="flex-1 overflow-y-auto">
            <Separator />
            <section className="p-4">
              <h3 className="mb-3 flex items-center gap-2 font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
                <LockIcon size={10} className="text-accent" aria-hidden />
                Vault
              </h3>
              {vaultError ? (
                <p
                  className="text-sm text-danger"
                  title={vaultError}
                  data-testid="vault-error"
                >
                  Failed to load vault: {vaultError.slice(0, 80)}
                </p>
              ) : profiles.length === 0 ? (
                <p className="flex items-center gap-2 text-sm text-muted">
                  <InfoCircleIcon size={12} aria-hidden />
                  No profiles configured.
                </p>
              ) : (
                <ul className="flex flex-col gap-2">
                  {profiles.map((p) => (
                    <li
                      key={p.name}
                      className="flex items-center gap-2 rounded-md border border-default/30 bg-surface/30 px-2.5 py-1.5 text-sm transition-colors hover:border-accent/40"
                    >
                      <span
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent shadow-[0_0_4px_var(--accent)]"
                        aria-hidden
                      />
                      <span className="font-medium">{p.provider}</span>
                      <span className="text-muted">· {p.method}</span>
                    </li>
                  ))}
                </ul>
              )}
            </section>
            <Separator />
            <section className="p-4">
              <h3 className="mb-3 flex items-center gap-2 font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
                <HistoryCircleIcon
                  size={10}
                  className="text-accent"
                  aria-hidden
                />
                Activity
              </h3>
              <p className="flex items-center gap-2 text-sm text-muted">
                <InfoCircleIcon size={12} aria-hidden />
                No recent activity.
              </p>
            </section>
          </div>
        )}
        {rightCollapsed && (
          <div className="flex flex-1 flex-col items-center justify-end gap-3 p-3">
            <span
              className="h-2 w-2 rounded-full bg-success shadow-[0_0_6px_var(--success)]"
              title="System online"
              aria-label="System online"
            />
          </div>
        )}

        {/* Fold toggle — middle of the left edge. Shared
            <FoldToggleButton side="right" /> mirrors the arrow via
            CSS scaleX(-1) so the visual matches the left rail. */}
        <FoldToggleButton
          side="right"
          isCollapsed={rightCollapsed}
          onToggle={() => setRightCollapsed((c) => !c)}
          label="inspector"
        />
      </aside>
    </div>
  );
}
