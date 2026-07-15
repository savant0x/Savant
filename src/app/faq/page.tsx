"use client";

// FID-028 — FAQ page wired to the curated Q&A data.
//
// Source: `src/lib/faq-data.ts` (8 Q&A items grounded in the
// project's own CHANGELOG, README, LEARNINGS, the 22-crate Rust
// workspace, and the FID lifecycle). No real FAQ module exists in
// the savant-orig (verified via find/grep — see
// `src/lib/faq-data.ts` §Future FID candidate comment for the
// follow-on work item).
//
// In browser preview, the IPC wrapper returns the hardcoded
// `FAQ_ITEMS` array. The Tauri runtime wire-up is a follow-on
// FID — not in this FID's scope per LESSON-038.

import { useEffect, useState } from "react";
import { Card } from "@heroui/react";
import { DashboardShell } from "@/components/dashboard-shell";
import { getFaq, type FaqItem } from "@/lib/ipc";

export default function FaqPage() {
  const [items, setItems] = useState<FaqItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await getFaq();
        if (!cancelled) setItems(list);
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <DashboardShell>
      <Card className="p-6">
        <p className="mb-3 font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
          FAQ · 8 questions · source: project artifacts (CHANGELOG, README, LEARNINGS)
        </p>
        {error && (
          <p
            className="mb-3 font-mono text-[10px] uppercase tracking-[0.2em] text-danger"
            role="status"
          >
            {error}
          </p>
        )}
        {items === null && !error && (
          <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
            Loading FAQ…
          </p>
        )}
        {items !== null && (
          <div className="flex flex-col gap-2">
            {items.map((item, idx) => (
              <details
                key={idx}
                className="group rounded-md border border-default/30 open:border-accent/50"
              >
                <summary className="cursor-pointer list-none px-4 py-3 font-mono text-[11px] font-semibold uppercase tracking-[0.15em] text-foreground transition-colors hover:text-accent">
                  <span className="mr-2 text-muted group-open:text-accent">
                    ▸
                  </span>
                  {item.question}
                </summary>
                <p className="px-4 pb-3 text-sm text-foreground">{item.answer}</p>
              </details>
            ))}
          </div>
        )}
      </Card>
    </DashboardShell>
  );
}
