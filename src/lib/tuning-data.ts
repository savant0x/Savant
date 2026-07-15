// FID-028 Revision 2 (Spencer 2026-07-14) — Renderer-side metadata
// for the Tune page. Lives in a separate file from
// `parameter-descriptors.ts` because the gateway's
// `LlmParams::get_parameter_descriptors()` contract doesn't include
// examples or preset profiles — these are UX enrichment, not part of
// the IPC schema. Single source of truth for the param NAMES + RANGES
// stays in `parameter-descriptors.ts`; this file is pure data
// adjacent.
//
// ECHO Law 13 (utility-first, no duplicate logic): the example data
// is keyed by the param `name` (the gateway's snake_case identifier)
// so future rendering changes to the descriptors don't drift from
// the examples.

/**
 * TUNING_PARAM_LABELS — Human-readable names for the 4 sampling
 * knobs. The gateway uses snake_case (`temperature`, `top_p`,
 * `frequency_penalty`, `presence_penalty`); the UI uses Title Case
 * for display. Future additions (e.g., a `repetition_penalty` knob
 * from a new gateway version) just need an entry here.
 */
export const TUNING_PARAM_LABELS: Readonly<Record<string, string>> = {
  temperature: "Temperature",
  top_p: "Top-P (Nucleus Sampling)",
  frequency_penalty: "Frequency Penalty",
  presence_penalty: "Presence Penalty",
};

/**
 * TUNING_PARAM_DESCRIPTIONS — Short "what it does" blurbs for the
 * header section + per-param cards. Mirrors the long-form
 * `descriptor.description` field but condensed for inline use.
 */
export const TUNING_PARAM_DESCRIPTIONS: Readonly<Record<string, string>> =
  {
    temperature:
      "Controls randomness in token sampling. 0.0 = deterministic; higher = more random/creative.",
    top_p:
      "Nucleus sampling — only consider the top tokens whose cumulative probability is ≤ top_p. Lower = more focused; 1.0 = all tokens.",
    frequency_penalty:
      "Penalizes repeated tokens proportional to how often they've appeared. Positive = less repetition; negative = more.",
    presence_penalty:
      "Penalizes any token that has appeared (flat per-occurrence). Positive = more new topics; negative = more on-topic.",
  };

/**
 * TUNING_EXAMPLES — Per-param example use cases. Each entry pairs a
 * concrete value with a label + a one-line description of what that
 * value achieves. Renders under each param card so the user can see
 * "what 0.3 actually does" without reading the docs.
 */
export type TuningExample = {
  value: number;
  label: string;
  description: string;
};

export const TUNING_EXAMPLES: Readonly<
  Record<string, ReadonlyArray<TuningExample>>
> = {
  temperature: [
    {
      value: 0.0,
      label: "Deterministic",
      description:
        "Always pick the most likely token. Code completion, math, structured output.",
    },
    {
      value: 0.3,
      label: "Focused",
      description:
        "Slight variation, mostly predictable. Factual Q&A, documentation.",
    },
    {
      value: 0.78,
      label: "Balanced (default)",
      description:
        "Creative but coherent. General conversation, content creation.",
    },
    {
      value: 1.2,
      label: "Creative",
      description:
        "High variation, surprising choices. Brainstorming, poetry.",
    },
  ],
  top_p: [
    {
      value: 0.5,
      label: "Very focused",
      description:
        "Only top tokens considered. Use with low temperature for deterministic output.",
    },
    {
      value: 0.9,
      label: "Balanced",
      description:
        "Most likely tokens + some variety. Use with mid temperature.",
    },
    {
      value: 1.0,
      label: "All tokens (default)",
      description:
        "All tokens weighted by temperature. Use with high temperature for max creativity.",
    },
  ],
  frequency_penalty: [
    {
      value: 0.0,
      label: "No penalty (default)",
      description: "Words can repeat freely.",
    },
    {
      value: 0.5,
      label: "Light penalty",
      description:
        "Discourages obvious repetition. Use for long-form content.",
    },
    {
      value: 1.5,
      label: "Strong penalty",
      description:
        "Actively avoids repeated words. Use for diverse vocabulary.",
    },
  ],
  presence_penalty: [
    {
      value: 0.0,
      label: "No penalty (default)",
      description: "Model can stay on topic indefinitely.",
    },
    {
      value: 0.5,
      label: "Light penalty",
      description:
        "Encourages new topics. Use for brainstorming sessions.",
    },
    {
      value: 1.5,
      label: "Strong penalty",
      description:
        "Forces topic shifts. Use for exploration, ideation.",
    },
  ],
};

/**
 * TUNING_PRESETS — 4 click-to-apply preset profiles for the
 * header section. Each preset sets all 4 values to a coherent
 * combination for a specific use case. Clicking a preset card
 * overwrites the current `values` state (the user can then
 * fine-tune individual knobs before clicking Apply).
 */
export type TuningPreset = {
  name: string;
  description: string;
  values: Readonly<Record<string, number>>;
};

export const TUNING_PRESETS: ReadonlyArray<TuningPreset> = [
  {
    name: "Code completion",
    description: "Deterministic, focused. Code, math, structured output.",
    values: {
      temperature: 0.2,
      top_p: 0.9,
      frequency_penalty: 0.0,
      presence_penalty: 0.0,
    },
  },
  {
    name: "Creative writing",
    description: "High variation, rich vocabulary. Stories, poems, content.",
    values: {
      temperature: 0.9,
      top_p: 0.95,
      frequency_penalty: 0.3,
      presence_penalty: 0.0,
    },
  },
  {
    name: "Factual Q&A",
    description: "Low variation, stays on topic. Documentation, factual answers.",
    values: {
      temperature: 0.3,
      top_p: 0.85,
      frequency_penalty: 0.0,
      presence_penalty: 0.2,
    },
  },
  {
    name: "Brainstorming",
    description: "Wild, explores many topics. Ideation, exploration.",
    values: {
      temperature: 1.2,
      top_p: 0.98,
      frequency_penalty: 0.0,
      presence_penalty: 0.4,
    },
  },
];

/**
 * Local-storage key for the browser-preview's saved tuning values.
 * Used to persist the user's last-saved values across page reloads
 * in the absence of a real `loadSettings` IPC command. The Tauri
 * runtime would route through the gateway's `GET /api/settings` /
 * `POST /api/settings` endpoints instead.
 */
export const LS_TUNE_SETTINGS = "savant.tune.settings";
