"use client";

// Phase 1 IPC wrappers — minimal type annotations that match the Rust types
// in src-tauri/src/security/master_key.rs (ProfileSummary) and the cmd return
// signatures in src-tauri/src/lib.rs. tauri-specta v2 in Phase 2 will replace
// these hand-written types with auto-generated bindings — see FID-001 "Q-Next"
// section for that work item.
//
// Browser-preview mode: `setupMockIPC()` installs @tauri-apps/api/mocks so the
// renderer works in any browser without a Tauri host. The mock is a no-op
// inside the Tauri webview (window.__TAURI_INTERNALS__ is set there).

import { invoke } from "@tauri-apps/api/core";
import { setupMockIPC } from "./mock-ipc";
import { fetchChangelog } from "./changelog";
import type { ManifestStreamEvent } from "./manifest-mock";
import type {
  AgentManifestPlan,
  BulkManifestPayload,
  BulkManifestResult,
  ManifestResult,
  SoulManifestPayload,
} from "@/types/control-frames";
import { logger } from "@/lib/logger";

setupMockIPC();

export type ProfileSummary = {
  name: string;
  provider: string;
  method: string;
  secret_ref_kind: string;
  base_url: string | null;
  updated_at: number;
};

/** Persist a provider profile (e.g. "openrouter") to the OS app-data vault. */
export async function saveMasterKey(
  provider: string,
  apiKey: string,
): Promise<void> {
  return invoke("setup_master_key", { provider, apiKey });
}

/** Single-shot chat completion against OpenRouter's default profile. */
export async function inferOpenrouter(prompt: string): Promise<string> {
  return invoke("infer_openrouter", { prompt });
}

/** List all profiles currently in the vault (api keys not returned). */
export async function listProfiles(): Promise<ProfileSummary[]> {
  return invoke("vault_list_profiles");
}

/** Redacted master-key summary. NEVER includes the actual key bytes. */
export type MasterKeyInfo = {
  exists: boolean;
  last4?: string;
  savedAt?: number | null;
  /**
   * FID-008 — which tier is the active source:
   * - `"env"`   = `process.env.OPENROUTER_MASTER_KEY` (tier 1)
   * - `"vault"` = `setup_master_key` save (tier 2)
   * - `"none"`  = no key available; manifest falls back to template
   */
  source: "env" | "vault" | "none";
};

/**
 * FID-007 — Query the saved master key for a provider. Returns
 * redacted metadata only (existence + last-4 + savedAt timestamp) so
 * the Settings page can render a masked key chip without ever
 * holding the raw key bytes in React state. Returns `{ exists: false }`
 * when no key is saved for the provider.
 */
export async function getMasterKeyInfo(
  provider: string,
): Promise<MasterKeyInfo> {
  return invoke<MasterKeyInfo>("get_master_key_info", { provider });
}

/**
 * FID-007 — Remove a saved master key. Wipes the localStorage mirror
 * + the mock-IPC module cache so the next `manifest_soul` call falls
 * through to the static 18-section template. The derived subkey
 * (LS_DERIVED) is intentionally left in place — the user can still
 * chat with the existing subkey until it expires / they hit
 * Disconnect on the Session Key card.
 */
export async function removeMasterKey(provider: string): Promise<void> {
  return invoke("remove_master_key", { provider });
}

export type AppConfig = {
  provider: string;
  modelId: string;
};

/** Persist the runtime config (provider + selected model) to the app vault. */
export async function saveConfig(config: AppConfig): Promise<void> {
  return invoke("save_config", config);
}

/** Load the runtime config from the app vault. Returns null on a fresh install. */
export async function loadConfig(): Promise<AppConfig | null> {
  return invoke("load_config");
}

// ─────────────────────────────────────────────────────────────────
// FID-0003 — Session key derivation (auto-scoped subkey provisioning)
// ─────────────────────────────────────────────────────────────────

// Wire-envelope shape returned by OpenRouter `POST /v1/keys` (verified
// live 2026-07-12 00:55 in dev/fids/0003-auto-derived-session-key.md).
// 20 fields under `data` + 1 top-level sibling `key`.
type ProvisionWireResponse = {
  data?: Record<string, unknown>;
  key?: unknown;
};

/** Normalized SessionKey persisted in `LS_DERIVED` and consumed by chat. */
export type SessionKey = {
  /** 64-char hex; DELETE /v1/keys/<hash> path-segment (NOT `id`). */
  hash: string;
  /** User-supplied on creation; OpenRouter rejects duplicate names. */
  name: string;
  /** Bearer value for `Authorization: Bearer` on chat. Top-level sibling. */
  key: string;
  /** Server-controlled: <prefix>…<suffix>. UI-only — never logged. */
  label: string;
  /** ISO-8601; daily cron ≥24h check reads this. */
  created_at: string;
  /** null = no expiry (OQ-2 inheritance). */
  expires_at: string | null;
  disabled: boolean;
  /** USD cap; null = inherit (OQ-2 inheritance). */
  limit: number | null;
  include_byok_in_limit: boolean;
};

export type ProvisionKeyInput = {
  profile: string;
  agentName: string;
  scope?: {
    limit?: number;
    limitReset?: "daily" | "weekly" | "monthly";
    expiresAt?: string;
  };
};

export type ClearKeyInput = {
  profile: string;
  name: string;
  /** Required — DELETE /v1/keys/<hash>, not <name>. */
  hash: string;
};

/**
 * Flatten the wire envelope to a typed `SessionKey`. Surfaces
 * structured errors for invalid envelopes so the renderer can render
 * the OpenRouter error verbatim (Law 14 — every realistic failure
 * mode has visible feedback, no silent fallbacks).
 */
function normalizeProvisionResponse(wire: ProvisionWireResponse): SessionKey {
  if (!wire || typeof wire !== "object") {
    throw new Error("provision failed: invalid response envelope");
  }
  const data = wire.data;
  if (!data || typeof data !== "object") {
    throw new Error("provision failed: missing data envelope");
  }
  if (data["disabled"] === true) {
    throw new Error(
      "provision failed: OpenRouter returned `disabled: true` for subkey",
    );
  }
  const hash = data["hash"];
  const name = data["name"];
  const label = data["label"];
  const created = data["created_at"];
  if (
    typeof hash !== "string" ||
    typeof name !== "string" ||
    typeof label !== "string" ||
    typeof created !== "string"
  ) {
    throw new Error(
      "provision failed: malformed data envelope — missing hash/name/label/created_at",
    );
  }
  const topKey = wire.key;
  if (typeof topKey !== "string" || !topKey.startsWith("sk-or-v1-")) {
    throw new Error(
      "provision failed: malformed response — missing or invalid top-level `key`",
    );
  }
  const expRaw = data["expires_at"];
  const expires_at = typeof expRaw === "string" ? expRaw : null;
  const limitRaw = data["limit"];
  const limit = typeof limitRaw === "number" ? limitRaw : null;
  return {
    hash,
    name,
    key: topKey,
    label,
    created_at: created,
    expires_at,
    disabled: false,
    limit,
    include_byok_in_limit: Boolean(data["include_byok_in_limit"] ?? false),
  };
}

/**
 * Provision a scoped subkey from the IPC vault. Calls the
 * `provision_session_key` Tauri command (browser-preview: mocked in
 * `mock-ipc.ts` to actually hit OpenRouter `/v1/keys`).
 */
export async function provisionSessionKey(
  input: ProvisionKeyInput,
): Promise<SessionKey> {
  const wire = await invoke<ProvisionWireResponse>(
    "provision_session_key",
    input,
  );
  try {
    return normalizeProvisionResponse(wire);
  } catch (e) {
    // ECHO Law 12 — log structured context (no secret material;
    // `agent_name` is the user-supplied name, not the key bytes)
    // and re-throw so the caller propagates the error to the UI.
    logger.warn(
      "provision normalize failed",
      {
        input_profile: input.profile,
        input_agent: input.agentName,
        code: "openrouter_provision_status_parser",
        agent_name: input.agentName.slice(-4),
      },
      e,
    );
    throw e;
  }
}

/**
 * Delete a previously-provisioned subkey from OpenRouter. DELETE is
 * by `hash`, not `name` (verified live 2026-07-12 00:55). Returns
 * `{ ok: boolean }` — failure surfaces to the renderer, never silently.
 */
export async function clearSessionKey(
  input: ClearKeyInput,
): Promise<{ ok: boolean }> {
  return invoke<{ ok: boolean }>("clear_session_key", input);
}

// ─────────────────────────────────────────────────────────────────
// FID-006 v3 — Soul builder IPC wrappers (Phase 1 mock; real Tauri
// in Phase 2). The mock intercepts these in `mock-ipc.ts`.
// ─────────────────────────────────────────────────────────────────

/**
 * Build a soul from a manifest request. Mirrors
 * `ControlFrame::SoulManifest` at `crates/core/src/types/mod.rs:75`
 * (prompt, name?, bootstrap_tier?, model?) and the gateway handler at
 * `crates/gateway/src/handlers/mod.rs:1718-1982`
 * (`execute_manifestation`).
 *
 * Returns a `ManifestResult` with the full SOUL.md body, metrics
 * (lines, sections, depth_score), and a status discriminator
 * (`"complete"` for LLM success, `"template"` for the no-key
 * fallback, `"error"` for failures). Phase 1: real OpenRouter
 * `POST /v1/chat/completions` call from the browser when a master
 * key is captured in `mockMasters["openrouter"]`, otherwise static
 * 18-section template fallback. Phase 2: real Tauri command writes
 * to `workspace-savant/SOUL.md`.
 */
export async function manifestSoul(
  payload: SoulManifestPayload,
): Promise<ManifestResult> {
  return invoke<ManifestResult>(
    "manifest_soul",
    payload as unknown as Record<string, unknown>,
  );
}

/**
 * Bulk-manifest N agents (each `AgentManifestPlan` becomes a
 * `workspace-savant/SOUL.md` + `workspace-savant/IDENTITY.md` pair).
 * Mirrors `ControlFrame::BulkManifest` at
 * `crates/core/src/types/mod.rs:84` and the server dispatch at
 * `crates/gateway/src/handlers/mod.rs:645-665` (SEC #8: 10 agents max).
 *
 * Returns `{ status: "SWARM_DEPLOYED", count }` on success or
 * `{ status: "REJECTED", reason: "SEC_8_LIMIT_EXCEEDED" }` on the
 * server-side SEC #8 rejection. On success, the mock IPC also
 * persists the payload as the new swarm baseline (FID-013) so the
 * next `getSwarmBaseline` returns it.
 */
export async function bulkManifest(
  payload: BulkManifestPayload,
): Promise<BulkManifestResult> {
  return invoke<BulkManifestResult>(
    "bulk_manifest",
    payload as unknown as Record<string, unknown>,
  );
}

/**
 * FID-013 — Read the current active swarm baseline. Returns the
 * last successfully-deployed `AgentManifestPlan[]` (mock: from
 * localStorage; Phase 2: from Rust state). Empty array on a fresh
 * install (no baseline yet). Used by the /manifest page's
 * "Deploy Swarm" preview to compute the 3-way diff
 * (added/modified/removed vs the proposed deployment).
 */
export async function getSwarmBaseline(): Promise<AgentManifestPlan[]> {
  return invoke<AgentManifestPlan[]>("get_swarm_baseline");
}

// ─────────────────────────────────────────────────────────────────
// FID-010 — Soul generation streaming (SSE).
//
// OpenRouter's `/v1/chat/completions` supports `stream: true` to
// emit Server-Sent Events. We expose a Channel-shaped IPC contract
// that mirrors Tauri v2's `Channel<T>` API (verified in
// dev/fids/FID-2026-07-13-010-streaming-soul-generation.md §Phase 2):
// the renderer creates a channel, subscribes via `onmessage`, passes
// the channel to `invoke("manifest_soul_stream", { ...payload, _channel })`,
// and receives events as the LLM produces tokens.
//
// Phase 1 (browser mock): the `_channel` is a plain
// `ManifestStreamChannel` (duck-typed), passed by reference since
// mockIPC is an in-process function call. Phase 2 (Tauri) will
// swap the channel creation for `new Channel<ManifestStreamEvent>()`
// from `@tauri-apps/api/core` and pass it through the same
// invoke arg slot — the renderer code is unchanged.
// ─────────────────────────────────────────────────────────────────

/** Re-export of the stream event shape from `manifest-mock.ts`. */
export type { ManifestStreamEvent };

/**
 * Channel-shaped event sink for `manifestSoulStream()`. Mirrors
 * Tauri v2's `Channel<T>` shape (a class with `send` for the
 * server-side + `onmessage` for the client-side). We don't
 * directly extend Tauri's `Channel` class because:
 * 1. Tauri is not yet wired up (Phase 1 browser mock).
 * 2. Duck-typing the shape lets us swap implementations without
 *    breaking the renderer.
 *
 * Phase 2 migration: replace `createManifestStreamChannel()` with
 * `new Channel<ManifestStreamEvent>()` and update the mock to
 * forward events via `channel.send()`. The `onmessage(handler)`
 * API stays identical.
 */
export interface ManifestStreamChannel {
  /** Subscribe to events. Returns an unsubscribe function. */
  onmessage(handler: (event: ManifestStreamEvent) => void): () => void;
  /** Internal — called by the IPC handler. Not part of the
   *  public renderer API; exposed for the mock implementation. */
  send(event: ManifestStreamEvent): void;
}

/**
 * Create a stream channel. The returned object is passed to
 * `manifestSoulStream()` as the `_channel` arg. The handler
 * receives events as the LLM produces tokens.
 */
export function createManifestStreamChannel(): ManifestStreamChannel {
  const handlers: Array<(e: ManifestStreamEvent) => void> = [];
  return {
    onmessage: (handler) => {
      handlers.push(handler);
      return () => {
        const idx = handlers.indexOf(handler);
        if (idx >= 0) handlers.splice(idx, 1);
      };
    },
    send: (event) => {
      // Snapshot the handlers list so an unsubscribe inside a
      // handler doesn't skip the remaining listeners.
      for (const h of [...handlers]) h(event);
    },
  };
}

/**
 * Handle returned by `manifestSoulStream()`. The renderer uses
 * `cancel()` to abort an in-flight stream (e.g. from a Cancel
 * button) and `done` to await final cleanup.
 */
export interface ManifestStreamHandle {
  /** Fire-and-forget cancel. Tears down the underlying fetch +
   *  SSE parser. The renderer should immediately reset its UI
   *  state and ignore any straggler events that arrive after
   *  cancel (the mock guarantees no further `send()` calls
   *  after the abort settles, but a few in-flight chunks may
   *  already be in the channel dispatch queue). */
  cancel(): void;
  /** Resolves when the stream ends (complete / error / cancel).
   *  The renderer can use this to coordinate cleanup (e.g. clear
   *  the elapsed-time ticker). */
  done: Promise<void>;
}

/**
 * Stream a soul generation via OpenRouter SSE. Yields
 * `preamble` / `chunk` / `complete` / `error` events through the
 * channel. Returns a handle with `cancel()` (fire-and-forget
 * abort) + `done` (resolves when the stream ends).
 *
 * The channel is the sole event sink — no streamId is needed
 * because the renderer's Cancel button calls `handle.cancel()`
 * directly (closes over the AbortController in the mock IPC
 * command). Phase 2 Tauri migration is a drop-in replacement of
 * the channel factory (see `createManifestStreamChannel`).
 */
export async function manifestSoulStream(
  payload: SoulManifestPayload,
  channel: ManifestStreamChannel,
): Promise<ManifestStreamHandle> {
  return invoke<ManifestStreamHandle>("manifest_soul_stream", {
    ...payload,
    _channel: channel,
  } as unknown as Record<string, unknown>);
}

// ─────────────────────────────────────────────────────────────────
// FID-029 §Step 2 — Chat Persistence IPC wrappers
//
// Renderer-side TypeScript surface for the 5 chat-persistence
// commands. The Tauri-runtime DAEMON commands live at
// `src-tauri/src/lib.rs::FID-029 Phase-4` (deferred to FID-032 per
// master FID-035 §Layered Build Order Layer 3; for v0.0.8 the
// browser-preview mock in `src/lib/mock-ipc.ts` is the active path).
//
// Returns `Promise<T>` per FID-029 §Missed Questions #11 — the IPC
// bridge turns `Err(String)` into a thrown Promise, so the renderer
// catches with `try { await listChatSessions() } catch (e) {...}`.
// The renderer does NOT use `{ ok: false, error: "..." }` return
// shapes.
// ─────────────────────────────────────────────────────────────────

import type {
	ChatMessage,
	ChatSearchResult,
	ChatSession,
} from "@/lib/chat-data";

export async function listChatSessions(): Promise<ChatSession[]> {
	return invoke<ChatSession[]>("list_chat_sessions");
}

export async function loadChatHistory(
	sessionId: string,
	limit = 100,
): Promise<ChatMessage[]> {
	return invoke<ChatMessage[]>("load_chat_history", {
		sessionId,
		limit,
	});
}

export async function persistChatTurn(
	sessionId: string,
	userMessage: ChatMessage,
	assistantMessage: ChatMessage,
): Promise<void> {
	return invoke<void>("persist_chat_turn", {
		sessionId,
		userMessage,
		assistantMessage,
	});
}

export async function deleteChatSession(sessionId: string): Promise<void> {
	return invoke<void>("delete_chat_session", { sessionId });
}

export async function searchChatHistory(
	query: string,
	limit = 50,
): Promise<ChatSearchResult[]> {
	return invoke<ChatSearchResult[]>("search_chat_history", {
		query,
		limit,
	});
}

// ─────────────────────────────────────────────────────────────────
// FID-017 — Inner monologue IPC wrappers
//
// Mirrors the savant-orig `crates/agent/src/{pulse,consciousness,learning}/`
// surfaces through Tauri commands. The 12-lens rotation is pulled from
// `savant_agent::pulse::prompts::LENSES` on the Rust side; this is just
// the bridge.
// ─────────────────────────────────────────────────────────────────

export type ConsciousnessState =
  | "THINKING"
  | "IDLE"
  | "DORMANT"
  | "WONDERING"
  | "UNKNOWN";

/**
 * FID-017 — Confirm the AppState is reachable. No-op in Tauri runtime
 * (AppState is initialized in the Tauri `setup()` callback); useful as
 * a renderer-side smoke test to verify the IPC bridge is wired.
 */
export async function initializeAppState(workspacePath: string): Promise<void> {
  return invoke<void>("initialize_app_state", { workspacePath });
}

/**
 * FID-017 — Start the consciousness daemon in a background Tokio task.
 * Returns the initial state ("THINKING"). The daemon cycles through
 * THINKING/IDLE/DORMANT/WONDERING every 5s; full LLM-driven synthesis
 * comes in FID-018+ (this MVP proves the lifecycle works).
 */
export async function startConsciousness(): Promise<ConsciousnessState> {
  return invoke<ConsciousnessState>("start_consciousness");
}

/**
 * FID-017 — Stop the consciousness daemon. Triggers the
 * CancellationToken and awaits the JoinHandle.
 */
export async function stopConsciousness(): Promise<void> {
  return invoke<void>("stop_consciousness");
}

/**
 * FID-017 — Read the current consciousness state (raw AtomicU8 from
 * the daemon's state handle, decoded to the human-readable enum).
 */
export async function getConsciousnessState(): Promise<ConsciousnessState> {
  return invoke<ConsciousnessState>("get_consciousness_state");
}

/**
 * FID-017 — Trigger a single reflection. Picks the next lens from the
 * 19-entry rotation (or honors `lensOverride`), calls OpenRouter with
 * the model the user selected in Settings, and appends the result to
 * `workspace-savant/REFLECTIONS.md`.
 *
 * The `model` arg is REQUIRED in the sense that the caller MUST pass
 * the model the user picked via Settings (`useLoadedConfig().modelId`).
 * We do not hardcode a default model — the entire point of the
 * Settings page is that the user selects which model to run. If the
 * caller passes `null`/`undefined`, the mock will surface a clear
 * "no model configured" error pointing the user to Settings rather
 * than silently picking a different model.
 *
 * Returns the new narrative body. The full REFLECTIONS.md entry
 * (header + body) is in the file; this return value is just the body
 * for UI display.
 */
export async function triggerReflection(
  lensOverride?: string,
  model?: string,
): Promise<string> {
  return invoke<string>("trigger_reflection", {
    lensOverride: lensOverride ?? null,
    model: model ?? null,
  });
}

// ─────────────────────────────────────────────────────────────────
// FID-028 — Scaffold 3 placeholder pages (changelog / faq / tune)
// by wiring them to the real Rust engines (gateway's /api/changelog
// + LlmParams::get_parameter_descriptors() + curated FAQ data).
// See [`dev/fids/FID-2026-07-14-028-scaffold-changelog-faq-tune-pages.md`]
// for the full FID body.
// ─────────────────────────────────────────────────────────────────

/**
 * FID-028 (Spencer correction 2026-07-14) — Return the project's
 * CHANGELOG.md content as a string. The source of truth is GitHub
 * (see `CHANGELOG_GITHUB_RAW_URL` in `src/lib/changelog.ts`), NOT
 * the local `CHANGELOG.md` file. End users downloading the app
 * don't have the local file; the GitHub-hosted copy is canonical.
 *
 * The fetch is client-side (no IPC roundtrip needed) and goes
 * through `src/lib/changelog.ts::fetchChangelog()`. The Tauri
 * runtime could alternatively route through the gateway's
 * `/api/changelog` endpoint (which reads the local file with
 * `include_str!` fallback at [`crates/gateway/src/server.rs:1593`])
 * — but per the correction, GitHub is the source of truth, so the
 * client-side fetch is preferred.
 */
export async function getChangelog(): Promise<string> {
  return fetchChangelog();
}

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

/**
 * FID-028 — Return the LLM parameter descriptors (the same shape
 * the gateway's `/api/models` returns at runtime). The Tune page
 * renders these as a tunable form. Ranges match the gateway's
 * `settings_post_handler` clamp values (see
 * `crates/gateway/src/server.rs:1110-1180`).
 */
export async function getParameterDescriptors(): Promise<ParameterDescriptor[]> {
  return invoke<ParameterDescriptor[]>("get_parameter_descriptors");
}

/**
 * FID-028 (Spencer revision 2026-07-14) — Return the 4 TRUE tuning
 * parameter descriptors (filtered from `get_parameter_descriptors`).
 * The Tune page uses this; model-selection fields (chat_model,
 * manifestation_model, vision_model, provider, ollama_url) are
 * configured on the Settings page. Per Spencer's verbatim: *"Fine
 * tuning is for the actual model it's self, this page is not to
 * change models."*
 *
 * Filter source: `src/lib/parameter-descriptors.ts::TUNING_FIELDS`
 * (a `Set<string>` of the 4 sampling-knob names: temperature, top_p,
 * frequency_penalty, presence_penalty). The full
 * `get_parameter_descriptors` IPC command still returns all 9 (the
 * gateway contract) for the future Settings wiring.
 */
export async function getTuningDescriptors(): Promise<ParameterDescriptor[]> {
  return invoke<ParameterDescriptor[]>("get_tuning_descriptors");
}

/**
 * FID-028 Revision 2 (Spencer 2026-07-14) — Persist the 4 tuning
 * parameter values. The Tune page's Apply button dispatches this;
 * the gateway's `POST /api/settings` (see
 * `crates/gateway/src/server.rs:1110-1180`) clamps to the OpenAI
 * ranges before persisting.
 *
 * Input: a `Record<string, number>` keyed by the gateway's
 * snake_case param names (temperature, top_p, frequency_penalty,
 * presence_penalty). Only the 4 known knobs are sent; unknown
 * keys are dropped by the gateway's clamp step.
 *
 * The browser-preview mock stores the values in module-scoped
 * state + returns `{ ok: true }`; the Tauri runtime would route
 * through the gateway's `POST /api/settings` endpoint.
 */
export type SaveSettingsInput = Readonly<Record<string, number>>;

export async function saveSettings(
  values: SaveSettingsInput,
): Promise<{ ok: boolean }> {
  return invoke<{ ok: boolean }>("save_settings", { values });
}

export type FaqItem = {
  question: string;
  answer: string;
};

/**
 * FID-028 — Return the curated FAQ items. No real FAQ module
 * exists in the savant-orig (the 6-8 Q&A are grounded in the
 * project's own CHANGELOG / README / LEARNINGS artifacts). The
 * mock IPC serves the data directly; a real gateway FAQ endpoint
 * is a follow-on FID.
 */
export async function getFaq(): Promise<FaqItem[]> {
  return invoke<FaqItem[]>("get_faq");
}
