"use client";

// FID-028 — Shared <MarkdownRenderer> component.
//
// ECHO Law 13: "utility-first, no duplicate logic across consumers."
// The `a` external-link handler + the `prose prose-invert max-w-none
// text-sm text-foreground` class set + the `remarkGfm` plugin were
// duplicated across the reflections page (FID-017) and the changelog
// page (FID-028). This component is the single source of truth — any
// future markdown consumer (manifest page body, settings help text,
// FAQ accordion bodies, etc.) consumes the same component for free.
//
// Behavior contract:
// - External links (http/https) open in a new tab with rel="noopener
//   noreferrer" (security best practice — prevents reverse-tabnabbing
//   and referrer leakage).
// - Internal anchors (href starting with #) get default browser
//   behavior (no target="_blank", no rel).
// - GFM (GitHub Flavored Markdown) is enabled by default: tables, task
//   lists, strikethrough, autolinks, fenced code blocks, hard line
//   breaks, escape sequences — all rendered per the GFM spec.
// - Markdown source is XSS-safe by construction (react-markdown uses
//   ref-based AST traversal → React elements; React's native escaping
//   in string children — no `dangerouslySetInnerHTML`).

import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * The default `a` (anchor) component override. Opens external
 * `http(s)` links in a new tab with `rel="noopener noreferrer"`
 * (security best practice — prevents reverse-tabnabbing + referrer
 * leakage); internal anchors (href starts with `#`) get default
 * browser behavior. Exported as a named const so callers can
 * compose on top of it (e.g., for callers that want to add extra
 * analytics tracking on external link clicks).
 */
export const defaultMarkdownComponents: Components = {
  a: ({ node, ...props }) => {
    const href = String(props.href ?? "");
    const isExternal = /^https?:\/\//i.test(href);
    if (isExternal) {
      return <a {...props} target="_blank" rel="noopener noreferrer" />;
    }
    return <a {...props} />;
  },
};

export type MarkdownRendererProps = {
  /** The markdown source string. */
  content: string;
  /**
   * Optional override for the prose wrapper className. Defaults to
   * the reflections + changelog canonical set:
   * `"prose prose-invert max-w-none text-sm text-foreground"`.
   * Pass an empty string to render without any wrapper (e.g., for
   * inline markdown where the parent's typography already applies).
   */
  className?: string;
  /**
   * Optional `react-markdown` component overrides. MERGED with
   * `defaultMarkdownComponents` (caller's keys win, but the
   * default `a` external-link handler is preserved unless the
   * caller explicitly overrides it). This is the safe-default
   * pattern: callers can add custom `code` / `h1` / `pre`
   * renderers without forking the component, but cannot
   * accidentally drop the external-link security behavior.
   */
  components?: Components;
};

/**
 * Renders markdown with the Savant dashboard's standard styling +
 * external-link behavior. Used by /reflections (FID-017) +
 * /changelog (FID-028). Future markdown consumers SHOULD use this
 * component rather than calling `<ReactMarkdown>` directly.
 */
export function MarkdownRenderer({
  content,
  className = "prose prose-invert max-w-none text-sm text-foreground",
  components,
}: MarkdownRendererProps) {
  return (
    <div className={className}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{ ...defaultMarkdownComponents, ...components }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
