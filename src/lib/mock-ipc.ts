"use client";

// Browser preview mock for Tauri IPC.
//
// In a normal browser, @tauri-apps/api/core's `invoke()` fails because there's
// no Tauri runtime. We install @tauri-apps/api/mocks' `mockIPC()` to intercept
// the invoke calls and return realistic data. The renderer code in src/lib/ipc.ts
// doesn't change — it just calls `invoke('setup_master_key', ...)` and the mock
// answers.
//
// Branching is automatic:
//   - Tauri desktop: window.__TAURI_INTERNALS__ is set → no mock, real IPC
//   - Browser preview: window.__TAURI_INTERNALS__ undefined → mock installed

import { mockIPC } from "@tauri-apps/api/mocks";
import type { InvokeArgs } from "@tauri-apps/api/core";
import type { AppConfig, ProfileSummary } from "./ipc";
import type { ChatMessage } from "./chat-data";
import {
  sortedSessions,
  loadChatHistoryMock,
  persistTurnMock,
  deleteSessionMock,
  searchHistoryMock,
} from "./mock-chat";
import {
  generateSoul,
  generateSoulStream,
  type ManifestStreamEvent,
} from "./manifest-mock";
import { LENSES } from "./reflections/lenses";
// FID-028 — import the data modules for the 2 mock cases that
// still need the mock layer (parameter_descriptors + faq). The
// changelog case is REMOVED in the Spencer correction pass: the
// changelog now comes from GitHub (client-side fetch) so no
// mock layer is needed — the IPC wrapper `getChangelog()` calls
// `fetchChangelog()` directly. See `src/lib/changelog.ts` for the
// full rationale (end users don't have the local CHANGELOG.md;
// GitHub is the canonical source).
import { getParameterDescriptors, getTuningDescriptors } from "./parameter-descriptors";
import { getFaqItems } from "./faq-data";
import type {
  AgentManifestPlan,
  BootstrapTier,
  BulkManifestResult,
  ManifestResult,
  SoulManifestPayload,
} from "@/types/control-frames";
import { logger } from "@/lib/logger";

// Tauri injects __TAURI_INTERNALS__ on the window object when running inside
// the webview. The @tauri-apps/api umbrella doesn't export this as a typed
// property, so we augment Window locally for this module.
declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

// FID-007 — localStorage keys for browser-preview persistence.
// `mockMasters` + `mockProfiles` were previously module-scoped and
// got wiped every time `setupMockIPC()` ran (HMR / route re-mount),
// which made the Settings page show the Master Key input as blank
// even after a successful save + the manifest page fall back to the
// static template. Persisting to `localStorage` matches the existing
// `LS_DERIVED` pattern (a real OpenRouter subkey is already stored
// there) and survives HMR + route re-mounts.
//
// SECURITY NOTE: in the Tauri desktop runtime these bytes would live
// in the OS keychain via `tauri-plugin-stronghold`; localStorage is
// the browser-preview equivalent. The `LS_MASTER` key is intentionally
// NOT a `LS_*` const from `@/lib/hooks/use-loaded-config` because the
// master key write path is mock-IPC-internal, not a renderer hook.
export const LS_MASTER = "savant.master.openrouter";
// `LS_PROFILES` is intentionally NOT exported (YAGNI — no external
// consumer; profiles are managed by `listProfiles` / the mock IPC
// switch). Promote to `export` only when a second consumer emerges.
const LS_PROFILES = "savant.vault.profiles";
// FID-013 — Swarm baseline. Persists the last successfully-deployed
// `AgentManifestPlan[]` so the next `get_swarm_baseline` can compute
// the diff preview (added/modified/removed vs active). Cleared on
// HMR (HMR resets the module, which is fine — the manifest page
// re-reads on mount). Phase 2 will replace with real Rust state.
const LS_SWARM_BASELINE = "savant.bulk.baseline";
// FID-017 — Reflections entries (browser preview stand-in for
// `workspace-savant/REFLECTIONS.md`). Written by the mock
// `trigger_reflection` case; read by `useReflections` so the
// timeline section can show the latest entries. Cleared on HMR
// (HMR resets the module); the Tauri runtime reads/writes the real
// `workspace-savant/REFLECTIONS.md` via the savant_agent crate.
// Shared with `src/lib/hooks/use-reflections.ts` — single source of
// truth for the localStorage key (both files import the same const).
//
// Key renamed 2026-07-13 from `savant.monologue.reflections` to
// `savant.reflections.entries` for naming hygiene (the dashboard
// feature is "reflections", not "monologue"). Old entries in
// users' localStorage from before the rename are silently ignored —
// the new write path starts a fresh stream.
const MOCK_REFLECTIONS_KEY = "savant.reflections.entries";

let mockProfiles: ProfileSummary[] = [];
let mockConfig: AppConfig | null = null;

// Per-profile master key mirror — populated by `setup_master_key` so
// `provision_session_key` + `clear_session_key` + `manifest_soul` can
// authorize their real `/v1/keys` + `/v1/chat/completions` HTTP calls.
// Persisted to `localStorage` (FID-007) so it survives HMR / route
// re-mounts. Master bytes never leave this module; chat outbound
// traffic only sees the derived (subkey) `key` field.
let mockMasters: Record<string, string> = {};

// FID-028 Revision 2 (Spencer 2026-07-14) — Per-knob tuning value
// mirror (the 4 sampling knobs: temperature, top_p, frequency_penalty,
// presence_penalty). Populated by the `save_settings` mock case when
// the Tune page's Apply button fires. The Tune page ALSO writes to
// `localStorage[LS_TUNE_SETTINGS]` directly (UI concern, not IPC) so
// the values survive page reloads without a `loadSettings` IPC
// command. The mock case just acknowledges the dispatch; the Tauri
// runtime would route through the gateway's `POST /api/settings`.
let mockTuningValues: Record<string, number> = {};

// FID-008 — env var master key (tier 1, highest priority). Cached on
// `setupMockIPC()` init from `/api/env`. The env var shadows any
// vault entry when set. The renderer's `getMasterKeyInfo` reads
// `source: "env" | "vault" | "none"` to render the right UI.
// `effectiveMasterKey()` is the single point of override-precedence
// resolution: env > vault > "" (template fallback).
let _envMasterKey: string | null = null;

function hydrateEnvMasterKey(): void {
  if (typeof window === "undefined") return;
  // Fire-and-forget; the first call to `get_master_key_info` may
  // race and return `source: "none"`. We dispatch a custom event
  // when the fetch resolves so the Settings page can re-fetch
  // `masterInfo` and switch from "none" → "env" without waiting
  // for the next storage event. This closes the cold-start race
  // (typically < 100ms on localhost, but non-zero).
  fetch("/api/env")
    .then((r) => r.json())
    .then((data: { openrouterMasterKey?: string | null }) => {
      _envMasterKey = data.openrouterMasterKey ?? null;
      if (typeof window !== "undefined") {
        window.dispatchEvent(new CustomEvent("savant:env-master-key-hydrated"));
      }
    })
    .catch(() => {
      /* network error / 404 / server down — leave _envMasterKey null */
    });
}

/**
 * Resolve the effective master key for a provider per the override
 * precedence: env var (tier 1) > vault entry (tier 2) > "" (no
 * key; the next `manifest_soul` call falls through to the static
 * 18-section template). The env var shadows the vault when set,
 * but the vault entry is still saved (for when the env var is
 * unset).
 */
function effectiveMasterKey(provider: string): string {
  return _envMasterKey ?? mockMasters[provider] ?? "";
}

function persistMasters(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(LS_MASTER, JSON.stringify(mockMasters));
  } catch {
    /* noop — quota / private-mode fail is non-fatal for the session */
  }
}

function hydrateMasters(): void {
  if (typeof window === "undefined") return;
  const raw = window.localStorage.getItem(LS_MASTER);
  if (!raw) return;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const out: Record<string, string> = {};
      for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
        if (typeof v === "string" && v.length > 0) out[k] = v;
      }
      mockMasters = out;
    }
  } catch {
    /* malformed JSON — leave mockMasters empty; the user will re-save */
  }
}

function persistProfiles(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(LS_PROFILES, JSON.stringify(mockProfiles));
  } catch {
    /* noop */
  }
}

function hydrateProfiles(): void {
  if (typeof window === "undefined") return;
  const raw = window.localStorage.getItem(LS_PROFILES);
  if (!raw) return;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (Array.isArray(parsed)) {
      mockProfiles = parsed.filter(
        (p): p is ProfileSummary =>
          p !== null &&
          typeof p === "object" &&
          typeof (p as ProfileSummary).name === "string" &&
          typeof (p as ProfileSummary).provider === "string",
      );
    }
  } catch {
    /* malformed JSON — leave mockProfiles empty */
  }
}

// Full wire-envelope responses keyed by `${profile}:${agentName}` for
// regression inspection. The renderer never sees these directly — they
// flow through `normalizeProvisionResponse` in `../lib/ipc.ts`.
const mockReports: Record<
  string,
  { data: Record<string, unknown>; key: string }
> = {};

// FID-006 v3 (reopened 2026-07-13) — built-soul map. Each entry is
// the result of a `manifest_soul` IPC dispatch (now returning the
// full `ManifestResult` with content + metrics + status). Most-recent-
// first. Persists across mock-IPC re-installs within the same page
// session; cleared on HMR (HMR resets the module, which is fine —
// the manifest page reads from localStorage for cross-reload
// persistence).
type BuiltSoulEntry = {
  ts: number;
  prompt: string;
  name: string | null;
  tier: string | null;
  status: ManifestResult["status"];
  metrics: ManifestResult["metrics"];
  content: string;
  note?: string;
  error?: string;
};
const builtSouls: BuiltSoulEntry[] = [];

const OPENROUTER_PROVISION_URL = "https://openrouter.ai/api/v1/keys";
const OPENROUTER_DELETE_KEY_URL_FMT = (hash: string): string =>
  `https://openrouter.ai/api/v1/keys/${hash}`;

// FID-017 — chat-completion URL for the inner monologue mock. The
// model itself is NOT hardcoded — the page reads it from the user's
// saved config (via useLoadedConfig) and passes it through the
// triggerReflection args. The Settings page is the only place a
// model gets chosen; this mock honors that choice and errors out
// (rather than falling back to a random default) if no model is
// configured.
const OPENROUTER_CHAT_URL = "https://openrouter.ai/api/v1/chat/completions";

export function setupMockIPC(): void {
  if (typeof window === "undefined") return; // server-side, no-op
  if (window.__TAURI_INTERNALS__) return; // real Tauri runtime, no mock needed

  // Hydrate BEFORE the reset so the wipe doesn't drop user data.
  // Then reset module-scoped transient state (sessions, reports,
  // soul history) but PRESERVE the persisted master + profiles —
  // those survive HMR by design (FID-007).
  hydrateMasters();
  hydrateProfiles();
  hydrateEnvMasterKey();
  mockConfig = null;
  for (const k of Object.keys(mockReports)) delete mockReports[k];
  for (let i = builtSouls.length - 1; i >= 0; i--) builtSouls.splice(i, 1);

  // InvokeArgs is a union (Record<string, unknown> | number[] | ...), so
  // we cast to Record<string, unknown> for ergonomic property access on
  // the key/value variant. The mock commands we handle always pass
  // key/value args, so this narrowing is safe.
  mockIPC((cmd: string, payload?: InvokeArgs) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case "vault_list_profiles":
        return [...mockProfiles];

      case "setup_master_key": {
        const provider = String(args.provider ?? "openrouter");
        const apiKey = String(args.apiKey ?? "");
        // Capture master per profile so sibling commands can authorize
        // their real fetch. Persisted to localStorage (FID-007) so
        // HMR / route re-mounts don't wipe the saved key.
        mockMasters[provider] = apiKey;
        persistMasters();
        const profile: ProfileSummary = {
          name: `${provider}-default`,
          provider,
          method: "api_key",
          secret_ref_kind: "env",
          base_url:
            provider === "openrouter" ? "https://openrouter.ai/api/v1" : null,
          updated_at: Math.floor(Date.now() / 1000),
        };
        const idx = mockProfiles.findIndex((p) => p.name === profile.name);
        if (idx >= 0) mockProfiles[idx] = profile;
        else mockProfiles.push(profile);
        persistProfiles();
        return null;
      }

      // FID-007 + FID-008 — Return a redacted summary of the effective
      // master key (existence + last-4 + savedAt + source). The
      // `source` discriminator tells the Settings page which tier is
      // active: `"env"` (tier 1, env var), `"vault"` (tier 2, saved
      // via `setup_master_key`), or `"none"` (no key). NEVER returns
      // the actual key bytes (Law 12).
      case "get_master_key_info": {
        const provider = String(args.provider ?? "openrouter");
        // Env var (tier 1) shadows the vault when set.
        if (_envMasterKey) {
          return {
            exists: true,
            last4: _envMasterKey.slice(-4),
            savedAt: null,
            source: "env" as const,
          };
        }
        // Vault entry (tier 2).
        const master = mockMasters[provider] ?? "";
        if (!master) return { exists: false, source: "none" as const };
        return {
          exists: true,
          last4: master.slice(-4),
          savedAt:
            mockProfiles.find((p) => p.provider === provider)?.updated_at ??
            null,
          source: "vault" as const,
        };
      }

      // FID-007 — Remove a saved master key. Wipes BOTH the
      // localStorage mirror AND the module-scoped cache so the next
      // `manifest_soul` call falls through to the static template
      // (UNLESS the env var is set, in which case the env var still
      // authorizes calls — the vault entry was already shadowed).
      // The derived subkey (LS_DERIVED) is left in place — the user
      // can still chat with the existing subkey until it expires or
      // they hit Disconnect on the Session Key card.
      case "remove_master_key": {
        const provider = String(args.provider ?? "openrouter");
        delete mockMasters[provider];
        persistMasters();
        mockProfiles = mockProfiles.filter((p) => p.provider !== provider);
        persistProfiles();
        return null;
      }

      // Provision a scoped subkey from OpenRouter `/v1/keys`. Real
      // HTTP call (mock IPC's realness principle, FID Lessons Learned).
      // Output is the RAW wire envelope (data + top-level key); the
      // bridge's `normalizeProvisionResponse` flattens it to
      // `SessionKey` for the renderer.
      case "provision_session_key": {
        const profile = String(args.profile ?? "openrouter");
        const agentName = String(args.agentName ?? "");
        // FID-008 — env var (tier 1) > vault (tier 2) > "" (no key).
        const master = effectiveMasterKey(profile);
        if (!master) {
          throw new Error(
            `Mock IPC: no master captured for profile '${profile}'. Call setup_master_key first.`,
          );
        }
        const body: Record<string, unknown> = { name: agentName };
        const scope = args.scope as
          | { limit?: number; limitReset?: string; expiresAt?: string }
          | undefined;
        if (scope?.limit !== undefined) body["limit"] = scope.limit;
        if (scope?.limitReset) body["limit_reset"] = scope.limitReset;
        if (scope?.expiresAt) body["expires_at"] = scope.expiresAt;

        // Fire-and-forget on a then-chain so we're synchronous-shaped
        // for the mock return — but the mockIPC callback returns the
        // Promise so awaiting is fine in practice.
        const wire$: Promise<{ data: Record<string, unknown>; key: string }> =
          (async () => {
            const response = await fetch(OPENROUTER_PROVISION_URL, {
              method: "POST",
              headers: {
                Authorization: `Bearer ${master}`,
                "Content-Type": "application/json",
              },
              body: JSON.stringify(body),
            });
            if (!response.ok) {
              const text = await response.text();
              throw new Error(
                `Mock IPC: OpenRouter /v1/keys ${response.status}: ${text.slice(0, 200)}`,
              );
            }
            const parsed = (await response.json()) as {
              data: Record<string, unknown>;
              key: string;
            };
            mockReports[`${profile}:${agentName}`] = parsed;
            return parsed;
          })();
        // Resolve the async fetch via the return Promise; mockIPC
        // accepts async returns, so this propagates correctly to
        // `await invoke(...)` in the IPC bridge.
        return wire$;
      }

      // Delete a previously-provisioned subkey. Real DELETE call;
      // DELETE is by `hash`, not `name` (verified live 2026-07-12).
      // The signature still carries `name` for traceability on the
      // upstream call site (it's the human-readable label of the
      // subkey being deleted) but the mock doesn't need it for the
      // HTTP DELETE path-segment.
      case "clear_session_key": {
        const profile = String(args.profile ?? "openrouter");
        const hash = String(args.hash ?? "");
        // FID-008 — env var (tier 1) > vault (tier 2) > "" (no key).
        const master = effectiveMasterKey(profile);
        if (!master || !hash) {
          // Silent { ok: false } on disconnect-without-master so the
          // renderer can keep the UX flat (failure surfaces in
          // already-rendered error banner, not as a new exception).
          return { ok: false };
        }
        const response$ = fetch(OPENROUTER_DELETE_KEY_URL_FMT(hash), {
          method: "DELETE",
          headers: { Authorization: `Bearer ${master}` },
        }).then((response) => {
          // Cleanup the report entry on response (success or failure:
          // local cache invalidation is independent of upstream ack).
          for (const k of Object.keys(mockReports)) {
            const r = mockReports[k];
            if (
              k.startsWith(`${profile}:`) &&
              typeof r?.data?.["hash"] === "string" &&
              r.data["hash"] === hash
            ) {
              delete mockReports[k];
            }
          }
          return { ok: response.ok };
        });
        return response$;
      }

      case "infer_openrouter": {
        const prompt = String(args.prompt ?? "");
        const preview = prompt.slice(0, 80);
        return `[mock response — browser preview only] Received ${prompt.length} chars: "${preview}${prompt.length > 80 ? "..." : ""}"`;
      }

      case "save_config": {
        const provider = String(args.provider ?? "openrouter");
        const modelId = String(args.modelId ?? "");
        mockConfig = { provider, modelId };
        return null;
      }

      case "load_config": {
        return mockConfig;
      }

      // FID-006 v3 (reopened 2026-07-13) — Soul builder. Phase 1
      // mock for the Tauri `manifest_soul` command. Mirrors the
      // gateway handler at `crates/gateway/src/handlers/mod.rs:1718-1982`
      // (`execute_manifestation`): makes a real OpenRouter
      // `POST /v1/chat/completions` call when an OpenRouter master key
      // is captured in `mockMasters["openrouter"]` (set by
      // `setup_master_key`); otherwise falls back to the static
      // 18-section template with `status: "template"`. The result is
      // a `ManifestResult` shape mirroring the savant-orig
      // `MANIFEST_DRAFT` payload at
      // [`mod.rs:1917-1942`].
      case "manifest_soul": {
        const payload = args as Partial<SoulManifestPayload>;
        const prompt = String(payload.prompt ?? "").trim();
        if (!prompt) {
          throw new Error("Mock IPC: manifest_soul requires `prompt`");
        }
        const name = payload.name?.trim() ? payload.name.trim() : null;
        // snake_case `bootstrap_tier` matches the Rust IPC field name at
        // `crates/core/src/types/mod.rs:75-79` and the dashboard's
        // `sendControlFrame` payload shape (PB-F).
        const tier: BootstrapTier =
          (payload.bootstrap_tier as BootstrapTier | undefined) ?? "grounded";
        // FID-008 — env var (tier 1) > vault (tier 2) > "" (no key).
        const masterKey = effectiveMasterKey("openrouter");
        // FID-009 — diagnostic: log the active source + key length so
        // the user can verify which tier is authorizing the LLM call
        // (env var vs vault entry vs none). Helps debug the "Template
        // generated" fallback if the user thinks they have a key set
        // but the mock IPC is reading from a different source.
        // ECHO Law 12 — the `redact()` helper ensures the source
        // discriminator + key length (NOT the key bytes) are logged.
        if (typeof window !== "undefined") {
          logger.info("manifest_soul: source + key length", {
            source: _envMasterKey ? "env" : masterKey ? "vault" : "none",
            key_len: masterKey.length,
          });
        }
        // Optional model hint from the manifest page (reads
        // `useLoadedConfig()`); falls back to the mock default if
        // absent. Phase 2 will read `ai.manifestation_model` from
        // savant-orig config.
        const model = String(payload.model ?? "");
        // Return a Promise directly (matches the `provision_session_key`
        // pattern in this same mockIPC switch — the mock framework
        // awaits the return value). The `builtSouls.unshift` side
        // effect runs after the LLM call resolves so the entry
        // reflects the actual result.
        return generateSoul(prompt, name, tier, masterKey, model).then(
          (result) => {
            builtSouls.unshift({
              ts: Date.now(),
              prompt,
              name,
              tier,
              status: result.status,
              metrics: result.metrics,
              content: result.content,
              note: result.note,
              error: result.error,
            });
            // Cap to last 20 to keep module state bounded.
            while (builtSouls.length > 20) builtSouls.pop();
            return result;
          },
        );
      }

      // FID-010 — Soul generation streaming (SSE). The renderer
      // passes a `ManifestStreamChannel` (duck-typed Phase-1 mock
      // of Tauri v2's `Channel<ManifestStreamEvent>`). The mock
      // runs `generateSoulStream()` in a background task and pipes
      // each yielded event through `channel.send()`. Returns a
      // `{ cancel, done }` handle so the renderer can abort the
      // in-flight stream (e.g. from the Cancel button) and wait
      // for the loop to unwind.
      //
      // The channel is passed BY REFERENCE in browser mock
      // (mockIPC is a function call, not serialized). Phase 2 Tauri
      // will pass a real `Channel<ManifestStreamEvent>` and ignore
      // the `_channel` payload field. The `done` promise is the
      // IIFE's return value — it resolves when the background
      // loop unwinds (naturally OR via abort).
      case "manifest_soul_stream": {
        const payload = args as Partial<SoulManifestPayload>;
        const prompt = String(payload.prompt ?? "").trim();
        if (!prompt) {
          throw new Error("Mock IPC: manifest_soul_stream requires `prompt`");
        }
        const name = payload.name?.trim() ? payload.name.trim() : null;
        const tier: BootstrapTier =
          (payload.bootstrap_tier as BootstrapTier | undefined) ?? "grounded";
        const masterKey = effectiveMasterKey("openrouter");
        const model = String(payload.model ?? "");
        // The renderer-supplied channel (see `manifestSoulStream`
        // in `../lib/ipc.ts`). The `_streamId` is generated by
        // the wrapper for future cross-scope cancellation; not
        // needed today since `handle.cancel()` covers the only
        // caller (the manifest page's Cancel button).
        const channel = args._channel as
          { send: (e: ManifestStreamEvent) => void } | undefined;
        if (!channel) {
          throw new Error(
            "Mock IPC: manifest_soul_stream requires a `_channel` arg",
          );
        }
        const controller = new AbortController();
        // Background loop wrapped in an IIFE whose return value
        // IS the `done` promise. The `try/catch` pattern is
        // built into the IIFE — no separate `resolveDone`
        // variable needed. This is cleaner than the
        // `let resolveDone` + `new Promise` pattern (which trips
        // TS's "used before assigned" check on `resolveDone`).
        //
        // CONTRACT: the `done` promise always RESOLVES (never
        // rejects). Errors from `generateSoulStream` are caught
        // in the `catch` block below and surfaced as `error`
        // events via the channel, not as rejections. The renderer
        // must check the channel's `onmessage` for the terminal
        // event (`complete` | `error`) to know the stream's
        // outcome — `await handle.done` is purely for cleanup
        // timing (resets streaming state when the loop unwinds).
        const done = (async (): Promise<void> => {
          try {
            for await (const event of generateSoulStream(
              prompt,
              name,
              tier,
              masterKey,
              model,
              controller.signal,
            )) {
              if (controller.signal.aborted) break;
              channel.send(event);
              if (event.type === "complete" || event.type === "error") {
                break;
              }
            }
          } catch (e) {
            if (!controller.signal.aborted) {
              channel.send({
                type: "error",
                error: `Stream failed: ${
                  e instanceof Error ? e.message : String(e)
                }`,
              });
            }
          }
        })();
        return {
          cancel: (): void => {
            controller.abort();
          },
          done,
        };
      }

      // FID-006 v3 — Swarm deployment (Phase 1 mock for the Tauri
      // `bulk_manifest` command). Mirrors the server-side dispatch at
      // `crates/gateway/src/handlers/mod.rs:645-665` (SEC #8 limit of
      // 10 agents per BulkManifest request).
      case "bulk_manifest": {
        const agents = (args.agents as unknown[] | undefined) ?? [];
        const SEC_8_LIMIT = 10;
        if (agents.length > SEC_8_LIMIT) {
          return {
            status: "REJECTED",
            reason: "SEC_8_LIMIT_EXCEEDED",
          } satisfies BulkManifestResult;
        }
        // FID-013 — Persist the successfully-deployed swarm as the
        // new baseline so the next `get_swarm_baseline` returns it.
        // The renderer uses this to compute the diff preview
        // (added/modified/removed vs the active deployment). Phase
        // 2 will replace localStorage with a real Rust state read
        // (parse `workspace-savant/SOUL.md` for each agent).
        //
        // NOTE: this write is NOT transactional with the deploy
        // success — if `bulkManifest` returns SWARM_DEPLOYED but
        // the localStorage write throws (quota exceeded /
        // private-mode), the deploy is reported as success but
        // the baseline is stale. Next diff would show every
        // agent as "modified" (old baseline vs new proposed).
        // Low risk in practice (localStorage is per-origin and
        // not typically quota-constrained) but worth documenting.
        try {
          if (typeof window !== "undefined") {
            window.localStorage.setItem(
              LS_SWARM_BASELINE,
              JSON.stringify(agents),
            );
          }
        } catch {
          /* noop — quota / private-mode fail doesn't fail the deploy */
        }
        return {
          status: "SWARM_DEPLOYED",
          count: agents.length,
        } satisfies BulkManifestResult;
      }

      // FID-013 — Read the current active swarm baseline. Returns
      // the last successfully-deployed `AgentManifestPlan[]` from
      // localStorage, or `[]` if no baseline exists yet (first
      // deploy — all proposed agents will be "ADDED" in the diff).
      case "get_swarm_baseline": {
        if (typeof window === "undefined") return [];
        try {
          const raw = window.localStorage.getItem(LS_SWARM_BASELINE);
          if (!raw) return [];
          const parsed = JSON.parse(raw) as unknown;
          if (!Array.isArray(parsed)) return [];
          // Validate each entry has the AgentManifestPlan shape
          // (defensive — corrupt LS shouldn't crash the UI).
          return parsed.filter(
            (a): a is AgentManifestPlan =>
              a !== null &&
              typeof a === "object" &&
              typeof (a as AgentManifestPlan).name === "string" &&
              typeof (a as AgentManifestPlan).soul === "string",
          );
        } catch {
          return [];
        }
      }

      // ───────────────────────────────────────────────────────────
      // FID-017 — Inner monologue mock cases
      // ───────────────────────────────────────────────────────────

      // MOCK_REFLECTIONS_KEY is declared at module scope (see the
      // top of this file, near LS_SWARM_BASELINE) so it's accessible
      // to both this write path AND the read path in
      // useReflections (same localStorage key, single source of truth).

      case "initialize_app_state": {
        // noop — AppState is initialized at startup in Tauri runtime;
        // browser preview has no concept of "startup" for this.
        return null;
      }

      case "start_consciousness": {
        // Cycle THINKING -> IDLE -> DORMANT -> WONDERING every 5s.
        // (Browser preview — no actual daemon, just visual feedback.)
        const state = (globalThis as { __savantConsciousness?: {
          intervalId: number | null;
          current: "THINKING" | "IDLE" | "DORMANT" | "WONDERING";
          cycleIndex: number;
        } }).__savantConsciousness ?? {
          intervalId: null,
          current: "IDLE" as const,
          cycleIndex: 0,
        };
        if (state.intervalId !== null) {
          (globalThis as { __savantConsciousness?: typeof state }).__savantConsciousness = state;
          return state.current;
        }
        state.intervalId = window.setInterval(() => {
          state.cycleIndex = (state.cycleIndex + 1) % 4;
          state.current = (["THINKING", "IDLE", "DORMANT", "WONDERING"] as const)[
            state.cycleIndex
          ];
        }, 5000);
        state.current = "THINKING";
        state.cycleIndex = 0;
        (globalThis as { __savantConsciousness?: typeof state }).__savantConsciousness = state;
        return "THINKING";
      }

      case "stop_consciousness": {
        const state = (globalThis as { __savantConsciousness?: {
          intervalId: number | null;
          current: "THINKING" | "IDLE" | "DORMANT" | "WONDERING";
          cycleIndex: number;
        } }).__savantConsciousness;
        if (state?.intervalId !== null && state?.intervalId !== undefined) {
          window.clearInterval(state.intervalId);
        }
        if (state) {
          state.intervalId = null;
          state.current = "IDLE";
        }
        return null;
      }

      case "get_consciousness_state": {
        const state = (globalThis as { __savantConsciousness?: {
          intervalId: number | null;
          current: "THINKING" | "IDLE" | "DORMANT" | "WONDERING";
          cycleIndex: number;
        } }).__savantConsciousness;
        return state?.current ?? "IDLE";
      }

      case "trigger_reflection": {
        const override = typeof args.lensOverride === "string" ? args.lensOverride : null;
        // Module-scoped lens index (resets on HMR). The actual LENSES
        // array lives at src/lib/inner-monologue/lenses.ts (TS port of
        // crates/agent/src/pulse/prompts.rs::LENSES). 19 entries.
        const idxKey = "savant.monologue.lensIndex";
        const currentIdx = Number(window.localStorage.getItem(idxKey) ?? "0");
        // Pick the lens — overridden or rotated through the 19-entry
        // LENSES array (NOT a hardcoded EMERGENCE). Both branches
        // return the same `readonly [string, string]` tuple shape so
        // `lens[0]` / `lens[1]` work uniformly below (TS7053 would
        // fire if the override branch produced `{ name, prompt }`
        // instead — tuples don't accept object indexing and vice versa).
        const lens: readonly [string, string] = override
          ? [override, LENSES.find((l) => l[0] === override)?.[1] ?? `[lens: ${override}]`]
          : LENSES[currentIdx % LENSES.length];
        const nextIdx = (currentIdx + 1) % LENSES.length;
        window.localStorage.setItem(idxKey, String(nextIdx));
        // Build the prompt and call OpenRouter (real HTTP, mirrors
        // manifest_soul pattern).
        const masterKey = effectiveMasterKey("openrouter");
        if (!masterKey) {
          return `[mock reflection — no master key] Lens: ${lens[0]}. Set up your OpenRouter master key in Settings to enable real reflections.`;
        }
        // Capture the active key source + length for 401 diagnostics.
        // Knowing WHICH tier is active (env var vs vault) is the first
        // step to fixing an auth failure — and the 401 from OpenRouter
        // is the same either way (they reject the key, not the source).
        const keySource = _envMasterKey
          ? "env"
          : mockMasters["openrouter"]
            ? "vault"
            : "none";
        const keyLength = masterKey.length;
        // Model is sourced from the user's saved config (set via
        // Settings page). The page passes it explicitly in args; we
        // also fall back to mockConfig (set by save_config) as a
        // defense-in-depth check. NO hardcoded default — the Settings
        // page is the only way to set a model, and if neither source
        // is set we surface a clear error pointing the user there.
        // The whole point of Settings is the user picks the model;
        // we never override that choice with a random default.
        // Throws (not returns) so the page's catch block routes this
        // to the red error banner — a returned string would otherwise
        // render in the live-reflection card as if it were a real
        // reflection, hiding the actual problem from the user.
        const model = String(args.model ?? mockConfig?.modelId ?? "");
        if (!model) {
          throw new Error(
            `No model configured. Set your model in Settings → OpenRouter before triggering reflections. (Lens: ${lens[0]})`,
          );
        }
        const response$ = (async (): Promise<string> => {
          const response = await fetch(OPENROUTER_CHAT_URL, {
            method: "POST",
            headers: {
              Authorization: `Bearer ${masterKey}`,
              "Content-Type": "application/json",
              "HTTP-Referer":
                typeof window !== "undefined" ? window.location.origin : "https://savant.local",
              "X-Title": "Savant Inner Monologue",
            },
            body: JSON.stringify({
              model,
              messages: [
                {
                  role: "system",
                  content: `${lens[1]}\n\nYou are Savant. Reflect on whatever comes to mind using this lens. Write freely in Markdown.`,
                },
              ],
            }),
          });
          if (!response.ok) {
            const text = await response.text();
            // 401 with "User not found" is OpenRouter's specific response
            // to an invalid or unknown API key — NOT a transient outage.
            // Surface the active source + key length so the user can
            // immediately tell WHICH tier is the problem (env var vs
            // vault) without having to grep their own config. The
            // original error from OpenRouter is included verbatim.
            if (response.status === 401) {
              throw new Error(
                `OpenRouter rejected the API key (401 User not found). ` +
                `Active source: ${keySource} (length: ${keyLength}). ` +
                `If source is "env", check your OPENROUTER_MASTER_KEY env var (.env or shell). ` +
                `If source is "vault", update via Settings → OpenRouter Master Key. ` +
                `Env var shadows the vault when set. ` +
                `Original: ${text.slice(0, 120)}`,
              );
            }
            throw new Error(
              `OpenRouter /v1/chat/completions ${response.status}: ${text.slice(0, 200)}`,
            );
          }
          const parsed = (await response.json()) as {
            choices?: Array<{ message?: { content?: string } }>;
          };
          const narrative = parsed.choices?.[0]?.message?.content ?? "(empty reflection)";
          // Persist to localStorage as a REFLECTIONS.md stand-in.
          // NO lens tag — the consciousness stream is a single continuous
          // journal, not a per-lens partition (FID-017 correction 2026-07-13
          // per Spencer: "all lenses are supposed to be a single stream, not
          // separated by lenses but all joined together"). The lens is used
          // internally to pick the LLM prompt angle; the output narrative
          // is one thread of consciousness with no per-entry lens tag.
          const ts = new Date().toISOString();
          const existing = JSON.parse(
            window.localStorage.getItem(MOCK_REFLECTIONS_KEY) ?? "[]",
          ) as Array<{ ts: string; content: string }>;
          existing.unshift({ ts, content: narrative });
          window.localStorage.setItem(
            MOCK_REFLECTIONS_KEY,
            JSON.stringify(existing.slice(0, 100)),
          );
          return narrative;
        })();
        return response$;
      }

      // ───────────────────────────────────────────────────────────
      // FID-028 — Scaffold 3 placeholder pages (changelog / faq / tune).
      // The changelog case was REMOVED in the Spencer correction
      // pass (2026-07-14) — the changelog now comes from GitHub via
      // a client-side fetch in `src/lib/changelog.ts`, so no mock
      // layer is needed (the IPC wrapper `getChangelog()` calls
      // `fetchChangelog()` directly).
      // ───────────────────────────────────────────────────────────

      // get_parameter_descriptors — returns the LlmParams descriptor
      // list (matches the gateway's `/api/models` response body).
      // All 9 entries: 4 sampling knobs + 5 model-selection fields.
      // KEPT for the future Settings-page wiring (the Settings page
      // will use the full list to render model + provider + URL
      // fields).
      case "get_parameter_descriptors": {
        return getParameterDescriptors();
      }

      // get_tuning_descriptors — returns the 4 TRUE tuning parameter
      // descriptors (filtered from get_parameter_descriptors). The
      // Tune page uses this; the 5 model-selection fields (chat_model,
      // manifestation_model, vision_model, provider, ollama_url) are
      // configured on the Settings page instead.
      // Spencer revision 2026-07-14: "Fine tuning is for the actual
      // model it's self, this page is not to change models."
      case "get_tuning_descriptors": {
        return getTuningDescriptors();
      }

      // save_settings — persists the 4 tuning parameter values
      // (temperature, top_p, frequency_penalty, presence_penalty).
      // Browser-preview mock: stores in module-scoped state + returns
      // success. The Tauri runtime would route through the gateway's
      // POST /api/settings endpoint (which clamps to the OpenAI
      // ranges before persisting). The localStorage write happens
      // in the TUNE PAGE (not here) — it's a UI concern, not an IPC
      // concern; the mock IPC just acknowledges the dispatch.
      case "save_settings": {
        const rawValues = (args.values as Record<string, unknown>) ?? {};
        const sanitized: Record<string, number> = {};
        for (const [k, v] of Object.entries(rawValues)) {
          if (typeof v === "number" && Number.isFinite(v)) {
            sanitized[k] = v;
          }
        }
        mockTuningValues = sanitized;
        return { ok: true };
      }

      // get_faq — returns the curated FAQ Q&A list. No real FAQ
      // source in the savant-orig (see
      // `src/lib/faq-data.ts` §Future FID candidate comment).
      case "get_faq": {
        return getFaqItems();
      }
    case "list_chat_sessions":
      return sortedSessions();
    case "load_chat_history":
      return loadChatHistoryMock(args.sessionId as string, args.limit as number | 0);    case "persist_chat_turn":
      // TS2345 narrowing: `args` is typed as Record<string, unknown>
      // (see cast at top of switch). The renderer-side producer
      // (`use-chat-history.ts::sendTurn`) constructs full ChatMessage
      // objects, so this assertion is safe at the mock layer. The
      // DAEMON path (FID-032 Layer 3) will validate the shape via
      // serde at the IPC boundary, eliminating the helper-side cast.
      return persistTurnMock(
        args.sessionId as string,
        args.userMessage as ChatMessage,
        args.assistantMessage as ChatMessage,
      );
    case "delete_chat_session":
      return deleteSessionMock(args.sessionId as string);
    case "search_chat_history":
      return searchHistoryMock(args.query as string, args.limit as number | 0);

      default:
        throw new Error(`Mock IPC: unknown command "${cmd}"`);
    }
  });

  // One-time install log. Helpful for dev (confirms the mock IPC
  // is active in browser preview mode vs the real Tauri runtime
  // in `cargo tauri dev`).
  logger.info(
    "Tauri mock IPC installed (browser preview mode). Run `cargo tauri dev` for real IPC.",
  );
}
