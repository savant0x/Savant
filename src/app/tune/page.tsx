"use client";

// FID-028 (Spencer revision 5, 2026-07-14) — Tune page redesign.
// Addresses 3 user-flagged issues from the latest feedback:
//
// 1. "The slider is hard to see, it looks like the slider is floating
//    on the page, fix the design." → Replaced the native
//    `<input type="range">` (which had a too-subtle `bg-default/30`
//    track + browser-default thumb) with the HeroUI v3 `<Slider>`
//    compound component. The track is now `h-3 bg-default/60` (darker,
//    clearly visible) and the thumb is `h-5 w-5 border-2 border-accent
//    bg-background shadow-md` (prominent, clearly draggable). The
//    slider is no longer "floating" — it has a visible track + a
//    prominent thumb with a shadow.
//
// 2. "Make each div have a header+ body, enhanced with hero ui
//    features." → Replaced plain `<div>` wrappers with the HeroUI v3
//    `<Card>` compound components: `CardHeader` + `CardTitle` +
//    `CardDescription` + `CardContent` + `CardFooter`. Each section
//    (header explainer, 4 param cards, footer actions) now has a
//    clear `header` (title + description) + `body` (interactive
//    content) + `footer` (secondary info) structure with
//    `border-b` / `border-t` separators. This is the canonical
//    HeroUI v3 Card pattern (compound components replace v2's
//    component hooks per the LLMS research).
//
// 3. "On each slider you have examples but there are no 'place
//    holders' where the user can quickly slide to the value on the
//    bottom, it would be better if it added those to the slider with
//    a key." → Added a "Quick set" row of clickable chips BELOW
//    each slider. Each chip shows the example value + label (e.g.,
//    "0.78 · Balanced (default)"). Clicking a chip sets the slider
//    to that value. The active chip (matching the current value
//    within ±0.01 tolerance) is highlighted with the accent color.
//    This is the "place holders" the user requested — the user can
//    see all the recommended values at a glance + click to jump
//    rather than dragging the slider to guess where the value sits.
//
// HeroUI v3 alpha (3.0.0-beta.2) is installed. Using the compound
// `<Card>` + `<Slider>` sub-components for the first time — previous
// revisions stuck to `<Card>` + native inputs due to alpha risk;
// the basher's research confirmed the Slider compound + CardHeader
// / CardTitle / CardDescription / CardContent / CardFooter are all
// available in the installed package.
//
// Per Spencer 2026-07-14 (original): "Fine tuning is for the actual
// model it's self, this page is not to change models." The 4 knobs
// are still the TRUE tuning parameters (sampling knobs), not model
// selection.
//
// Data: `src/lib/tuning-data.ts` holds the renderer-side metadata
// (TUNING_PARAM_LABELS, TUNING_EXAMPLES, TUNING_PRESETS,
// LS_TUNE_SETTINGS) — separate from `parameter-descriptors.ts`
// because the gateway IPC contract doesn't include examples or
// presets (UX enrichment, not part of the IPC schema). ECHO
// Law 13: example data keyed by the param `name` (gateway's
// snake_case identifier) so future descriptor changes don't drift
// from the examples.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
  Slider,
  SliderFill,
  SliderThumb,
  SliderTrack,
} from "@heroui/react";
import { DashboardShell } from "@/components/dashboard-shell";
import {
  getTuningDescriptors,
  saveSettings,
  type ParameterDescriptor,
  type SaveSettingsInput,
} from "@/lib/ipc";
import {
  LS_TUNE_SETTINGS,
  TUNING_EXAMPLES,
  TUNING_PARAM_LABELS,
  TUNING_PRESETS,
  type TuningExample,
  type TuningPreset,
} from "@/lib/tuning-data";
import { logger } from "@/lib/logger";

export default function TunePage() {
  const [descriptors, setDescriptors] = useState<ParameterDescriptor[] | null>(
    null,
  );
  const [values, setValues] = useState<SaveSettingsInput>({});
  const [savedValues, setSavedValues] = useState<SaveSettingsInput>({});
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  // Load descriptors + initialize values from localStorage (or
  // descriptor defaults if no localStorage entry exists). This
  // closes the gap where the page would always revert to defaults
  // on a fresh mount (no `loadSettings` IPC command yet — the
  // localStorage fallback is browser-preview only).
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await getTuningDescriptors();
        if (cancelled) return;
        setDescriptors(list);

        // Build initial values from localStorage if present,
        // otherwise from descriptor defaults. Validates each
        // localStorage entry is a finite number AND clamps to
        // the descriptor's range so a stale entry can't push
        // the slider out of bounds.
        const initial: Record<string, number> = {};
        if (typeof window !== "undefined") {
          try {
            const raw = window.localStorage.getItem(LS_TUNE_SETTINGS);
            if (raw) {
              const parsed = JSON.parse(raw) as Record<string, unknown>;
              for (const d of list) {
                if (typeof d.default !== "number") continue;
                const v = parsed[d.name];
                if (typeof v === "number" && Number.isFinite(v)) {
                  const lo = d.min ?? -Infinity;
                  const hi = d.max ?? Infinity;
                  initial[d.name] = Math.min(Math.max(v, lo), hi);
                } else {
                  initial[d.name] = d.default;
                }
              }
            }
          } catch {
            /* malformed localStorage — fall through to defaults */
          }
        }
        if (Object.keys(initial).length === 0) {
          for (const d of list) {
            if (typeof d.default === "number") {
              initial[d.name] = d.default;
            }
          }
        }
        setValues(initial);
        // savedValues mirrors the initial state so the Apply
        // button is disabled until the user makes a change.
        setSavedValues(initial);
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          logger.warn("getTuningDescriptors failed", {}, e);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // isDirty: any param value differs from the last-saved state.
  // Used to disable the Apply button when nothing has changed
  // (prevents redundant IPC roundtrips).
  const isDirty = useMemo(() => {
    if (Object.keys(values).length === 0) return false;
    return Object.keys(values).some((k) => values[k] !== savedValues[k]);
  }, [values, savedValues]);

  const handleApply = useCallback(async (): Promise<void> => {
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      await saveSettings(values);
      // Persist to localStorage so the values survive page
      // reloads. The Tauri runtime would route through the
      // gateway instead — the localStorage write is the
      // browser-preview equivalent.
      if (typeof window !== "undefined") {
        try {
          window.localStorage.setItem(
            LS_TUNE_SETTINGS,
            JSON.stringify(values),
          );
        } catch {
          /* noop — quota / private-mode fail doesn't fail the apply */
        }
      }
      setSavedValues({ ...values });
      setSaved(true);
      // Fade the "Saved" indicator after 3s. Clear any previous
      // timeout first so a quick double-Apply doesn't leave a
      // stale timer. Tracked in a ref so the unmount cleanup
      // (see useEffect below) can cancel it if the user
      // navigates away within 3s.
      if (savedTimeoutRef.current !== null) {
        window.clearTimeout(savedTimeoutRef.current);
      }
      savedTimeoutRef.current = window.setTimeout(() => {
        setSaved(false);
        savedTimeoutRef.current = null;
      }, 3000);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      logger.warn("saveSettings failed", {}, e);
    } finally {
      setSaving(false);
    }
  }, [values]);

  // Reset to descriptor defaults (per the thinker's Q9 validation:
  // Reset to descriptor defaults, NOT to last-saved values; the
  // page reverts to the last-saved values naturally on refresh).
  const handleReset = useCallback((): void => {
    if (descriptors === null) return;
    const reset: Record<string, number> = {};
    for (const d of descriptors) {
      if (typeof d.default === "number") {
        reset[d.name] = d.default;
      }
    }
    setValues(reset);
  }, [descriptors]);

  const applyPreset = useCallback((preset: TuningPreset): void => {
    setValues({ ...preset.values });
  }, []);

  const updateValue = useCallback((name: string, value: number): void => {
    setValues((prev) => ({ ...prev, [name]: value }));
  }, []);

  // Cleanup the "Saved" indicator timeout on unmount so a
  // navigation-away within 3s doesn't leak a setState call
  // (React 18 just warns, but the pattern is worth establishing).
  const savedTimeoutRef = useRef<number | null>(null);
  useEffect(() => {
    return () => {
      if (savedTimeoutRef.current !== null) {
        window.clearTimeout(savedTimeoutRef.current);
        savedTimeoutRef.current = null;
      }
    };
  }, []);

  return (
    <DashboardShell>
      <div className="flex flex-col gap-6">
        {/* ── Header Card: structured header + body + preset section ── */}
        <Card className="overflow-hidden">
          <CardHeader className="border-b border-default/30 px-6 py-5">
            <p className="mb-1 font-mono text-[10px] font-semibold uppercase tracking-[0.3em] text-muted">
              Fine-Tuning
            </p>
            <CardTitle className="font-mono text-2xl font-semibold uppercase tracking-[0.2em] text-foreground">
              Shape how the model thinks
            </CardTitle>
          </CardHeader>
          <CardContent className="px-6 py-5">
            <p className="mb-4 text-sm text-foreground">
              <strong>Fine-tuning (in the LLM context)</strong> means
              adjusting the sampling parameters that control how the
              model picks the next token at <strong>inference time</strong>.
              These are <strong>NOT</strong> training the model —
              they&apos;re knobs that change the model&apos;s behavior at
              runtime. Every response Savant generates uses these 4
              values.
            </p>
            <p className="text-sm text-foreground">
              The right combination can mean the difference between a
              focused, deterministic code completion and a wildly
              creative brainstorming partner. Each knob below changes
              one dimension of the model&apos;s behavior. Click a preset
              profile to start, then fine-tune the individual knobs.
            </p>
          </CardContent>
          <div className="border-t border-default/30 bg-surface/20 px-6 py-5">
            <p className="mb-3 font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
              Preset profiles (click to apply)
            </p>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
              {TUNING_PRESETS.map((preset) => (
                <button
                  key={preset.name}
                  type="button"
                  onClick={() => applyPreset(preset)}
                  disabled={descriptors === null}
                  className="rounded-md border border-default/30 bg-surface/20 p-3 text-left transition-colors hover:border-accent/40 hover:bg-accent/5 disabled:cursor-not-allowed disabled:opacity-40"
                >
                  <h4 className="mb-1 font-mono text-[11px] font-semibold uppercase tracking-[0.15em] text-foreground">
                    {preset.name}
                  </h4>
                  <p className="mb-2 text-xs text-muted">
                    {preset.description}
                  </p>
                  <div className="font-mono text-[9px] uppercase tracking-[0.2em] text-accent">
                    t = {preset.values.temperature} · p = {preset.values.top_p} · f = {preset.values.frequency_penalty} · pr = {preset.values.presence_penalty}
                  </div>
                </button>
              ))}
            </div>
          </div>
        </Card>

        {/* ── Loading / error states ── */}
        {descriptors === null && !error && (
          <Card className="p-6">
            <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
              Loading parameter descriptors…
            </p>
          </Card>
        )}
        {error && (
          <Card className="border-danger/40 bg-danger/5 p-6">
            <p
              className="font-mono text-[10px] uppercase tracking-[0.2em] text-danger"
              role="alert"
            >
              {error}
            </p>
          </Card>
        )}

        {/* ── 4 parameter cards: structured header + body + footer ── */}
        {descriptors !== null && (
          <div className="flex flex-col gap-5">
            {descriptors.map((d) => (
              <ParameterField
                key={d.name}
                descriptor={d}
                value={values[d.name] ?? 0}
                onChange={(v) => updateValue(d.name, v)}
                examples={TUNING_EXAMPLES[d.name] ?? []}
              />
            ))}
          </div>
        )}

        {/* ── Footer Card: Reset + Apply + Saved indicator ── */}
        {descriptors !== null && (
          <Card className="overflow-hidden">
            <CardContent className="flex flex-wrap items-center justify-between gap-3 p-5">
              <button
                type="button"
                onClick={handleReset}
                disabled={saving}
                className="flex items-center gap-2 rounded-sm border border-default/60 px-3 py-1.5 font-mono text-[10px] uppercase tracking-[0.2em] text-muted transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
              >
                <i className="fas fa-rotate-left" aria-hidden /> Reset to
                defaults
              </button>
              <div className="flex flex-wrap items-center gap-3">
                {saved && (
                  <span
                    aria-live="polite"
                    className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.2em] text-success"
                  >
                    <span
                      className="h-1.5 w-1.5 rounded-full bg-success shadow-[0_0_4px_var(--success)]"
                      aria-hidden
                    />
                    Saved
                  </span>
                )}
                {error && (
                  <span
                    className="font-mono text-[10px] uppercase tracking-[0.2em] text-danger"
                    role="status"
                  >
                    {error.slice(0, 80)}
                  </span>
                )}
                <button
                  type="button"
                  onClick={() => void handleApply()}
                  disabled={saving || !isDirty}
                  className="flex items-center gap-2 rounded-md border border-accent bg-accent/15 px-4 py-1.5 font-mono text-[10px] uppercase tracking-[0.2em] text-accent transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:opacity-40"
                >
                  {saving ? (
                    <>
                      <i className="fas fa-spinner fa-spin" aria-hidden />{" "}
                      Applying…
                    </>
                  ) : (
                    <>
                      <i className="fas fa-floppy-disk" aria-hidden />{" "}
                      Apply changes
                    </>
                  )}
                </button>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </DashboardShell>
  );
}

function ParameterField({
  descriptor,
  value,
  onChange,
  examples,
}: {
  descriptor: ParameterDescriptor;
  value: number;
  onChange: (value: number) => void;
  examples: ReadonlyArray<TuningExample>;
}) {
  const { name, description, min, max, default: defaultValue } = descriptor;
  const paramLabel = TUNING_PARAM_LABELS[name] ?? name;

  return (
    <Card className="overflow-hidden">
      {/* Header — title + description (border-b separator) */}
      <CardHeader className="border-b border-default/30 px-5 py-4">
        <CardTitle className="font-mono text-sm font-semibold uppercase tracking-[0.18em] text-foreground">
          {paramLabel}
        </CardTitle>
        <CardDescription className="mt-1 text-xs text-muted">
          {description}
        </CardDescription>
      </CardHeader>

      {/* Body — HeroUI v3 Slider + number + min/default/max + quick-set chips */}
      <CardContent className="px-5 py-5">
        <div className="flex items-center gap-4">
          <Slider
            value={value}
            onChange={(v) => onChange(Array.isArray(v) ? v[0] : v)}
            minValue={min ?? 0}
            maxValue={max ?? 1}
            step={0.01}
            className="flex-1"
            aria-label={`${paramLabel} value`}
          >
            <SliderTrack className="h-3 bg-default/60">
              <SliderFill className="bg-accent" />
              <SliderThumb className="h-5 w-5 border-2 border-accent bg-background shadow-md" />
            </SliderTrack>
          </Slider>
          <span
            aria-hidden
            className="w-16 rounded border border-[color:var(--input-border-color)] bg-surface/30 px-2 py-1 text-center font-mono text-sm tabular-nums text-foreground"
          >
            {value.toFixed(2)}
          </span>
        </div>
        <div className="mt-2 flex items-center justify-between font-mono text-[9px] uppercase tracking-[0.2em] text-muted">
          <span>min: {min}</span>
          <span>default: {defaultValue}</span>
          <span>max: {max}</span>
        </div>

        {/* Quick-set chips — clickable shortcuts to example values.
            This is the "place holders" feature Spencer requested:
            the user can see all the recommended values at a glance
            + click to jump rather than dragging the slider to guess
            where the value sits. The active chip (matching the
            current value within ±0.01 tolerance) is highlighted
            with the accent color. */}
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
            Quick set:
          </span>
          {examples.map((ex) => {
            const isActive = Math.abs(value - ex.value) < 0.01;
            return (
              <button
                key={ex.value}
                type="button"
                onClick={() => onChange(ex.value)}
                className={[
                  "rounded-md border px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.15em] transition-colors",
                  isActive
                    ? "border-accent bg-accent/15 text-accent"
                    : "border-default/40 bg-surface/30 text-muted hover:border-accent/40 hover:text-accent/80",
                ].join(" ")}
                title={ex.description}
              >
                {ex.value.toFixed(2)} · {ex.label}
              </button>
            );
          })}
        </div>
      </CardContent>

      {/* Footer — Example use cases (border-t separator, lighter bg) */}
      <CardFooter className="flex flex-col items-stretch gap-2 border-t border-default/20 bg-surface/10 px-5 py-4">
        <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
          Example use cases
        </p>
        <ul className="flex flex-col gap-2">
          {examples.map((ex) => (
            <li
              key={`${name}-${ex.value}`}
              className="flex gap-3 text-xs"
            >
              <span className="w-14 shrink-0 font-mono font-semibold tabular-nums text-accent">
                {ex.value.toFixed(2)}
              </span>
              <span className="flex-1 text-foreground">
                <strong className="font-semibold">{ex.label}</strong>{" "}
                — {ex.description}
              </span>
            </li>
          ))}
        </ul>
      </CardFooter>
    </Card>
  );
}
