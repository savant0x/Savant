// FID-028 — LlmParams parameter_descriptors (mirror of
// `savant_core::types::LlmParams::get_parameter_descriptors()` in the
// Rust core). The gateway's `/api/models` endpoint returns the same
// shape at runtime (see `crates/gateway/src/server.rs:1500+`).
//
// This file is the **browser-preview** source of truth. The Tauri
// runtime can either (a) call the gateway's `/api/models` directly,
// or (b) use this same hardcoded list. The hardcoded list is the
// authoritative source of truth for the parameter NAMES + RANGES
// (the gateway's `settings_post_handler` at
// `crates/gateway/src/server.rs:1110-1180` clamps to these same
// ranges — see `clamp()` calls).
//
// All defaults mirror `savant_core::config::AiConfig::default()`.
// Fields match the gateway's `SettingsUpdate` struct at
// `crates/gateway/src/server.rs:1064-1081`.

export type ParameterDescriptor = {
  /** Snake-case field name (matches the gateway's SettingsUpdate keys). */
  name: string;
  /** UI-facing type label (f32 / string / optional string). */
  type: "f32" | "string" | "option<string>";
  /** Human-readable description (shown in the Tune page form). */
  description: string;
  /** Inclusive min for numeric params. */
  min?: number;
  /** Inclusive max for numeric params. */
  max?: number;
  /** Default value (from AiConfig::default()). */
  default?: number | string | null;
};

export const PARAMETER_DESCRIPTORS: ParameterDescriptor[] = [
  {
    name: "temperature",
    type: "f32",
    description:
      "Controls randomness in token sampling. 0.0 = deterministic (always pick the most likely token); higher = more random/creative. 0.78 is the SOUL builder default.",
    min: 0.0,
    max: 2.0,
    default: 0.78,
  },
  {
    name: "top_p",
    type: "f32",
    description:
      "Nucleus sampling — only consider the top tokens whose cumulative probability mass is ≤ top_p. Lower = more focused; 1.0 = all tokens considered. Works with Temperature to fine-tune creativity.",
    min: 0.0,
    max: 1.0,
    default: 1.0,
  },
  {
    name: "frequency_penalty",
    type: "f32",
    description:
      "Penalizes tokens that have already appeared, proportional to their frequency. Positive = less repetition; negative = more repetition. -2.0 to 2.0.",
    min: -2.0,
    max: 2.0,
    default: 0.0,
  },
  {
    name: "presence_penalty",
    type: "f32",
    description:
      "Penalizes tokens that have appeared at all (flat penalty per occurrence). Positive = more likely to introduce new topics; negative = more likely to stay on-topic. -2.0 to 2.0.",
    min: -2.0,
    max: 2.0,
    default: 0.0,
  },
  {
    name: "chat_model",
    type: "string",
    description:
      "The default OpenRouter model id for chat. User-overridable on the Settings page; the Tune page shows the system default. Empty = use the model picked in Settings.",
    default: "meta-llama/llama-3.3-70b-instruct:free",
  },
  {
    name: "manifestation_model",
    type: "option<string>",
    description:
      "Optional override for the SOUL builder model. If set, the manifest page uses this model instead of the chat model. Empty = inherit chat model.",
    default: null,
  },
  {
    name: "vision_model",
    type: "string",
    description:
      "The default vision model for browser-side image understanding. Separate from the chat model because vision models have different capability + cost profiles.",
    default: "llama-3.2-90b-vision-instruct",
  },
  {
    name: "provider",
    type: "string",
    description:
      "The active LLM provider. One of: openrouter, ollama, anthropic, openai. Switches the inference backend. The Settings page validates this against `savant_core::types::ModelProvider`.",
    default: "openrouter",
  },
  {
    name: "ollama_url",
    type: "string",
    description:
      "Base URL for the Ollama provider (when provider = 'ollama'). Defaults to http://localhost:11434. Setting this auto-switches the provider to 'ollama'.",
    default: "http://localhost:11434",
  },
];

/**
 * Returns the parameter descriptors. Identical to the gateway's
 * `/api/models` response body's `parameter_descriptors` field.
 */
export function getParameterDescriptors(): ParameterDescriptor[] {
  return PARAMETER_DESCRIPTORS;
}

// ─────────────────────────────────────────────────────────────────
// Spencer revision 2026-07-14 — Tune page is for the actual model
// tuning (sampling knobs), NOT model selection. The 5 model-
// selection fields (chat_model, manifestation_model, vision_model,
// provider, ollama_url) live on the Settings page instead.
//
// The savant-orig `savant_core::types::LlmParams` struct contains
// both categories; the gateway returns all 9 via
// `get_parameter_descriptors`. The renderer filters to the 4
// sampling knobs for the Tune page. SINGLE SOURCE OF TRUTH: the
// full list stays in PARAMETER_DESCRIPTORS; TUNING_DESCRIPTORS is
// a derived view (no schema duplication, obeys ECHO Law 13).
// ─────────────────────────────────────────────────────────────────

/** TUNING_FIELDS — the 4 sampling-knob names. */
const TUNING_FIELDS: ReadonlySet<string> = new Set([
  "temperature",
  "top_p",
  "frequency_penalty",
  "presence_penalty",
]);

/** TUNING_DESCRIPTORS — the 4 TRUE tuning parameter descriptors,
 *  filtered from PARAMETER_DESCRIPTORS. The Tune page uses this;
 *  model-selection fields are configured on the Settings page. */
export const TUNING_DESCRIPTORS: ParameterDescriptor[] =
  PARAMETER_DESCRIPTORS.filter((d) => TUNING_FIELDS.has(d.name));

/**
 * Returns the 4 TRUE tuning parameter descriptors. The Tune page
 * calls this via the `getTuningDescriptors` IPC wrapper. The
 * full `get_parameter_descriptors` IPC command still returns all
 * 9 (the gateway contract) for the future Settings wiring.
 */
export function getTuningDescriptors(): ParameterDescriptor[] {
  return TUNING_DESCRIPTORS;
}
