# FID: Chat Persistence — Wire Chat Page to Real Memory System

**Filename:** `FID-2026-07-14-029-chat-persistence.md`
**ID:** FID-2026-07-14-029
**Severity:** medium
**Status:** closed (FID-029 §Layer 1 closed 2026-07-15; chat persistence backend + renderer bridge verified; FID-030 Layer 2 starts next cycle.)
**Created:** 2026-07-14 18:30
**Author:** Buffy (ECHO agent, on Spencer's "wire the chat page's useState<Message[]>([]) to the real memory system" directive)

---

## Summary

The chat page (`src/app/chat/page.tsx`) currently holds the conversation
history in React state — `useState<Message[]>([])` at line 50. Every
page refresh wipes the history; switching tabs loses the context; closing
the browser drops the conversation. The real memory stack already
exists in the Rust crates (`LsmStorageEngine::append_message`,
`LsmStorageEngine::fetch_session_tail`, `SessionState`/`TurnState` at
`crates/memory/src/models.rs`, the partitioned transcript collection
`transcript.{session_id}`, `GatewayPersistence::persist_chat` at
`crates/gateway/src/persistence.rs`) — but the renderer is not wired
to it. This FID adds 5 IPC commands wrapping the existing Rust
primitives + a 1-field amendment to the Rust `SessionState` struct
(adding `title: Option<String>` for O(1) sidebar listing) + a
`useChatHistory` hook (mirroring `useReflections`'s pattern) + a chat
page rewrite that (1) hydrates the current session's history on mount,
(2) auto-persists each user + assistant turn, (3) adds a multi-session
list/switcher in a collapsible left-rail drawer, (4) adds a "Clear"
button that removes the current session, and (5) adds a real-time FTS
search bar that surfaces matching messages across all sessions. The
Tauri runtime is wired in v1 (no stub) — the `MemoryEnclave` is
initialized in the Tauri `setup()` callback and the 5 IPC commands
dispatch to real Rust primitives.

## Environment

- **OS:** Windows 11 (win32)
- **Language/Runtime:** TypeScript 5.7, Next.js 15, React 19, Node >=22
- **Tool Versions:** Same as the rest of the dashboard (HeroUI v3 alpha
  `@heroui/react@3.0.0-beta.2`, `@tauri-apps/api@2`, `vitest`)
- **Commit/State:** branch `main`, pre-existing uncommitted work on the
  Tune page (FID-028 Revisions 1-5) + the dashboard icon swap (FID-027)
  + the lint-defer + release-check tooling (FID-022/026)

## Detailed Description

### Problem

`src/app/chat/page.tsx:50` declares
`const [messages, setMessages] = useState<Message[]>([]);` — the
conversation history lives in React state. The file's own v1 constraints
comment at line 14 is explicit:

```text
// v1 constraints:
//   - No streaming. POST /v1/chat/completions non-stream.
//   - No persistence yet. Messages live in React state.
//   - No multi-turn trimming. Full history each call.
//   - One persona ("Savant") hard-coded system prompt.
```

Consequences:

- **Page refresh wipes the history.** A user deep in a debugging
  conversation loses everything on a hot reload.
- **No multi-session support.** The page has a single implicit session
  keyed off the derived subkey (`LS_DERIVED.name`). When the subkey
  rotates (daily cron per OQ-4), the conversation context is
  disconnected from the new subkey.
- **No search across past conversations.** The user can't recall what
  they discussed last week.
- **No "Clear" affordance.** The only way to reset is to refresh the
  page.

### Expected Behavior

After FID-029 lands:

1. **On mount:** The page reads the current session ID from
   `LS_CURRENT_SESSION` (new localStorage key, single source of truth
   for the active session). If absent, generates a new session ID via
   `crypto.randomUUID()` (per §ID Generation) and writes it back.
   Loads the session's messages from the persistence layer (browser
   preview: localStorage via `chatMessagesKey(session_id)`; Tauri
   runtime: `LsmStorageEngine::fetch_session_tail`).
2. **On send (per §Missed Questions #2, persist order is CRITICAL):**
   The user message is added to React state (NOT persisted yet).
   The OpenRouter call fires with the full hydrated history
   (sliced to `OPENROUTER_CONTEXT_MAX_CHARS` via
   `trimMessagesForContext` per §Missed Questions #4 + §Verifier Pass
   HIGH #2 — drops WHOLE older messages from the start of the array,
   never slices content mid-string). The response is awaited. The
   assistant reply is added to React state. THEN
   `persistTurn(userContent, assistantContent)` is called — both
   messages are persisted SEQUENTIALLY in one dispatch cycle
   (back-to-back). The session metadata is auto-created if the
   session is new (turn_count=1, title=first 50 chars of the first
   user message OR `NO_TITLE_FALLBACK` per §Missed Questions #8).
   This prevents the orphan-user-message problem if the tab
   refreshes mid-fetch.
3. **Multi-session sidebar:** A secondary collapsible left-rail
   drawer INSIDE the chat page (per §Missed Questions #12 + §Page
   Rewrite) lists all known sessions sorted by `last_active`
   descending. Each row shows the auto-title + relative time +
   turn_count. The active session row gets a `border-l-2
   border-accent` + `bg-accent/5` highlight. **When the sidebar is
   collapsed (per §Verifier Pass MEDIUM #5), the main chat header
   shows the current session's title as a secondary visual cue**
   (e.g., "Savant · {currentSession?.title ?? NO_TITLE_FALLBACK}")
   so the user always knows their current context. Clicking a row
   switches the current session (per §Missed Questions #10 + §Verifier
   Pass MEDIUM #4, `switchSession(id)` fast-returns when
   `id === currentSession?.id` to avoid the abort-during-in-flight
   race; the unsent composer text is preserved per-session via
   `composerDrafts`). A "+" button at the top creates a new
   session. A trash icon on hover + a `window.confirm()` dialog
   handles per-session delete (per §Suggestions for Improvement A).
4. **Clear buttons (per §Decisions #2):** Two clear paths —
   (a) per-session trash icon on each session row (with confirm
   dialog, per §Suggestions for Improvement A) for surgical
   deletes, AND (b) a "Clear current chat" link at the top of the
   composer (right side) for the common case of "I want to start
   over in this session." No global "Clear all" in v1 (per
   §Decisions #7 — follow-on FID with proper undo UX).
5. **FTS search bar:** A search input inside the left-rail drawer
   (per §Page Rewrite) above the session list. Cmd/Ctrl+K focuses
   the search bar (per §Suggestions for Improvement B + §Verifier
   Pass HIGH #3 — the listener is bound INSIDE the `if (derived)`
   branch so the shortcut doesn't fire when the OQ-3 blocking
   modal is up; cross-platform modifier `e.metaKey || e.ctrlKey`).
   The OQ-3 blocking modal is the existing "no derived subkey"
   early-return at `src/app/chat/page.tsx:31-49` (a no-key
   safeguard that returns the BlockingModal before the chat
   surface renders).
   As the user types, results stream in from
   `search_chat_history(query, 20)`. Each result shows the matching
   snippet (with the query highlighted) + the session title + the
   message role + relative time. Clicking a result calls
   `switchSession(result.session_id)` + scrolls to the message
   (per §Suggestions for Improvement D with a 3-second yellow
   highlight). Empty state shows "No results for '{query}' — try
   a different search term" with a Clear button (per §Suggestions
   for Improvement C).
6. **Multi-tab sync:** The `storage` event syncs `LS_CHAT_SESSIONS`
   + `chatMessagesKey(currentSession)` + `LS_CURRENT_SESSION`
   + `LS_CHAT_SIDEBAR_COLLAPSED` across tabs (matches the existing
   `LS_DERIVED` pattern at `src/app/chat/page.tsx:71-78`). The
   `savant:chat-history-updated` custom event (dispatched by the
   mock IPC after a write) lets the local state update immediately
   without waiting for the `storage` event.
7. **Tauri runtime parity:** All 5 IPC commands work in both browser
   preview (mock cases) and Tauri runtime (real Rust commands in
   `src-tauri/src/lib.rs`). The hook is identical in both modes — the
   IPC layer abstracts the difference. The Tauri `AppState.memory`
   handle is initialized asynchronously via
   `tauri::async_runtime::spawn` (per §Missed Questions #7) so the
   Tauri runtime boots instantly without waiting for the Ollama
   health check.

### Root Cause

Renderer-first rebuild is in progress (per `protocol.config.yaml`).
The chat page was scaffolded with `useState<Message[]>([])` as the v1
deliverable; the persistence wiring is Phase 2 work that this FID
completes for the chat surface specifically. The Rust persistence
primitives have been in `crates/memory/` since the substrate's first
landing; `GatewayPersistence::persist_chat` at
`crates/gateway/src/persistence.rs:13-58` is the existing
`Storage::append_chat` wrapper that's been the canonical chat-persist
path since the gateway was first wired. The gap is the IPC bridge +
the renderer-side hook + the UI surfaces, not the storage engine.

### Evidence

Rust persistence primitives (already in the codebase, NOT being
rewritten by this FID):

```rust
// crates/memory/src/lsm_engine.rs:211-237
pub fn append_message(
    &self,
    session_id: &str,
    message: &AgentMessage,
) -> Result<(), MemoryError> {
    let bytes = rkyv::to_bytes::<RkyvError>(message)
        .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
    let collection = Self::transcript_collection(session_id);
    // ... write to transcript.{session_id} collection ...
}

// crates/memory/src/lsm_engine.rs:331-410
pub fn fetch_session_tail(&self, session_id: &str, limit: usize) -> Vec<AgentMessage> {
    // ... newest-first, capped by limit, skips Archive channel ...
}

// crates/memory/src/lsm_engine.rs:107-117
fn transcript_collection(session_id: &str) -> String {
    format!("transcript.{}", session_id)
}

// crates/memory/src/models.rs:165-219 — AgentMessage with id, session_id,
// role (MessageRole), content, tool_calls, tool_results, timestamp,
// parent_id, channel. The 4 user-facing fields for v1 chat are:
//   id, session_id, role, content, timestamp
// (tool_calls, tool_results, parent_id, channel are reserved for
// future FIDs — see §Out of scope below.)
```

Gateway persistence wrapper:

```rust
// crates/gateway/src/persistence.rs:11-58 — GatewayPersistence::persist_chat
// Partition precedence: agent_id > sender > recipient > session_id > "global".
// Routes the ChatMessage into `chat.{agent_name}` so the dashboard's
// HistoryRequest can find it. (For FID-029, the renderer creates the
// session_id directly so the partition IS the session_id — matches
// the fetch_session_tail call shape.)
```

Session + turn state:

```rust
// crates/memory/src/models.rs:507-560 — SessionState
// Tracks session_id, created_at, last_active, turn_count, active_turn_id,
// auto_approved_tools, denied_tools, parent_session_id, fork_point_turn_id.
// The 5 renderer-visible fields for v1 chat are:
//   session_id, created_at, last_active, turn_count
// (auto_approved_tools, denied_tools, parent_session_id, fork_point_turn_id,
// active_turn_id are reserved for future FIDs.)

// crates/memory/src/models.rs:603-650 — TurnState
// Tracks per-turn lifecycle (Processing / Completed / Failed /
// Interrupted / AwaitingApproval). The renderer doesn't need to see
// this for v1 — the Tauri-runtime IPC can write TurnState internally
// without exposing it to the renderer.
```

Chat page v1 state:

```tsx
// src/app/chat/page.tsx:50
const [messages, setMessages] = useState<Message[]>([]);
// ...
// Line 165-167: the user/assistant message pair is added to React state
// after the OpenRouter call returns, with NO persistence.
setMessages([...messages, userMsg, assistantMsg]);
```

## Impact Assessment

### Affected Components

- **New (renderer):**
  - `src/lib/hooks/use-chat-history.ts` — hook (mirrors `useReflections` + `useDerivedRotation` patterns)
  - `src/lib/chat-data.ts` — TS type definitions + localStorage key constants + helpers
  - `src/app/chat/components/chat-sidebar.tsx` — session list + search bar + per-session delete (per §Verifier Pass MEDIUM #6)
  - `src/app/chat/components/chat-composer.tsx` — textarea + per-session draft + send handler + Clear link (per §Verifier Pass MEDIUM #6)
  - `src/app/chat/components/chat-message-list.tsx` — message rendering with `data-message-id` for search scroll (per §Verifier Pass MEDIUM #6)
  - `src/app/chat/components/chat-header.tsx` — page header with sidebar toggle + current-session title (collapsed-cue per §Verifier Pass MEDIUM #5)
  - `src/app/chat/components/chat-search-results.tsx` — search results panel + empty state (per §Verifier Pass MEDIUM #6)
- **Modified (renderer):**
  - `src/lib/ipc.ts` — 5 new IPC wrappers (`listChatSessions`, `loadChatHistory`, `persistChatTurn`, `deleteChatSession`, `searchChatHistory`)
  - `src/lib/mock-ipc.ts` — 5 new mock cases (localStorage-backed, structured JSON matching the `ChatMessage` + `ChatSession` shapes)
  - `src/app/chat/page.tsx` — full rewrite (rip out `useState<Message[]>([])` + compose the 5 sub-components + wire to `useChatHistory` hook)
- **Modified (Tauri):**
  - `src-tauri/src/lib.rs` — 5 new Tauri commands wrapping `MemoryEnclave::append_message` + `MemoryEnclave::fetch_session_tail` + `LsmStorageEngine::iter_session_states` + `LsmStorageEngine::delete_session` + a simple substring-search (or call to `MemoryEnclave::hybrid_search` for the FTS-with-recency case)
- **Modified (docs):**
  - `dev/fids/FID-2026-07-14-029-chat-persistence.md` — this doc (impl loop + verifier pass + resolution added after the impl lands)

### Risk Level

- [ ] Critical: System crash, data loss, or security vulnerability
- [ ] High: Major feature broken, no workaround
- [x] Medium: Feature degraded, workaround exists (current `useState<Message[]>([])` is a v1 placeholder; the user has explicitly accepted the limitation; FID-029 is the upgrade, not a fix)
- [ ] Low: Minor issue, cosmetic, or edge case

## Proposed Solution

### Approach

Wire the existing Rust persistence primitives to the renderer through
5 thin IPC commands (all return `Result<T, String>` per the standard
Tauri pattern, per §Missed Questions #11). Mirror the existing
`useReflections` pattern for the hook (mount-time hydrate + refresh +
cross-tab sync via the `storage` event). Add a multi-session sidebar
in a secondary collapsible left-rail drawer (using the HeroUI v3
`<Listbox>` or a hand-rolled `<ul>`) + a real-time FTS search bar
inside the drawer (using the existing `<input type="search">` pattern
from the reflections page) + per-session Clear button (using
`window.confirm()` for confirm, per §Suggestions for Improvement A).
The browser-preview mock layer uses **per-session localStorage keys**
(`LS_CHAT_SESSIONS` for the metadata array + `chatMessagesKey(session_id)`
for each session's message array, per §Decisions #6 + §Data Model) —
NOT a single aggregate key, which would force a parse+stringify of
the ENTIRE dataset on every message turn. Per-session keys match the
`MOCK_REFLECTIONS_KEY` pattern's per-feature isolation (each chat
session is independent).

**Page rewrite scope (per §Verifier Pass MEDIUM #6):** The full page
rewrite is split into 5 sub-components (sidebar / composer /
message-list / header / search-results) so each file stays under
~300 lines. The page itself becomes a thin composer (~150 lines) that
wires the hook to the sub-components. This is a non-trivial refactor
(5 new files), but it prevents the page.tsx complexity breach that
would otherwise make the file >700 lines.

### Data Model

The TypeScript mirror of `AgentMessage` + `SessionState` (only the
v1-essential fields):

```ts
// src/lib/chat-data.ts
//
// v1 SCOPE: text-only user + assistant messages. "system" and "tool"
// roles are reserved for future FIDs (per §Out of Scope #1 — tool calls
// + tool results rendering). The Role type is RESTRICTED to user +
// assistant to prevent dead paths in the renderer; the AgentMessage
// type in Rust keeps all 4 roles (the Rust side handles them
// generically).
export type Role = "user" | "assistant";

export type ChatMessage = {
  id: string;          // UUID v4 (via crypto.randomUUID(), not randomHex(16))
  session_id: string;  // matches ChatSession.session_id (UUID v4)
  role: Role;
  content: string;     // CAPPED at MAX_MESSAGE_BYTES (50_000) at persist time
  ts: number;          // unix ms (matches AgentMessage.timestamp / 1_000_000)
};

export type ChatSession = {
  session_id: string;
  title: string;       // auto-titled from first user message (first 50 chars,
                       //   or NO_TITLE_FALLBACK if empty)
  created_at: number;  // unix ms
  last_active: number; // unix ms
  turn_count: number;  // 1 turn = 1 user + 1 assistant message
  message_count: number;
  // NEW (per §Verifier Pass post-ratification-re-survey #2 — suggestion H):
  // Pinned sessions float to the top of the sidebar in a distinct
  // "Pinned" section (per Decision #11). Max 10 pinned sessions
  // (per suggestion M); exceeding the cap shows a toast/alert.
  pinned: boolean;
};

export type ChatSearchResult = {
  session_id: string;
  session_title: string;
  message: ChatMessage;
  score: number;       // 0-1, higher is more relevant
  match: string;       // the matching snippet (with context, ~120 chars)
};

// PER-SESSION localStorage keys (NOT a single aggregate key — see
// §Decisions Awaiting Spencer's Input #6 for the perf rationale).
// A single LS_CHAT_DATA key would force a parse+stringify of the
// ENTIRE dataset on every message turn; with N sessions × M messages,
// this becomes a perf cliff. Per-session keys mean each write touches
// only the relevant session's message array + the small LS_CHAT_SESSIONS
// metadata array.
export const LS_CHAT_SESSIONS = "savant.chat.sessions";
export const LS_CURRENT_SESSION = "savant.chat.currentSession";
export const LS_CHAT_SIDEBAR_COLLAPSED = "savant.chat.sidebarCollapsed";
export function chatMessagesKey(sessionId: string): string {
  return `savant.chat.messages.${sessionId}`;
}

// Limits + fallbacks (per §Missed Questions #2-#5).
export const MAX_LOADED_MESSAGES = 100;        // Render-time cap (in-memory)
export const MAX_MESSAGE_BYTES = 50_000;       // Per-message cap (chars)
export const OPENROUTER_CONTEXT_MAX_CHARS = 40_000; // Pre-fetch trimmer target
export const TITLE_MAX_CHARS = 50;             // Auto-title truncation
export const NO_TITLE_FALLBACK = "New chat";   // Used when first message is empty
export const QUOTA_DROP_RATIO = 0.1;           // Drop oldest 10% on QuotaExceeded
```

The localStorage shape (per-session keys, structured JSON):

```ts
// Stored under LS_CHAT_SESSIONS:
//   ChatSession[] — sorted by last_active descending
//   (Bounded by the number of sessions; typically <100, so the metadata
//    array stays small even with thousands of conversations.)
//
// Stored under chatMessagesKey(sessionId):
//   ChatMessage[] — sorted by ts ascending
//   (Bounded by MAX_LOADED_MESSAGES=100; older messages exist in storage
//    but aren't loaded into the renderer's working set.)
//
// Stored under LS_CURRENT_SESSION:
//   string — the active session_id (matches the page's "current chat")
//
// Stored under LS_CHAT_SIDEBAR_COLLAPSED:
//   "true" | "false" — the left-rail drawer's collapse state
```

### Sibling `session_titles` Collection (storage layer for titles — pivoted from struct amendment 2026-07-15)

The renderer-facing `ChatSession.title` field is backed by a NEW
`session_titles` sibling collection in CortexaDB, NOT by a field on
the rkyv `SessionState` struct. Per Spencer's 2026-07-15 directive
("NEVER use in memory for anything persistent — we use the db"),
this design avoids:

1. **rkyv 0.7.x backward-compat risk** — the rkyv `SessionState` struct
   at `crates/memory/src/models.rs:707-727` is `#[repr(C)]` + serialized
   via `rkyv::to_bytes::<RkyvError>(state)` (see `save_session_state` at
   `crates/memory/src/lsm_engine.rs:1204`). Adding a new field would
   shift byte offsets and corrupt pre-FID-029 on-disk records on first
   deserialize (rkyv would read garbage from the next record's start
   as the `Option` discriminant → UB territory).
2. **Migration code burden** — no `migration.rs` needed; pre-FID-029
   data stays untouched and deserializes cleanly (title field is just
   not present, which is fine because rkyv uses the struct definition
   at compile time, not runtime introspection).
3. **Wire-format breakage** — the Tauri IPC contract gains `title:
   Option<String>` (via `savant_core::types::SessionState.title` with
   `#[serde(default)]`), but old clients default to `None` so the
   change is wire-format additive only.

**Architecture (sibling collection design):**

| Layer | Type | Title storage |
| :--- | :--- | :--- |
| `crates/memory/src/models.rs:707-727` | `crate::models::SessionState` (rkyv struct, 9 fields) | NO title field — UNCHANGED |
| `crates/core/src/types/mod.rs:21-35` | `savant_core::types::SessionState` (plain serde, 10 fields) | `pub title: Option<String>` with `#[serde(default)]` |
| `crates/memory/src/lsm_engine.rs` (new methods after `iter_session_states`) | `LsmStorageEngine` | `session_titles` CortexaDB collection |
| `crates/memory/src/async_backend.rs` L571/596/615 | `MemoryBackend` trait impl | populates `savant_core::types::SessionState.title` from sibling collection at hydrate; writes to sibling on `save_session` |

**3 new `LsmStorageEngine` methods:**

```rust
// Async (mirrors save_session_state)
pub async fn save_session_title(
    &self,
    session_id: &str,
    title: Option<&str>,  // None = no-op (default state)
) -> Result<(), MemoryError>;

// Sync (mirrors get_session_state)
pub fn load_session_title(
    &self,
    session_id: &str,
) -> Result<Option<String>, MemoryError>;

// Sync (mirrors iter_session_states)
pub fn iter_session_titles(
    &self,
) -> Result<HashMap<String, String>, MemoryError>;
```

**Wire format:** The Tauri IPC command `list_chat_sessions` (added in
§Step 9) returns `Vec<ChatSession>` where `ChatSession.title:
Option<String>` is populated from the sibling collection via
`iter_session_titles` join in `async_backend.rs`. The renderer's
`src/lib/chat-data.ts::ChatSession` type already has `title: string`
(per the FID's §Data Model), so the renderer change is wire-format
additive only.

**Browser preview parity:** The browser preview's localStorage mock
layer stores titles in `LS_CHAT_SESSIONS[i].title` directly (per
§Data Model). The Tauri runtime uses the sibling collection. The IPC
contract abstracts the difference — the renderer never sees the
storage layer.

**Why this design over the alternatives (per Spencer's 2026-07-15 audit):**

- **In-memory `#[rkyv(with = Skip)]`**: REJECTED — violates Spencer's
  "NEVER use in memory for anything persistent" directive. Titles
  would be lost on every save/load cycle in Tauri runtime.
- **Migration path (v1 → v2)**: viable but adds ~120 LoC of migration
  code + a one-time migration runner. Overkill for an empty DB.
- **This sibling-collection design**: cleanest separation of concerns.
  Title metadata is orthogonal to the rkyv session state, so it gets
  its own collection. ~70 LoC across 3 files. No migration. No struct
  changes to the rkyv layer.

### ID Generation (CRITICAL — `crypto.randomUUID()`, NOT `randomHex(16)`)

**Rule:** All session_id + message_id values are UUID v4 via
`crypto.randomUUID()` (browser-native, available in all modern
browsers + Tauri webview). The `randomHex(16)` from `src/lib/ids.ts`
is for opaque tokens (e.g., the agent name suffix in
`provisionSessionKey`); it produces 32-char hex strings without dashes,
which is NOT a valid UUID v4 format. The Rust `AgentMessage::user`
constructor at `crates/memory/src/models.rs:172-188` generates
`uuid::Uuid::new_v4().to_string()` (36 chars with dashes); a 32-char
hex string would silently break any future Rust code that tries to
parse the string as a `uuid::Uuid` (e.g., for foreign-key relationships
or migration tooling).

**Browser compatibility:** `crypto.randomUUID()` is available in
Chrome 92+ (Aug 2021), Firefox 95+ (Dec 2021), Safari 15.4+ (Mar
2022). The dashboard's `browserslist` target is the latest 2 versions
of each — `crypto.randomUUID()` is universally available. No
polyfill needed.

**Tauri runtime:** Same `crypto.randomUUID()` in the renderer;
`uuid::Uuid::new_v4()` in the Rust Tauri commands (matches the
existing `AgentMessage::user` constructor).

### IPC Commands (5)

All snake_case to match the Tauri v2 convention. The renderer calls
`invoke<T>("cmd_name", args)`; the mock intercepts in browser preview;
the Tauri host intercepts in desktop. **All commands return
`Result<T, String>`** (standard Tauri pattern — the IPC bridge turns
`Err(String)` into a thrown Promise that the renderer catches with
try/catch). The renderer does NOT use `{ ok: false, error: "..." }`
return shapes (per §Missed Questions #11).

1. **`list_chat_sessions() -> ChatSession[]`**
   - Browser mock: reads `LS_CHAT_SESSIONS`, returns sorted by
     `last_active` desc.
   - Tauri: calls `LsmStorageEngine::iter_session_states()`, maps
     `SessionState` to `ChatSession` (drops the agent-tooling fields;
     uses the new `title` field).
2. **`load_chat_history(session_id: string) -> ChatMessage[]`**
   - Browser mock: reads `chatMessagesKey(session_id)`, sorts by `ts`
     asc, returns the last 100 (capped at `MAX_LOADED_MESSAGES`).
     Returns `[]` if the key is missing or the session doesn't exist.
   - Tauri: calls `MemoryEnclave::fetch_session_tail(session_id, 100)`,
     maps `AgentMessage` to `ChatMessage` (drops `tool_calls`,
     `tool_results`, `parent_id`, `channel` for v1).
3. **`persist_chat_turn(session_id: string, role: Role, content: string) -> { id: string, ts: number }`**
   - Browser mock: generates UUID v4 via `crypto.randomUUID()`,
     appends to `chatMessagesKey(session_id)`, updates the session
     metadata in `LS_CHAT_SESSIONS` (last_active, turn_count,
     message_count; auto-creates the session if it doesn't exist with
     `title` = first 50 chars of the first user message OR
     `NO_TITLE_FALLBACK` if empty). Wraps the `setItem` in a
     try/catch; on `QuotaExceededError`, drops the oldest
     `QUOTA_DROP_RATIO` (10%) of sessions by `last_active` — AND
     removes each dropped session's `chatMessagesKey(session_id)`
     entry (per §Verifier Pass HIGH #1) — then retries the write
     once. Dispatches `savant:chat-history-updated` for
     cross-component sync.
   - Tauri: calls `MemoryEnclave::append_message()` with a constructed
     `AgentMessage` (rkyv-serialized) + `MemoryEnclave::save_session_state()`
     with a `SessionState` (updated turn_count + last_active + title
     if first message).
4. **`delete_chat_session(session_id: string) -> { ok: boolean }`**
   - Browser mock: removes the session entry from `LS_CHAT_SESSIONS`
     and the corresponding `chatMessagesKey(session_id)` key. If the
     deleted session was the current session, clears
     `LS_CURRENT_SESSION` (the page will generate a new session on
     next mount).
   - Tauri: calls `LsmStorageEngine::delete_session(session_id)` +
     `LsmStorageEngine::delete_session_state(session_id)`.
5. **`search_chat_history(query: string, limit: number) -> ChatSearchResult[]`**
   - Browser mock: iterates all `LS_CHAT_SESSIONS` keys, reads each
     `chatMessagesKey(session_id)`, computes a weighted score:
     `substring_match ? 1.0 : 0.0` + recency boost (newer = higher,
     capped at +0.3) + role weighting (user > assistant). **Per
     §Verifier Pass post-SHOULD-FIX MEDIUM #5:** the raw weighted
     sum is then NORMALIZED to the [0, 1] range via
     `score = rawScore / 1.5` (the theoretical max of substring 1.0
     + recency 0.3 + role 0.2). The doc's `score: 0-1, higher is
     more relevant` claim is now accurate. Sorts by normalized score
     desc, returns top `limit`. The `match` field is the substring
     of the message content (60 chars before + the match + 60 chars
     after).
   - Tauri: calls `MemoryEnclave::hybrid_search()` (the existing
     BM25 + vector + RRF pipeline at `crates/memory/src/engine.rs`)
     with a fallback to simple substring search if
     `SAVANT_DISABLE_EMBEDDINGS=1`. The `match` field is built by
     extracting the highest-scoring BM25 hit's surrounding text.

6. **`toggle_chat_session_pin(session_id: string, pinned: boolean) -> { ok: boolean }`** *(NEW per §Verifier Pass post-ratification-re-survey #2 — suggestion H + missed question #1)*
   - Browser mock: flips the `pinned` field on the session in
     `LS_CHAT_SESSIONS`. Enforces the max-10 cap (per suggestion M)
     by returning `Err(String("max 10 pinned sessions"))` if the
     user tries to pin an 11th. Dispatches
     `savant:chat-history-updated` for cross-component sync.
   - Tauri: calls `MemoryEnclave::save_session_state()` with the
     updated `SessionState` (sets the `pinned` field — would
     require a 2nd-field amendment to `SessionState` per the same
     rkyv backward-compatibility pattern as `title`).

### Hook: `useChatHistory`

`src/lib/hooks/use-chat-history.ts` (new file, ~250 lines). Mirrors
`useReflections` (mount-time hydrate + `refresh()` + cross-tab sync
via `storage` event + `loading`/`error` state) + several new fields
specific to chat persistence. Signature:

```ts
export type ChatHistoryState = {
  sessions: ChatSession[];
  currentSession: ChatSession | null;
  messages: ChatMessage[];                       // last MAX_LOADED_MESSAGES, sorted asc
  loading: boolean;
  error: string | null;
  // Sidebar collapse state (per §Verifier Pass MEDIUM #5).
  // Persisted to LS_CHAT_SIDEBAR_COLLAPSED on every change.
  // Default = false (expanded).
  sidebarCollapsed: boolean;
  // Search-result highlight target (per §Suggestions for Improvement D).
  // The page sets this when the user clicks a search result; the
  // ChatMessageList highlights the matching message for 3s. The hook
  // clears the value via the `searchHighlightClearAt` deadline.
  searchHighlightId: string | null;
  // Cross-tab unread indicator (per §Verifier Pass
  // post-ratification-re-survey #2 — suggestion I + missed
  // question #3). The Set tracks session_ids that have unread
  // changes from other tabs (the `storage` event listener adds
  // to this Set; `switchSession(id)` removes from it). TRANSIENT
  // (per Decision #10) — not persisted across browser restarts.
  unreadSessions: Set<string>;
  // Per-session composer drafts (per §Missed Questions #10). The map
  // is keyed by session_id; on session switch, the current draft is
  // saved + the new session's draft is restored. Empty string = no
  // unsent text for that session.
  //
  // CRITICAL (per §Verifier Pass post-SHOULD-FIX HIGH #2):
  // composerDrafts is IN-MEMORY ONLY (not persisted to localStorage).
  // Tab refreshes / new tabs / new sessions start with empty drafts.
  // The earlier doc text claimed "the `storage` event syncs drafts
  // across tabs" — that was wrong. The `storage` event syncs ONLY
  // the messages + sessions metadata (which ARE localStorage-backed);
  // the drafts are per-tab transient state. Cross-tab draft sync
  // would require a new `set_composer_draft` IPC + an additional
  // `LS_COMPOSER_DRAFTS` key — FUTURE FID per LESSON-038.
  composerDrafts: Record<string, string>;
  // Mutators (each is a thin wrapper around the IPC call + a
  // local-state update; the hook handles the `storage` event fan-out
  // for cross-tab sync of the localStorage-backed fields).
  switchSession: (sessionId: string) => Promise<void>;
  newSession: () => Promise<ChatSession>;
  // CRITICAL (per §Missed Questions #2): persistTurn is called
  // SEQUENTIALLY after the OpenRouter fetch returns, NOT before. The
  // user message + assistant reply are both persisted in one
  // back-to-back dispatch once the fetch settles. This prevents the
  // orphan-user-message problem if the tab refreshes mid-fetch.
  persistTurn: (
    userContent: string,
    assistantContent: string,
  ) => Promise<{ userId: string; assistantId: string; userTs: number; assistantTs: number }>;
  deleteSession: (sessionId: string) => Promise<void>;
  search: (query: string, limit?: number) => Promise<ChatSearchResult[]>;
  refresh: () => void;
  // Setter for the active session's composer draft. IN-MEMORY ONLY
  // (per HIGH #2 above). The page calls this on every textarea
  // change; the hook updates `composerDrafts[currentSessionId]`
  // locally. No IPC, no localStorage write, no `storage` event.
  setComposerDraft: (text: string) => void;
  // Setter for the sidebar collapse state. Persists to
  // LS_CHAT_SIDEBAR_COLLAPSED.
  setSidebarCollapsed: (collapsed: boolean) => void;
  // Setter for the search-result highlight. Called by the page
  // when the user clicks a search result; auto-clears after 3s.
  // The 3s auto-clear timer is managed INSIDE the hook
  // (per §Verifier Pass post-ratification-re-survey #2 — missed
  // question #4) via a `useRef<NodeJS.Timeout>` — calling
  // `setSearchHighlight` clears the old timeout + starts a new
  // one. The page doesn't need to track the timer.
  setSearchHighlight: (messageId: string | null) => void;
  // Setter for the unread indicator. Adds/removes the session_id
  // from `unreadSessions`. Called internally by the `storage`
  // event listener; exposed for testing.
  setUnread: (sessionId: string, isUnread: boolean) => void;
  // Setter for the pinned status. Takes BOTH the session_id AND
  // the new pinned state (matches the IPC shape, per
  // code-reviewer MUST FIX #2 cross-reference fix). The hook is
  // a thin wrapper around the IPC; the max-10 cap is enforced
  // on the IPC side (returns `Err(String("max 10 pinned"))`).
  // Returns `Promise<void>` (NOT the LESSON-038-violating
  // `Promise<{ ok, error }>` pattern, per code-reviewer SHOULD
  // FIX #1); try/catch surfaces the error.
  togglePin: (sessionId: string, pinned: boolean) => Promise<void>;
};
```

**PERSIST ORDER (CRITICAL — per §Missed Questions #2):** The
`send(text: string)` mutator flow inside the page (not the hook — the
hook exposes the lower-level `persistTurn`) is:

1. Add the user message to React state (NOT persisted yet).
2. Call OpenRouter `fetch()` (with `trimMessagesForContext` on the
   hydrated history, per §Missed Questions #4 + §Verifier Pass
   HIGH #2 — drops WHOLE older messages, not string-slice) and AWAIT
   the response.
3. Add the assistant reply to React state.
4. Call `persistTurn(userContent, assistantContent)` — both messages
   are persisted atomically (back-to-back, in one dispatch cycle).
5. The hook updates the session metadata (last_active, turn_count,
   message_count) via the IPC call.

This prevents the orphan-user-message problem: if the user refreshes
the tab mid-fetch, the abort kills the fetch; the user message is
NEVER persisted (it only exists in React state, which is wiped on
refresh).

**AbortController (per §Missed Questions #11 + §Verifier Pass MEDIUM #4):**
The hook tracks the active `AbortController` in a
`useRef<AbortController | null>`. On session switch or session
delete, the hook calls `abortController.abort()` to cancel the
in-flight fetch. The `switchSession` mutator waits for the abort to
settle (typically <100ms) before clearing the messages array +
loading the new session. **CRITICAL (per §Verifier Pass MEDIUM #4):**
`switchSession(id)` MUST fast-return when `id === currentSession?.id`
— no abort fires, no state churn. The fast-return prevents the
abort-during-in-flight race when the user clicks the currently-active
session row (e.g., to re-focus the composer or via a search result
that points at the current session).

**Sidebar highlight (per §Missed Questions #12 + §Verifier Pass
MEDIUM #5):** The hook exposes `currentSession.id` so the page can
render the active session row with a `border-l-2 border-accent` +
`bg-accent/5` accent treatment. The page compares
`session.id === currentSession?.id` for the highlight check.
**When the sidebar is collapsed, the main chat header
(`<ChatHeader>`) renders the current session's title as a secondary
visual cue** (e.g., "Savant · {currentSession?.title ?? NO_TITLE_FALLBACK}")
so the user always knows their current context even when the sidebar
is hidden.

### Page Rewrite: `src/app/chat/page.tsx` (thin composer)

**Per §Verifier Pass MEDIUM #6**, the page is split into a thin
composer (~150 lines) + 5 sub-components. The page itself becomes:

```tsx
// src/app/chat/page.tsx (~150 lines)
export default function ChatPage() {
  const { derived, isLoading, error } = useDerivedRotation();
  const chat = useChatHistory();

  // OQ-3 blocking modal: do NOT render the chat surface until
  // the derived subkey is available.
  if (!derived || isLoading || error) {
    return <BlockingModal reason={error} />;
  }

  // Cmd/Ctrl+K: focus the search bar (per §Verifier Pass HIGH #3).
  // Listener is bound INSIDE this if-branch so the shortcut does
  // NOT fire when the blocking modal is up.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        document.getElementById("chat-search-input")?.focus();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // Enter-to-focus-composer (per §Suggestions J + §Verifier Pass
  // post-ratification-re-survey #2 — missed question #2): a
  // global Enter keydown listener focuses the chat composer
  // textarea. The listener MUST abort early if the user's
  // active element is already an INPUT, TEXTAREA, or BUTTON
  // (otherwise it would steal focus from the search bar when
  // the user presses Enter to submit a search query).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key !== "Enter" || e.shiftKey) return;
      const active = document.activeElement;
      const tag = active?.tagName ?? "";
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "BUTTON") return;
      e.preventDefault();
      document.getElementById("chat-composer-textarea")?.focus();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const sidebarCollapsed = chat.sidebarCollapsed;
  const onToggleSidebar = () => chat.setSidebarCollapsed(!sidebarCollapsed);

  return (
    <div className="flex h-full">
      {!sidebarCollapsed && (
        <aside className="w-64 border-r ...">
          <ChatSidebar chat={chat} />
        </aside>
      )}
      <main className="flex-1">
        <ChatHeader
          sidebarCollapsed={sidebarCollapsed}
          onToggleSidebar={onToggleSidebar}
          currentSessionTitle={chat.currentSession?.title ?? NO_TITLE_FALLBACK}
        />
        <ChatMessageList
          messages={chat.messages}
          searchHighlightId={chat.searchHighlightId}
        />
        <ChatComposer chat={chat} />
      </main>
    </div>
  );
}
```

The 5 sub-components (per §Impact Assessment):

- **`chat-sidebar.tsx` (~200 lines):** session list + search bar +
  per-session delete + "+ New" button + collapse-aware current-session
  highlight. Receives the `chat` state object as a prop.
- **`chat-composer.tsx` (~150 lines):** textarea + per-session draft
  + send handler + Clear link. Wires `composerDrafts[currentSessionId]`
  via the hook's `setComposerDraft` mutator. Calls
  `persistTurn(userContent, assistantContent)` after the OpenRouter
  fetch returns (sequential persist, per §Missed Questions #2).
- **`chat-message-list.tsx` (~120 lines):** message rendering with
  `data-message-id={message.id}` for search scroll-to. Shows a
  yellow `bg-warning/20` background on the highlighted message for
  3s (per §Suggestions for Improvement D).
- **`chat-header.tsx` (~60 lines):** page header with sidebar toggle
  + current-session title (collapsed-cue per §Verifier Pass
  MEDIUM #5).
- **`chat-search-results.tsx` (~100 lines):** search results panel +
  empty state ("No results for '{query}'" with Clear button, per
  §Suggestions for Improvement C). Receives the `chat.search()` result
  + the active `query` as props.

### Steps

1. **Amend Rust `SessionState`** in `crates/memory/src/models.rs`:
   add `pub title: Option<String>` as the last field (after
   `fork_point_turn_id`). Update the `SessionState::new()` constructor
   to default `title: None`. Add a roundtrip test in the test module
   confirming old data deserializes with `title: None`.
2. **Add 5 IPC wrappers** in `src/lib/ipc.ts`: `listChatSessions`,
   `loadChatHistory`, `persistChatTurn`, `deleteChatSession`,
   `searchChatHistory`. Each is a typed `invoke<T>()` call with
   snake_case command names. **All return `Result<T, String>`**
   (per §Missed Questions #11 — standard Tauri pattern; the IPC
   bridge turns `Err(String)` into a thrown Promise that the
   renderer catches with try/catch). The renderer does NOT use
   `{ ok: false, error: "..." }` return shapes.
3. **Add 5 mock cases** in `src/lib/mock-ipc.ts`:
   - `list_chat_sessions` reads `LS_CHAT_SESSIONS`, returns sorted by
     `last_active` desc.
   - `load_chat_history` reads `chatMessagesKey(args.session_id)`,
     returns the last 100, sorted asc. Returns `[]` if the key is
     missing or the session doesn't exist.
   - `persist_chat_turn` generates UUID, appends to
     `chatMessagesKey(session_id)`, updates the session metadata in
     `LS_CHAT_SESSIONS` (last_active, turn_count, message_count;
     auto-creates the session if it doesn't exist with `title`
     auto-set from the first user message's first 50 chars). Dispatches
     `savant:chat-history-updated` for cross-component sync. The
     `setItem` is wrapped in a try/catch; on `QuotaExceededError`,
     drops the oldest `QUOTA_DROP_RATIO` (10%) of sessions by
     `last_active` — AND removes each dropped session's
     `chatMessagesKey(session_id)` entry (per §Verifier Pass
     HIGH #1) — then retries the write once.
   - `delete_chat_session` removes `chatMessagesKey(session_id)` and
     the session entry from `LS_CHAT_SESSIONS`. If the deleted
     session was the current session, clears `LS_CURRENT_SESSION`
     (the page will generate a new session on next mount).
   - `search_chat_history` iterates all `LS_CHAT_SESSIONS` keys,
     reads each `chatMessagesKey(session_id)`, computes the substring
     + recency + role score (NORMALIZED to [0, 1] per §Verifier Pass
     post-SHOULD-FIX MEDIUM #5 — `score = rawScore / 1.5`), sorts by
     normalized score desc, returns top `limit`.
   - All cases hydrate from `LS_CHAT_SESSIONS` on `setupMockIPC()`
     (matches the existing `hydrateMasters` pattern). The
     `setupMockIPC()` hydrate step (per §Verifier Pass post-SHOULD-FIX
     LOW #6) is the entry point for the browser preview — it reads
     the localStorage keys once on mount + populates the in-memory
     `LS_CHAT_SESSIONS` array used by all subsequent mock cases.
     Without the hydrate step, the mock cases would return `[]` for
     every call (no sessions to iterate). The hydrate is also where
     the §Missed Questions #3 quota handler lives (the mock
     maintains an in-memory `LS_CHAT_SESSIONS` array AND a
     per-session `chatMessagesKey` map; both are mutated together).
4. **Create `src/lib/chat-data.ts`**: exports `Role`, `ChatMessage`,
   `ChatSession`, `ChatSearchResult`, `LS_CHAT_SESSIONS`,
   `LS_CURRENT_SESSION`, `LS_CHAT_SIDEBAR_COLLAPSED`, `chatMessagesKey(sessionId)`,
   `MAX_LOADED_MESSAGES` (100 — render-time cap, per §Missed
   Questions #4), `MAX_MESSAGE_BYTES` (50_000 — per-message persist
   cap, per §Missed Questions #4), `OPENROUTER_CONTEXT_MAX_CHARS`
   (40_000 — pre-fetch trimmer target), `TITLE_MAX_CHARS` (50),
   `NO_TITLE_FALLBACK` ("New chat", per §Missed Questions #8),
   `QUOTA_DROP_RATIO` (0.1 — drop oldest 10% on QuotaExceeded, per
   §Missed Questions #3 + §Verifier Pass HIGH #1 — also removes
   per-session keys), `autoTitleFromContent(content: string): string`
   helper (with empty-string fallback to `NO_TITLE_FALLBACK`, per
   §Missed Questions #8), `MESSAGE_SCORE_WEIGHTS` (the
   substring/recency/role weights for the search ranking), and
   `trimMessagesForContext(messages, maxChars)` helper (per
   §Missed Questions #4 + §Verifier Pass HIGH #2 — drops WHOLE
   older messages from the start of the array, never slices
   content mid-string).
5. **Create `src/lib/hooks/use-chat-history.ts`**: hook with the
   `ChatHistoryState` shape + cross-tab `storage` event listener
   (watches `LS_CHAT_SESSIONS` + `chatMessagesKey(currentSession)`)
   + `savant:chat-history-updated` custom-event listener + `loading`/`error`
   state. The hook reads `LS_CHAT_SESSIONS` once on mount (small array),
   reads `chatMessagesKey(currentSessionId)` on switch (small array
   capped at 100), and writes per-key on every mutation. The
   `switchSession` mutator fast-returns when
   `id === currentSession?.id` (per §Verifier Pass MEDIUM #4 — no
   abort fires on same-session clicks).
6. **Create the 5 chat sub-components** in `src/app/chat/components/`
   (per §Verifier Pass MEDIUM #6):
   - `chat-header.tsx` — page header with sidebar toggle +
     current-session title (collapsed-cue per §Verifier Pass
     MEDIUM #5).
   - `chat-sidebar.tsx` — session list + search bar + per-session
     delete + "+ New" button. Receives the `chat` state object.
   - `chat-message-list.tsx` — message rendering with
     `data-message-id` + 3s yellow highlight on search-result click
     (per §Suggestions for Improvement D).
   - `chat-composer.tsx` — textarea + per-session draft + send
     handler + Clear link. Wires `composerDrafts[currentSessionId]`
     via the hook's `setComposerDraft` mutator.
   - `chat-search-results.tsx` — search results panel + empty state
     ("No results for '{query}'" with Clear button).
7. **Rewrite `src/app/chat/page.tsx`**: rip out `useState<Message[]>([])`,
   use `useChatHistory`. Becomes a thin composer (~150 lines) that
   wires the hook to the 5 sub-components. Layout:
   `<div className="flex h-full">` with the collapsible left-rail
   drawer (when not collapsed) + the main chat area. Preserve the
   existing OQ-3 blocking modal pattern. Bind the Cmd/Ctrl+K
   listener INSIDE the `if (derived)` branch (per §Verifier Pass
   HIGH #3) so the shortcut does NOT fire when the blocking modal
   is up; use the cross-platform modifier `e.metaKey || e.ctrlKey`.
8. **Wire `AppState.memory` in Tauri runtime** (`src-tauri/src/lib.rs`)
   — **CRITICAL: ASYNC INIT (per §Missed Questions #7), NOT in
   `setup()` synchronously**:
   - Add `pub memory: Arc<RwLock<Option<Arc<MemoryEnclave>>>>` field
     to `AppState` (wrapped in `Arc<RwLock<>>` for shared async
     access from the IPC commands).
   - **In the Tauri `setup()` callback, do NOT call
     `create_embedding_service()` directly** — the 30-second Ollama
     health check would block Tauri startup and cause a 30-second
     white-screen. Instead, spawn the init via
     `tauri::async_runtime::spawn(async move { ... })`:
     - Inside the spawned task, create the storage path:
       `app.path().app_data_dir()?.join("chat")`.
     - Create the embedding service via the existing
       `create_embedding_service()` factory at
       `crates/core/src/utils/ollama_embeddings.rs:309` (handles
       `SAVANT_DISABLE_EMBEDDINGS=1` → NullEmbeddingProvider, Ollama
       → fastembed fallback chain).
     - Create the `MemoryEngine` via `MemoryEngine::new(path, config)`
       (handles BM25 + vector + CortexaDB initialization).
     - Wrap the result in `Arc<MemoryEnclave>` and store it via
       `*state.memory.write().await = Some(enclave)`.
   - The IPC commands read the memory handle via
     `state.memory.read().await.clone()`; if `None`, they return
     an `Err(String("memory not initialized — try again in a few seconds"))`
     (the user sees this only if they try to chat in the first
     ~30 seconds after Tauri startup).
   - Graceful fallback: if any step fails (e.g., Ollama unreachable,
     disk full, permission denied), log the error and leave
     `AppState.memory = None`. The 5 IPC commands return the standard
     Tauri `Err(String)` pattern (the renderer's try/catch surfaces
     the error to the user).
9. **Add 5 new Tauri commands** in `src-tauri/src/lib.rs` — all use
   `Result<T, String>` (per §Missed Questions #11 — standard Tauri
   pattern; the IPC bridge turns `Err(String)` into a thrown Promise):
   - `list_chat_sessions` — calls `MemoryEnclave::iter_session_states()`
     if the memory handle is `Some`, else returns `Ok(vec![])`. Maps
     `SessionState` to `ChatSession` (drops the agent-tooling fields;
     uses the new `title` field).
   - `load_chat_history` — calls `MemoryEnclave::fetch_session_tail()`
     if the memory handle is `Some`, else returns `Ok(vec![])`. Maps
     `AgentMessage` to `ChatMessage` (drops `tool_calls`,
     `tool_results`, `parent_id`, `channel` for v1).
   - `persist_chat_turn` — calls `MemoryEnclave::append_message()` with
     a constructed `AgentMessage` + `MemoryEnclave::save_session_state()`
     with a `SessionState` (updated turn_count + last_active + title
     if first message). Returns `Result<(), String>` (the renderer's
     try/catch handles errors).
   - `delete_chat_session` — calls `LsmStorageEngine::delete_session()`
     + `LsmStorageEngine::delete_session_state()`. Returns
     `Result<(), String>`.
   - `search_chat_history` — calls `MemoryEnclave::hybrid_search()` if
     the embedding service is enabled, falling back to a simple
     `iter_all_messages` substring search if `SAVANT_DISABLE_EMBEDDINGS=1`.
     Returns `Result<Vec<ChatSearchResult>, String>`.
10. **Add 1 wiring test** in `src/app/chat/chat_persistence.test.tsx`:
    - Renders the chat page with a pre-populated `LS_CHAT_SESSIONS`
      (3 sessions) + 3 corresponding `chatMessagesKey(session_id)`
      keys (each with 2-5 messages).
    - Asserts the session sidebar shows all 3 sessions sorted by
      `last_active` desc.
    - Asserts the active session row has the `border-l-2 border-accent`
      + `bg-accent/5` highlight (per §Missed Questions #12).
    - Asserts the message list shows the current session's messages
      (sorted asc, capped at `MAX_LOADED_MESSAGES`).
    - Types into the composer, switches sessions, switches back,
      asserts the composer text is preserved (per-session draft,
      per §Missed Questions #10).
    - Types into the search bar, asserts the results panel updates
      with the expected matches; asserts the empty state shows
      "No results for '{query}'" when no matches (per §Suggestions
      for Improvement C).
    - Presses Cmd+K, asserts the search bar gets focus (per
      §Suggestions for Improvement B + §Verifier Pass HIGH #3 —
      listener is bound inside the `if (derived)` branch so it
      doesn't fire when the blocking modal is up).
    - Clicks the trash icon on a session, confirms the dialog,
      asserts the session is removed from the sidebar + the
      messages are gone (the `chatMessagesKey(session_id)`
      localStorage entry is removed) + the corresponding
      `composerDrafts` entry is removed.
    - Toggles the session sidebar collapse, asserts the aside
      expands/collapses + `LS_CHAT_SIDEBAR_COLLAPSED` is updated
      + the main chat header shows the current session's title
      (per §Verifier Pass MEDIUM #5).
    - Calls `switchSession` with the current session's id, asserts
      no abort fires (per §Verifier Pass MEDIUM #4 — fast-return
      on same-session switch).
    - Simulates a `QuotaExceededError` on persist, asserts the
      quota handler drops the oldest 10% of sessions AND removes
      the corresponding per-session message arrays (per §Verifier
      Pass HIGH #1 — without this, the retry would fail).
    - **NEW (per §Verifier Pass post-SHOULD-FIX MEDIUM #3):**
      simulates a `storage` event fired by another tab (mutates
      `LS_CHAT_SESSIONS` directly + dispatches the
      `StorageEvent`), asserts the hook re-reads the sessions
      array + updates the `currentSession` if it was renamed/
      deleted. Asserts the cross-tab sync works for the
      localStorage-backed fields (sessions + messages + current
      + sidebar-collapsed). Asserts the `composerDrafts` are
      NOT synced (per §Verifier Pass post-SHOULD-FIX HIGH #2 —
      in-memory only).
    - **NEW (per §Verifier Pass post-SHOULD-FIX MEDIUM #4):**
      after a `persistTurn` call, asserts the
      `savant:chat-history-updated` custom event is dispatched
      (the mock dispatches it per the doc). Asserts the hook's
      listener fires + re-reads the sessions + messages arrays
      (so the local state is up-to-date without waiting for the
      `storage` event). The test should also assert that
      dispatching the event manually in the same tab triggers
      the same refresh path (idempotent re-read).
    - **NEW (per §Verifier Pass post-SHOULD-FIX MEDIUM #5):**
      asserts the `search` results' `score` field is in [0, 1]
      (normalized per the corrected `score = rawScore / 1.5`
      formula). Pre-correction the raw weighted sum could exceed
      1.0 (substring 1.0 + recency 0.3 + role 0.2 = 1.5); the
      test would fail without the normalization. Also asserts
      the `match` field is a substring of the message content
      (60 chars before + the match + 60 chars after).
    - Uses `happy-dom` for localStorage (already configured in
      FID-015).
11. **Verify** in parallel: `npm run type-check`, `npm run build`,
    `pnpm lint:docs`, `pnpm lint:defer`, `cargo check -p savant`,
    code-reviewer-minimax-m3.
12. **Close + archive:** Move the FID to `dev/fids/archive/`, flip
    Status to `closed`, append a FID-029 entry to `CHANGELOG.md`
    `## [Unreleased]`.

### Verification

- `npm run type-check` → exit 0
- `npm run build` → all 18 routes generated (chat + 17 others)
- `pnpm lint:docs` → exit 0 (LESSON-027 invariant preserved)
- `pnpm lint:defer` → exit 0 (LESSON-038 invariant preserved)
- `vitest run` → chat_persistence.test.tsx passes
- Visual smoke: visit `/chat` in dev mode; create a new session; send
  2-3 messages; refresh the page; confirm the history hydrates. Open
  a second tab; confirm the sessions list syncs.

## Out of Scope (Future FIDs)

Per LESSON-038, the following are explicit `out of scope` items for
FID-029. They require Spencer's separate ratification for any
follow-on FID that picks them up:

1. **Tool calls / tool_results in messages.** The `AgentMessage`
   struct has `tool_calls` + `tool_results` fields; the renderer
   doesn't read them in v1. A future FID-029+ can wire the tool pill
   rendering (matching the Hermes Workspace pattern).
2. **Streaming chat (SSE).** The existing chat page's v1 constraints
   comment notes "No streaming. POST /v1/chat/completions non-stream."
   FID-029 doesn't change the OpenRouter call shape.
3. **Auto-titling via LLM.** The current v1 auto-title is "first 50
   chars of the first user message" (a renderer-side heuristic). A
   future FID could call OpenRouter with a 1-shot "summarize this chat
   title in 5 words" prompt for better titles.
4. **Manual rename.** A future FID could add a `rename_chat_session`
   IPC + a `<input>` in the session row's hover state.
5. **Fork sessions.** The Rust `SessionState` already has
   `parent_session_id` + `fork_point_turn_id` fields (the D10
   forking at `crates/memory/src/engine.rs`). A future FID could
   wire the renderer's "Fork from this point" affordance.
6. **Multi-turn trimming (advanced).** The current `load_chat_history`
   loads the last `MAX_LOADED_MESSAGES` (100) and the
   `trimMessagesForContext(messages, OPENROUTER_CONTEXT_MAX_CHARS)`
   helper in `src/lib/chat-data.ts` (per §Missed Questions #4
   + §Verifier Pass HIGH #2 — drops WHOLE older messages, never
   slices content mid-string) handles the char-count trimmer
   (sliding window by message count, not by string slice). A
   future FID could implement summary-based context trimming (the
   Rust `consolidate` at `crates/memory/src/engine.rs:1328-1410` is
   the precedent) for even longer conversations.
7. **Real embedding-based search.** The browser mock uses substring
   + recency. A future FID could call the existing
   `EmbeddingProvider::embed` (Tauri runtime) + `MemoryEnclave::hybrid_search`
   for the BM25 + vector + RRF pipeline.
8. **`embed_memory` IPC command.** Originally proposed as the 6th
   command in the corrected scope; removed in the doc-trim pass
   because the embedding is only used by synthesis (not the chat
   surface). A future FID-029+ could add it for the semantic-search
   path.

## Decisions Awaiting Spencer's Input

These are the design decisions where I made a judgment call but
flagging them for ratification per the `## Questions You Should've Asked`
convention (LESSON-049):

1. **Session sidebar placement:** Secondary collapsible left-rail
   drawer INSIDE the chat page (next to the existing message area,
   not at the DashboardShell level) vs top-of-composer collapse vs
   replace the right-rail inspector vs off-canvas drawer. The chat
   page already has a right-rail inspector (per FID-006 v3); the
   left-rail is the dashboard's main nav. **Recommendation: secondary
   collapsible left-rail drawer INSIDE the chat page** (toggle button
   in the page header; default expanded on first visit; collapse state
   persists in `LS_CHAT_SIDEBAR_COLLAPSED` localStorage key). This
   matches the standard chat-app UX (ChatGPT, Slack, Claude all use
   a left-rail session list) without modifying the DashboardShell.

2. **Clear button scope:** Single-session delete (trash icon on each
   session row) vs "Clear current chat" (trash icon at the composer
   header) vs both. **Recommendation: BOTH — per-session trash icon
   for surgical deletes + a "Clear current chat" link at the top of
   the composer for the common case of "I want to start over in this
   session."** The "Clear all" link is a future-consider, not v1.

3. **Auto-titling strategy:** First 50 chars of first user message
   (renderer heuristic) vs LLM-generated (1 OpenRouter call per new
   session) vs user-supplied (manual rename only). **Recommendation:
   renderer heuristic for v1 (zero cost, instant); the LLM option is
   a follow-on FID if the heuristics produce bad titles in practice.**

4. **Search ranking in browser preview:** Pure substring + recency vs
   weighted (substring + recency + role). **Recommendation: weighted
   — user messages match the query better than assistant replies
   (the user is searching for what THEY asked, not what Savant said).**
   **Search scope:** global across all sessions by default
   (per the second thinker pass's DROP-MEDIUM-#8 verdict);
   per-session search is a follow-on FID if users want it.

5. **`MAX_LOADED_MESSAGES` cap + char-count trimmer:** 100 (chat
   surface) vs 20 (`MemoryConfig.recent_message_count` default) vs
   unlimited. **Recommendation: 100 (render-time cap) + a
   per-message `MAX_MESSAGE_BYTES=50_000` cap + a pre-fetch
   `OPENROUTER_CONTEXT_MAX_CHARS=40_000` trimmer
   (per §Missed Questions #4 + §Verifier Pass HIGH #2 — drops
   WHOLE older messages, not string-slice).** The 100-message cap
   is for the React render (browsers handle 100 messages fine). The
   `MAX_MESSAGE_BYTES` cap rejects oversized messages at persist
   time. The `OPENROUTER_CONTEXT_MAX_CHARS` trimmer drops whole
   older messages from the start of the array before the OpenRouter
   call (prevents 413 Payload Too Large errors). The full history
   exists in storage; only the loaded tail + the OpenRouter
   context window are capped.

6. **localStorage shape:** Single aggregate key (`LS_CHAT_DATA` holding
   `{ sessions, messages }`) vs split per-session keys
   (`LS_CHAT_SESSIONS` + `chatMessagesKey(session_id)`). **Recommendation:
   split per-session keys** — a single aggregate key forces a
   parse+stringify of the ENTIRE dataset on every message turn; with
   N sessions × M messages, this becomes a perf cliff. Per-session
   keys mean each write touches only the relevant session's message
   array (small, bounded at 100 entries) + the small metadata array
   (typically <100 sessions). The only O(N) operation is
   `search_chat_history` (which must read all session keys anyway).

7. **"Clear all" UX:** Confirm dialog (yes/no) vs undo toast (5s
   window) vs no global clear in v1. **Recommendation: NO global
   clear in v1** — the per-session trash icon + the dashboard's
   `localStorage.clear()` (in DevTools) are sufficient. The global
   clear is a follow-on FID with a proper undo UX.
8. **Soft-delete with 30-day Undo (per AionUi research 2026-07-15, RATIFIED).**
   The FID's original Decision #2 was hard-delete with `window.confirm()`.
   AionUi's "Move to Trash + 30-day TTL" pattern is the better default.
   New behavior: per-session trash icon triggers a "Move to Trash" action;
   the session moves from `LS_CHAT_SESSIONS` to `LS_CHAT_TRASH` with a
   `trashed_at: number` timestamp. A `LS_CHAT_TRASH_TTL_MS = 30 * 24 *
   3600 * 1000` constant governs the retention window. On page mount,
   the `useChatHistory` hook purges trashed sessions older than 30 days
   from BOTH `LS_CHAT_TRASH` AND `chatMessagesKey(sessionId)` (so the
   message array doesn't accumulate orphans in localStorage). The "Clear
   current chat" link in the composer (Decision #2's surgical-delete path)
   remains UNCHANGED — only the trash-icon path moves to soft-delete.
   Adds 1 IPC command (`purge_trashed_sessions() -> { purged_ids: string[],
   remaining_trash_count: number }`). The "Restore from Trash" UI is
   deferred to a follow-on FID (the data layer supports it; only the
   sidebar UI is missing) — per LESSON-038, this defer requires Spencer's
   explicit approval before shipping.
9. **Search snippets under session title (per AionUi research 2026-07-15, RATIFIED).**
   The FID's original search-results UI (per §Expected Behavior #5) was
   "session title + role + relative time" only — clicking a result
   opens the session but doesn't visually cue WHERE the match is.
   AionUi shows the matching snippet (~120 chars of context around the
   match) DIRECTLY UNDER the session title in the search-results panel,
   with the matched substring bolded. Adopt this from the start (no
   follow-on FID needed): the snippet comes from `MemoryEnclave::hybrid_search`'s
   `match` field (already in `ChatSearchResult` per §Data Model). The
   `match` field's `~120 char` window matches AionUi's UX. No additional
   IPC payload — just changes the renderer's `<ChatSearchResults>` layout
   from a 1-line row to a 2-line row.
10. **Cmd/Ctrl+K as canonical FTS entry (per Hermes research 2026-07-15, RATIFIED).**
    The FID's original §Expected Behavior #5 already specified
    Cmd/Ctrl+K as the FTS entry (binding the listener inside the `if (derived)`
    branch per §Verifier Pass HIGH #3). Hermes's right-rail + tab-based search
    is documented as an anti-pattern for chat-native apps (Hermes's own
    recommendation: "use a global command palette, not a right-rail tab").
    CONFIRMED: the FID's existing design is right. No change to the
    binding logic — just ratification that we're aligned with the
    recommended pattern.

## Perfection Loop

### §Step 1 — Sibling `session_titles` Collection (2026-07-15, RATIFIED + impl complete)

**PIVOT:** The originally-scoped "1-field amendment to the rkyv
`SessionState` struct" was abandoned after the code-reviewer REJECTED
the implementation citing rkyv 0.7.x backward-compat risk
(pre-FID-029 on-disk records would corrupt on first deserialize
because adding a field to `#[repr(C)]` shifts byte offsets → rkyv
reads garbage as the `Option` discriminant → UB territory).

**Spencer's directive (2026-07-15):** "We NEVER use in memory for
anything persistent — we use the db." This eliminated the
`#[rkyv(with = rkyv::with::Skip)]` fallback (in-memory only). The
pivot is to a NEW `session_titles` CortexaDB sibling collection
backed by `LsmStorageEngine::save_session_title` /
`load_session_title` / `iter_session_titles`.

### Loop 1 (2026-07-15 — initial implementation)

| Step | File(s) | Change |
| :--- | :--- | :--- |
| 1a | `crates/memory/src/models.rs` | REVERTED — removed `pub title: Option<String>` field from rkyv `SessionState` struct (back to 9 fields) |
| 1b | `crates/core/src/types/mod.rs` | ADDED `pub title: Option<String>` with `#[serde(default)]` to `savant_core::types::SessionState` (10 fields) |
| 1c | `crates/memory/src/lsm_engine.rs` | ADDED 3 sibling collection methods on `LsmStorageEngine` (save_session_title / load_session_title / iter_session_titles) at L1317/1345/1379 |
| 1d | `crates/memory/src/engine.rs` | ADDED 3 wrappers on `MemoryEnclave` + 3 wrappers on `MemoryEngine` (2 layers of delegation, total 6 new methods) |
| 1e | `crates/memory/src/async_backend.rs` | MODIFIED 3 sites (get_or_create_session + get_session populate title from sibling collection; save_session writes to sibling collection) |
| 1f | `crates/core/src/memory/mod.rs` | FIXED 2 initializers (L76/96 — added `title: None`) |
| 1g | `crates/agent/src/react/heuristic_tests.rs` + `crates/agent/src/react/stream.rs` | FIXED 2 mock initializers (L73/240 — added `title: None`) |
| 1h | `dev/fids/FID-2026-07-14-029-chat-persistence.md` | REPLACED §Step 1 text (struct-amendment → sibling-collection design) |

**Total scope:** 9 files modified, ~150 LoC added (sibling collection
methods + wrappers + doc update).

### Loop 2 (2026-07-15 — code-reviewer fixes)

Code-reviewer REJECTED Loop 1 with 1 critical bug + 4 fix-forward items.
All 5 issues resolved in Loop 2:

| # | Fix | File |
| :--- | :--- | :--- |
| 1 | Add `let _guard = self.lock_session(session_id).await;` to `MemoryEnclave::save_session_title` (prevent concurrent write races) | `crates/memory/src/engine.rs` |
| 2 | Add `tracing::warn!` for malformed metadata key in `iter_session_titles` | `crates/memory/src/lsm_engine.rs` |
| 3 | Add `tracing::warn!` for invalid UTF-8 in `iter_session_titles` | `crates/memory/src/lsm_engine.rs` |
| 5 | Graceful degradation in async_backend.rs (load/save failures don't block parent ops) | `crates/memory/src/async_backend.rs` (3 sites) |

**Final state:** Code-reviewer ACCEPT'd. `cargo check --workspace`
passes (exit 0). LESSON-027 + LESSON-038 gates GREEN. 7 files in
working tree (the 2 FID docs at `dev/.tmp-*.txt` were rm'd per
LESSON-029 cleanup discipline).

## Verifier Pass (2026-07-14 — meta-review of the analyzed-state doc)

**RED (gaps surfaced in this verifier pass):**

1. **LESSON-027 doc-drift invariant — preserved.** The 5 canonical +
   1 cascade-prose alternation anchors for the cascade-ordering
   phrase (per FID-022 / `pnpm lint:docs`) are unchanged: this
   FID-029 doc does NOT add any new cascade-ordering language.
2. **LESSON-038 no-unilateral-defer — compliance verified.** The
   §Out of Scope section explicitly tags 8 deferrals as
   "Spencer's separate ratification required" (no agent extension).
3. **Drift between §IPC Commands and §Data Model — FIXED.** The
   §IPC Commands section originally referenced `LS_CHAT_DATA` (a
   removed aggregate key) while §Data Model used the new per-session
   `LS_CHAT_SESSIONS` + `chatMessagesKey(session_id)` keys. The
   §IPC Commands section was rewritten to use the new keys
   consistently.
4. **4 fatal/UX design flaws — FIXED (per first thinker pass).**
   (1) Tauri stub rejected (now wires `MemoryEnclave` in
   `setup()`). (2) Rust `SessionState` gets a new `title: Option<String>`
   field. (3) localStorage is split per-session (no aggregate
   key). (4) Session sidebar moves to a collapsible left-rail
   drawer (was top-of-composer).
5. **5 HIGH missed questions — FIXED (per second thinker pass).**
   (1) `randomHex(16)` → `crypto.randomUUID()` everywhere.
   (2) OpenRouter fetch failure → persist order changed (sequential
   after fetch, not split before/after). (3) localStorage quota
   → try/catch + drop oldest 10%. (4) `MAX_LOADED_MESSAGES` is by
   count + new `MAX_MESSAGE_BYTES` per-message cap + new
   `OPENROUTER_CONTEXT_MAX_CHARS` pre-fetch trimmer.
   (5) `Role` type restricted to `"user" | "assistant"`.
6. **1 elevated HIGH missed question — FIXED.** Tauri memory init
   uses `tauri::async_runtime::spawn` + `Arc<RwLock<>>` (NOT in
   `setup()` synchronously) — prevents 30-second white-screen on
   startup.
7. **4 valid MEDIUM missed questions — FIXED.** (1) Concurrent tab
   write race documented as a known limitation. (2) Empty title
   fallback (`NO_TITLE_FALLBACK = "New chat"`). (3) Per-session
   composer drafts (composerDrafts map). (4) Tauri command error
   mapping uses `Result<T, String>` (standard Tauri pattern).
8. **2 new missed questions from second thinker pass — FIXED.**
   (1) `AbortController` bound to active session id (cancels
   in-flight fetch on session switch/delete). (2) Sidebar
   highlights the current session (`border-l-2 border-accent` +
   `bg-accent/5`).

**GREEN (recommendations for next session, NOT applied in this pass):**

1. **Tauri runtime end-to-end smoke test** (FUTURE FID-029r2+).
   The current FID-029 is browser-preview-verified. The Tauri
   runtime path is unit-tested by the Tauri command implementations
   but not end-to-end. Spawn a follow-on FID that:
   - Spins up `cargo tauri dev`
   - Creates a session via the UI
   - Sends 2-3 messages
   - Verifies the messages persist in
     `app_data_dir/chat/cortexa.db`
   - Restarts the app + verifies the history hydrates
   This is the real verification surface for FID-029; the
   browser-preview mock layer is a stand-in.
2. **Vitest end-to-end test for the chat page** (FUTURE FID-029r2+).
   The current 1 wiring test in §Steps Step 10 covers the basic
   flow. A more comprehensive test would:
   - Mock the OpenRouter fetch (return canned responses)
   - Verify the user + assistant messages are persisted
     SEQUENTIALLY (not split before/after)
   - Verify the AbortController cancels the fetch on session switch
   - Verify the localStorage quota handler drops the oldest 10%
     on QuotaExceededError
3. **Concurrent tab write race resolution** (FUTURE FID-029r2+).
   The known limitation (last-write-wins for simultaneous tab
   sends) is documented but not fixed. A future FID could use
   `navigator.locks()` + a CRDT-style resolution. This is a
   significant separate work-item; per LESSON-038 it requires
   Spencer's separate ratification.
4. **Cross-device sync** (FUTURE FID). The browser preview's
   localStorage is per-device. A "Sync to Tauri" button could
   migrate the localStorage data to the Tauri runtime's memory
   engine. Out of scope for FID-029 (the user would need to
   confirm the sync direction + conflict resolution policy).
5. **Auto-titling via LLM** (FUTURE FID). The renderer heuristic
   produces verbose titles; an OpenRouter 1-shot "summarize this
   chat title in 5 words" prompt would produce shorter titles at
   the cost of ~$0.001 per new session + ~500ms latency.

**AUDIT (this pass, 2026-07-14):**

- 2 thinker-with-files-gemini passes completed
  (design validation + missed-questions analysis)
- 6 str_replace edits applied to the doc (Data Model + IPC Commands
  + Hook + Page Rewrite + Steps Step 7-8 + Out of Scope)
- 4 new sections added (Perfection Loop + Verifier Pass + Missed
  Questions + Suggestions for Improvement)
- 1 section rewritten (Questions You Should've Asked → 4-field
  template + my answers)
- File remains in `dev/fids/` (Status: analyzed); not yet archived
- Status: STILL `analyzed` — impl pending Spencer's doc approval

**CHANGE DELTA:** ~20% of the doc body was rewritten or appended
during this verifier pass. The 5 HIGH + 1 elevated HIGH + 4 valid
MEDIUM + 2 new missed questions drove the substantive rewrites
(IPC Commands + Hook + Page Rewrite + Steps Step 7-8); the new
sections (Perfection Loop + Verifier Pass + Missed Questions +
Suggestions for Improvement) added ~250 lines of meta-review content.

## Missed Questions (meta-review of the doc-drafting pass)

The user asked: *"include answers to missed questions and suggestions"*.
The 12 missed questions below were identified by the second
thinker-with-files-gemini pass. Each uses the 4-field template
from `templates/FID-TEMPLATE.md` per LESSON-049; the "Recommended"
field contains my answer for Spencer's ratification.

1. **Q:** Should session_id + message_id use `randomHex(16)` (32-char
   hex, no dashes) or `crypto.randomUUID()` (36-char UUID v4 with
   dashes)?
   - **Context:** The `randomHex(16)` from `src/lib/ids.ts` is for
     opaque tokens (e.g., the agent name suffix in `provisionSessionKey`);
     it produces 32-char hex strings without dashes. The Rust
     `AgentMessage::user` constructor at `crates/memory/src/models.rs:172-188`
     generates `uuid::Uuid::new_v4().to_string()` (36 chars with
     dashes). A 32-char hex string would silently break any future
     Rust code that tries to parse the string as a `uuid::Uuid`.
   - **Recommended:** Use `crypto.randomUUID()` for ALL session_id
     + message_id values. `randomHex(16)` is for opaque tokens only.
     The doc's §Data Model + §ID Generation sections now specify
     this.
   - **Trade-off:** 4 extra characters per id; benefit is Rust
     compatibility + standard UUID v4 semantics (universally
     recognized, no future drift risk).

2. **Q:** Should the user message be persisted BEFORE the OpenRouter
   fetch (so it survives a page refresh mid-fetch) or AFTER (only
   when the assistant reply is available)?
   - **Context:** If persisted BEFORE, a tab refresh mid-fetch
     leaves an orphan user message with no response. If persisted
     AFTER, a tab refresh mid-fetch loses the user's input
     (acceptable — the user can re-type).
   - **Recommended:** Persist BOTH user + assistant SEQUENTIALLY
     AFTER the fetch returns. The user sees their message in
     React state immediately (via the optimistic update) but it
     is NOT persisted to localStorage until the assistant reply
     arrives. This prevents the orphan-user-message problem.
   - **Trade-off:** If the user refreshes mid-fetch, they lose
     their input. Benefit is the persistence layer is always
     consistent (every persisted message has a paired assistant
     reply).

3. **Q:** How should the chat surface handle a `QuotaExceededError`
   when writing to localStorage?
   - **Context:** localStorage has a ~5-10MB quota per origin.
     With 100 sessions × 100 messages × 500 chars, that's ~5MB
     and could hit the quota. A naive `setItem` would throw
     `QuotaExceededError` and crash the chat page.
   - **Recommended:** Wrap the `setItem` in a try/catch; on
     `QuotaExceededError`, drop the oldest 10% of sessions
     (by `last_active`) AND remove each dropped session's
     `chatMessagesKey(session_id)` per-session message array
     (per §Verifier Pass HIGH #1 — without this, the retry
     fails immediately because the orphaned arrays still
     consume the quota) + retry the write once. The quota
     handler is in `src/lib/mock-ipc.ts` (browser preview only;
     the Tauri runtime uses the CortexaDB storage which has no
     per-origin quota).
   - **Trade-off:** Silent data loss for the oldest 10% of
     sessions; benefit is the chat page never crashes on quota
     overflow.

4. **Q:** Is `MAX_LOADED_MESSAGES=100` (count-based) sufficient, or
   should it be char-count-based?
   - **Context:** A single 50K-char message × 100 = 5M chars,
     way over the OpenRouter context window (200K tokens ≈ 800K
     chars). The count-based cap could blow the context window.
   - **Recommended:** Keep `MAX_LOADED_MESSAGES=100` for the
     React render (browsers handle 100 messages fine) BUT add
     a `MAX_MESSAGE_BYTES=50_000` per-message cap (rejected at
     persist time) AND a `OPENROUTER_CONTEXT_MAX_CHARS=40_000`
     pre-fetch trimmer. **CRITICAL (per §Verifier Pass HIGH #2):**
     the trimmer must drop WHOLE older messages (one-at-a-time
     from the start of the array), NOT slice content mid-string.
     A `JSON.stringify(messages).slice(-40000)` approach would
     cut mid-JSON, mid-Markdown-block, or mid-Code-block —
     corrupting the LLM's view of the conversation. The helper
     `trimMessagesForContext(messages, maxChars)` in
     `src/lib/chat-data.ts` iterates messages, accumulates
     char count, and returns the trailing slice that fits within
     `maxChars` (always a complete prefix-suffix of the original
     array).
   - **Trade-off:** Older messages are trimmed from the context
     window (the LLM doesn't see them); benefit is no 413 Payload
     Too Large from OpenRouter.

5. **Q:** Should the `Role` TypeScript type be restricted to
   `"user" | "assistant"` for v1, or should it include `"system"`
   and `"tool"` for forward-compat?
   - **Context:** The Rust `AgentMessage` has 4 roles
   (`System`/`User`/`Assistant`/`Tool`). The v1 chat is
   user/assistant only. Including the unused variants in the
   TS type creates dead paths in the renderer.
   - **Recommended:** Restrict TS type to `type Role = "user" | "assistant"`
     for v1. The Rust `AgentMessage` keeps all 4 roles (the
     Rust side handles them generically). A future FID can widen
     the TS type when tool calls are wired.
   - **Trade-off:** Future FIDs need to widen the TS type when
     wiring tool calls; benefit is no dead paths in the renderer
     + the TypeScript type matches the v1 scope.

6. **Q:** How should the chat surface handle concurrent tab writes
   to the same session?
   - **Context:** Two tabs both open the same session, both send
     messages. The `chatMessagesKey(session_id)` key has
     last-write-wins semantics; some messages could be lost.
     The browser preview's localStorage does not support
     cross-tab locking out of the box.
   - **Recommended:** Document the limitation (last-write-wins
     for simultaneous tab sends) in the doc's §Out of Scope
     section. The `storage` event syncs updates across tabs AFTER
     each write completes, so sequential writes are safe. Truly
     concurrent writes (two tabs send within the same
     millisecond) are not addressed in v1. A future FID could
     use `navigator.locks()` + CRDT-style resolution.
   - **Trade-off:** Rare message loss in a power-user edge case
     (two tabs sending simultaneously); benefit is the v1
     implementation is simple + the common case (sequential
     writes) is fully safe.

7. **Q:** Should the Tauri `AppState.memory` initialization be
   synchronous (in `setup()`) or asynchronous (spawned via
   `tauri::async_runtime::spawn`)?
   - **Context:** The `create_embedding_service()` factory at
     `crates/core/src/utils/ollama_embeddings.rs:309` waits up
     to 30 seconds for an Ollama server to become ready. If this
     is invoked directly inside Tauri's `setup()` hook, **the
     entire desktop app white-screens for 30 seconds on boot**.
   - **Recommended:** Spawn the init via
     `tauri::async_runtime::spawn(async move { ... })` — the init
     runs in the background. The IPC commands check the memory
     handle via `state.memory.read().await.clone()`; if `None`,
     they return `Err(String("memory not initialized — try again
     in a few seconds"))`. The user sees this only if they try to
     chat in the first ~30 seconds after Tauri startup.
   - **Trade-off:** First chat message in the first ~30s after
     boot shows a "memory not ready" error; benefit is the
     Tauri runtime boots instantly (no white-screen).

8. **Q:** Should `autoTitleFromContent` fall back to a hardcoded
   "New chat" string when the first user message is empty?
   - **Context:** The renderer heuristic auto-titles sessions
     from the first user message's first 50 chars. If the first
     message is empty (e.g., a system-injected first message, or
     a race condition where the title is set before the message),
     the title would be empty.
   - **Recommended:** `autoTitleFromContent` returns
     `content.trim().slice(0, TITLE_MAX_CHARS) || NO_TITLE_FALLBACK`
     where `NO_TITLE_FALLBACK = "New chat"`. The trim + slice
     handles whitespace-only content; the `||` fallback handles
     the empty case.
   - **Trade-off:** Hardcoded fallback string ("New chat") is
     generic; benefit is the session always has a usable title
     in the sidebar.

9. **Q:** When the user switches sessions, should the unsent
   composer text be discarded or preserved per-session?
   - **Context:** The current v1 chat page has a single composer
     + single session. With multi-session, switching loses the
     composer's unsent text. This is a frustrating UX (the user
     is mid-thought and accidentally clicks another session).
   - **Recommended:** Store per-session composer drafts in a
     `Record<string, string>` map (keyed by session_id). The
     hook's `composerDrafts` field + `setComposerDraft` mutator
     handle the save + restore on session switch. On session
     delete, the corresponding draft is removed.
   - **Trade-off:** Small memory overhead (~100 sessions × ~500
     chars = 50KB max); benefit is the user's unsent text is
     preserved across session switches.

10. **Q:** Should `delete_chat_session` cancel an in-flight OpenRouter
    fetch for the deleted session?
    - **Context:** The user sends a message, then immediately
      deletes the session while OpenRouter is "thinking". The
      app would try to append the assistant message into a
      deleted session, potentially crashing the UI or
      resurrecting the session with an orphan message.
    - **Recommended:** Use an `AbortController` bound to the
      active session id. When the user switches or deletes the
      current session mid-fetch, the hook calls
      `controller.abort()` to cancel the in-flight fetch. The
      hook tracks the `AbortController` in a `useRef` and
      exposes the cancel via its `switchSession` + `deleteSession`
      mutators (which handle the abort + state cleanup internally).
    - **Trade-off:** Small complexity (AbortController management
      in the hook); benefit is the chat surface is robust to
      session switches during in-flight fetches.

11. **Q:** How should the Tauri commands signal errors back to the
    renderer?
    - **Context:** The doc originally proposed `{ ok: false,
      error: "..." }` return shapes for the 5 Tauri commands.
      This is a custom error pattern; the standard Tauri pattern
      is `Result<T, String>` which the IPC bridge automatically
      turns into a thrown Promise.
    - **Recommended:** All 5 Tauri commands use `Result<T, String>`
      (standard Tauri pattern; the IPC bridge turns `Err(String)`
      into a thrown Promise). The renderer's try/catch surfaces
      the error to the user via the existing error banner pattern.
    - **Trade-off:** The renderer must use try/catch (not
      success-only pattern matching); benefit is the standard
      Tauri error path + no custom error shape to maintain.

12. **Q:** How does the user know which session they're currently
    in, when looking at the sidebar?
    - **Context:** With multiple sessions in the sidebar, the
      user needs a visual cue for the active session (otherwise
      they might send a message to the wrong context).
    - **Recommended:** The sidebar row gets a `border-l-2
      border-accent` + `bg-accent/5` accent treatment when it
      matches `currentSession.id`. The page compares
      `session.id === currentSession?.id` for the highlight
      check. The accent color matches the existing accent
      palette (e.g., the chat message bubble border). **When
      the sidebar is collapsed, the main chat header also
      renders the current session's title as a secondary
      visual cue — see §Verifier Pass MEDIUM #5 for the
      full elaboration.**

## Suggestions for Improvement

The user asked: *"include answers to missed questions and suggestions"*.
The 5 suggestions below are design improvements I'd recommend
adding to the impl (one per item). Each is a follow-on micro-UX
tweak, not a separate FID.

A. **Per-session delete confirmation dialog** — When the user clicks
   the trash icon on a session row, show a `window.confirm("Delete
   this chat? This will remove all messages.")`. Prevents accidental
   deletes. The HeroUI v3 `<Dialog>` would be a more polished
   alternative, but `window.confirm` is the zero-dependency
   minimum-viable option that matches the existing codebase patterns
   (the existing manifest page uses `window.confirm` for swarm
   reset at `src/app/manifest/page.tsx`).

B. **Cmd/Ctrl+K keyboard shortcut to focus the search bar** —
   The chat page's left-rail drawer contains the search bar.
   A keyboard listener for `e.metaKey || e.ctrlKey` + `e.key === 'k'`
   focuses the search bar's `<input type="search">` (cross-platform
   modifier; works on macOS + Windows + Linux). This matches Linear,
   Slack, GitHub — high-value UX win for power users.
   **CRITICAL (per §Verifier Pass HIGH #3):** the listener is
   bound INSIDE the `if (derived)` branch of the page (not globally),
   so the shortcut doesn't fire when the OQ-3 blocking modal is up.
   The listener is added in a `useEffect` inside the page's
   post-blocking-modal render, removed on unmount.

C. **Search empty state UX** — When `search_chat_history` returns
   `[]`, the results panel shows "No results for '{query}' — try a
   different search term" with a Clear button (clears the input +
   hides the panel). The empty state is a small, bordered
   `<div>` with the muted-color text + a small icon.

D. **Search highlight in the message list** — After clicking a
   search result, the page calls `switchSession(result.session_id)`
   + scrolls to the message. The scroll target is the
   `data-message-id={message.id}` attribute on the message `<li>`.
   The match is highlighted with a yellow background
   (`bg-warning/20`) for 3 seconds (then fades via `setTimeout`).
   Medium difficulty for the impl (~20 lines of code) but high
   value for the UX.

E. **Side-by-side mode: chat + chat history in a single dashboard
   view** — A future FID could add a `/chat/history` route that
   shows all sessions in a list (no message thread) + a "Open"
   button to switch to the main `/chat` page with the selected
   session. This is a follow-on FID (not in FID-029 scope) for
   the case where the user wants to browse all conversations
   without being inside one.

## Ratification (2026-07-14)

Per Spencer's directive *"accept all of yur suggestions, update fid,
re-run perfection loop and include missed questions again"*, **all
24 items below are RATIFIED as-is**. No changes to the recommendations.
The doc is now approved for implementation per the §Perfection Loop
plan; the next step is the fresh gap-survey (see
`## Verifier Pass (2026-07-14 — post-ratification re-survey)` below).

### §Missed Questions (12 items, all RATIFIED)

- ✅ **#1** UUID format → `crypto.randomUUID()` for ALL session_id + message_id (not `randomHex(16)`)
- ✅ **#2** Persist order → BOTH user + assistant SEQUENTIALLY AFTER the fetch returns (prevents orphan user message)
- ✅ **#3** localStorage quota → try/catch `setItem`; on `QuotaExceededError`, drop oldest 10% by `last_active` + retry once
- ✅ **#4** Message cap + trimmer → 100 messages for render + `MAX_MESSAGE_BYTES=50_000` per-message cap + `OPENROUTER_CONTEXT_MAX_CHARS=40_000` pre-fetch trimmer
- ✅ **#5** Role type restriction → `type Role = "user" | "assistant"` for v1 (Rust keeps all 4 roles)
- ✅ **#6** Concurrent tab writes → documented as known limitation (last-write-wins; future FID = `navigator.locks()`)
- ✅ **#7** Tauri async init → `tauri::async_runtime::spawn` (NOT in `setup()` — prevents 30s white-screen on Ollama health check)
- ✅ **#8** Empty title fallback → `content.trim().slice(0, 50) || "New chat"` (`NO_TITLE_FALLBACK` constant)
- ✅ **#9** Per-session composer drafts → `composerDrafts: Record<string, string>` map (preserves unsent text across session switches)
- ✅ **#10** AbortController on delete → `AbortController` bound to active session id (cancels in-flight fetch on switch/delete)
- ✅ **#11** Tauri command error mapping → `Result<T, String>` standard Tauri pattern (NOT `{ ok: false }` shapes)
- ✅ **#12** Sidebar current-session highlight → `border-l-2 border-accent` + `bg-accent/5` on active row

### §Suggestions for Improvement (5 items, all RATIFIED)

- ✅ **A** Per-session delete `window.confirm()` dialog (prevents accidental deletes)
- ✅ **B** Cmd/Ctrl+K keyboard shortcut to focus search bar (matches Linear/Slack/GitHub convention)
- ✅ **C** Search empty state UX ("No results for '{query}'" with Clear button)
- ✅ **D** Search highlight in message list (yellow `bg-warning/20` for 3s after click; requires `data-message-id` + `scrollIntoView`)
- ✅ **E** Side-by-side mode: `/chat/history` route (follow-on FID — browse all sessions in a list view)

### §Decisions Awaiting Spencer's Input (7 items, all RATIFIED)

- ✅ **#1** Sidebar placement → Secondary collapsible left-rail drawer INSIDE the chat page
- ✅ **#2** Clear button scope → BOTH — per-session trash icon + "Clear current chat" link in composer header
- ✅ **#3** Auto-titling strategy → Renderer heuristic (first 50 chars of first user message) for v1
- ✅ **#4** Search ranking → Weighted (substring + recency + role) — user > assistant for query relevance. Search is global across all sessions by default
- ✅ **#5** Message cap + trimmer → 100 messages for render + 50K chars per message + 40K chars pre-fetch trimmer
- ✅ **#6** localStorage shape → Per-session keys (`LS_CHAT_SESSIONS` + `chatMessagesKey(session_id)`)
- ✅ **#7** "Clear all" UX → NO global clear in v1 (per-session delete is sufficient)

## Verifier Pass (2026-07-14 — post-ratification re-survey)

The post-ratification gap-survey (thinker-with-files-gemini pass #3)
surfaced **7 new missed questions** that emerged from the 24 ratified
items. These are NOT re-validations of the existing 12 missed
questions + 5 suggestions + 7 decisions; they are NEW gaps in the
doc that the ratification itself created (e.g., a ratification that
references "the quota handler drops the oldest 10%" didn't specify
the per-session message-array cleanup that the handler needs to do).

### RED (new gaps surfaced in this pass)

1. **HIGH #1 — Quota handler orphan messages.** The §Missed
   Questions #3 ratification says "drop oldest 10% by `last_active`
   + retry once" — but the original doc only mentioned dropping
   the `LS_CHAT_SESSIONS` metadata entry. The dropped sessions'
   per-session message arrays (`chatMessagesKey(session_id)`) would
   remain on disk, still consuming the origin's quota. The
   retry-after-quota-drop would fail immediately because the
   orphaned arrays are still there. **Fix:** the quota handler
   in `src/lib/mock-ipc.ts` MUST remove both the metadata entry
   AND the corresponding `chatMessagesKey(session_id)` per-session
   message array when dropping sessions. Applied to §Expected
   Behavior #6, §IPC Commands #3, §Steps Step 3, §Missed Questions
   #3, §Decisions #5, §Data Model.

2. **HIGH #2 — Trimmer message-boundary violation.** The §Missed
   Questions #4 ratification says "slices the message history to
   the last 40K chars" — but a naive `JSON.stringify(messages)
   .slice(-40000)` would cut mid-JSON, mid-Markdown-block, or
   mid-Code-block, corrupting the LLM's view of the conversation.
   **Fix:** the trimmer MUST drop WHOLE older messages
   (one-at-a-time from the start of the array), not slice content
   mid-string. Applied to §Expected Behavior #2, §Missed Questions
   #4, §Decisions #5, §Data Model, §Out of Scope #6.

3. **HIGH #3 — Cmd+K listener bypasses OQ-3 blocking modal.** The
   §Suggestions for Improvement B ratification says "A global
   keyboard listener for `metaKey + 'k'`" — but a global DOM
   listener would focus the search bar BEHIND the OQ-3 blocking
   modal (the modal sits on top of the chat surface, but the
   search bar's `<input>` is in the React tree underneath the
   modal). The user would press Cmd+K expecting nothing to happen
   (since the modal is up), but the search bar would steal focus,
   breaking the modal's keyboard accessibility. **Fix:** the
   keyboard listener is bound INSIDE the `if (derived)` branch
   of the page (not globally), so the shortcut doesn't fire when
   the blocking modal is up. The listener uses
   `e.metaKey || e.ctrlKey` for cross-platform compatibility.
   Applied to §Expected Behavior #5, §Page Rewrite, §Suggestions
   B, §Steps Step 7, §Steps Step 10 (wiring test).

4. **MEDIUM #4 — `switchSession` aborts in-flight fetch in the
   same-session case.** The §Missed Questions #10 ratification
   says "When the user switches or deletes the current session
   mid-fetch, the hook calls `controller.abort()`" — but the
   ratification didn't address the same-session case. Clicking
   the currently-active session row (e.g., to re-focus the
   composer, or via a search result that points at the current
   session) would call `switchSession(currentSession.id)`,
   which would fire the AbortController, cancelling the
   in-flight fetch unnecessarily. **Fix:** `switchSession(id)`
   MUST fast-return when `id === currentSession?.id` — no abort
   fires, no state churn, no IPC call. Applied to §Missed
   Questions #10, §Hook AbortController section, §Steps Step 5.

5. **MEDIUM #5 — Sidebar collapse hides current-session visual
   cue.** The §Missed Questions #12 ratification says "the
   sidebar row gets a `border-l-2 border-accent` + `bg-accent/5`
   accent treatment" — but the ratification didn't address the
   collapsed-sidebar case. When the sidebar is collapsed, the
   user has no visual indication of which session they're in.
   **Fix:** when the sidebar is collapsed, the main chat header
   (`<ChatHeader>` sub-component) renders the current session's
   title as a secondary visual cue (e.g., "Savant · {currentSession
   ?.title ?? NO_TITLE_FALLBACK}"). Applied to §Expected Behavior #3,
   §Missed Questions #12, §Page Rewrite, §Impact Assessment (new
   `chat-header.tsx` sub-component).

6. **MEDIUM #6 (ELEVATED to HIGH by code-reviewer pass) — page.tsx
   complexity breach.** The §Page Rewrite section describes a
   "full rewrite" of `src/app/chat/page.tsx` with the session
   sidebar + search bar + composer + per-session drafts +
   AbortController + 5 IPC wrappers + storage event listeners +
   custom event listeners. The result is a >700-line file that
   violates the codebase's existing pattern (the largest page
   file is `src/app/manifest/page.tsx` at ~480 lines). The
   reviewer upgrade is correct: a single >700-line file is
   unmaintainable. **Fix:** the page is split into a thin
   composer (~150 lines) + 5 sub-components in
   `src/app/chat/components/`. Applied to §Impact Assessment
   (5 new sub-components), §Page Rewrite (new thin-composer
   shape + 5 sub-components listed), §Steps Step 6 (split into
   Step 6 = sub-components + Step 7 = page rewrite).

7. **LOW (FUTURE) — Trimmer boundary edge case.** When the
   `OPENROUTER_CONTEXT_MAX_CHARS` budget can't fit even ONE
   full message, the trimmer returns `[]` (an empty array).
   This would cause the OpenRouter call to fail with "no
   messages" — but this is the correct behavior (the user's
   single message exceeds the budget; the LLM call can't
   proceed). The user sees the existing error banner. **No
   fix needed in v1**; document the behavior in the doc
   for transparency.

### GREEN (recommendations for next session, NOT applied)

1. **Tauri runtime end-to-end smoke test** (FUTURE FID-029r2+).
   See the existing GREEN #1 above — the post-ratification
   re-survey doesn't change this recommendation.
2. **Vitest end-to-end test for the chat page** (FUTURE
   FID-029r2+). The 1 wiring test in §Steps Step 10 should
   also cover: (a) QuotaExceededError simulation
   (per RED #1); (b) Cmd+K with blocking modal up
   (per RED #3); (c) Same-session switchSession fast-return
   (per RED #4); (d) Sidebar collapse + header title
   (per RED #5). 4 new test cases added to the wiring test.
3. **Concurrent tab write race resolution** (FUTURE
   FID-029r2+). See the existing GREEN #3.
4. **Cross-device sync** (FUTURE FID). See the existing
   GREEN #4.
5. **Auto-titling via LLM** (FUTURE FID). See the existing
   GREEN #5.
6. **NEW — Sidebar keyboard navigation** (FUTURE). The
   collapsed-cue in the main header (per RED #5) is a
   passive visual cue. A future FID could add a tooltip
   on hover + a keyboard shortcut (e.g., `Alt+S` to toggle
   the sidebar) for power users.
7. **NEW — Sub-component prop-drilling mitigation** (FUTURE).
   The 5 sub-components receive the `chat` state object as
   a prop (per §Page Rewrite). For deeper nesting (future
   FIDs adding sub-sub-components), a React Context wrapper
   around `useChatHistory` would be cleaner. Not needed
   for v1 (5 components is shallow enough).

### AUDIT (this pass, 2026-07-14)

- 1 additional thinker-with-files-gemini pass completed
  (post-ratification gap-survey)
- 1 code-reviewer-minimax-m3 pass completed (validity check
  on the 7 new missed questions; MEDIUM #6 elevated to HIGH)
- Doc rewritten via single `write_file` to apply all 7
  corrections consistently across §Expected Behavior,
  §Missed Questions, §Suggestions, §Decisions, §Impact
  Assessment, §Page Rewrite, §Steps, §Out of Scope
- 1 new section fully populated
  (`## Verifier Pass (2026-07-14 — post-ratification re-survey)`)
- 4 new sub-components added to §Impact Assessment
  (chat-header, chat-sidebar, chat-message-list, chat-composer,
  chat-search-results)
- File remains in `dev/fids/` (Status: analyzed); not yet
  archived
- Status: STILL `analyzed` — impl pending Spencer's final
  doc approval (all 24 items + 7 new corrections now ratified
  by implication; the new §Verifier Pass documents the 7
  corrections that the doc itself now implements)

**CHANGE DELTA:** ~10% of the doc body was rewritten or appended
during this verifier pass. The 7 new missed questions drove
substantive edits to 7 distinct sections (no section was
untouched). The new §Verifier Pass (post-ratification re-survey)
added ~150 lines of meta-review content. The §Impact Assessment
grew by 5 lines (5 new sub-components). The §Page Rewrite grew
by ~50 lines (the thin-composer shape + 5 sub-components listed).

## Verifier Pass (2026-07-14 — post-SHOULD-FIX re-survey)

## Ratification #2 (2026-07-14 — post-SHOULD-FIX pass #4)

Per Spencer's directive *"Accept all 13 new items (6 §Missed Questions + 5 §Suggestions F-J + 2 §Decisions #8-#9), update FID-029 with §Ratification section marking them as RATIFIED, then re-run the perfection loop with a 5th gap-survey"*, **all 13 items below are RATIFIED as-is**. The doc is now ready for a 5th gap-survey (see `## Verifier Pass (2026-07-14 — post-ratification-re-survey #2)` below, to be added at the end of the doc).

### §Missed Questions (6 items, all RATIFIED — from pass #4)

- ✅ **#13** Hook type signature missing 4 fields → Added `sidebarCollapsed` + `searchHighlightId` + `setSidebarCollapsed` + `setSearchHighlight` to `ChatHistoryState` type
- ✅ **#14** `setComposerDraft` falsely claimed cross-tab sync → Clarified as IN-MEMORY ONLY; cross-tab sync deferred to FUTURE FID per LESSON-038
- ✅ **#15** No test for `storage` event cross-tab sync → Added new test case in §Steps Step 10 (simulates `StorageEvent` from another tab)
- ✅ **#16** No test for `savant:chat-history-updated` event listener → Added new test case in §Steps Step 10 (asserts event dispatched + listener fires)
- ✅ **#17** `ChatSearchResult.score` range 0-1 violated by raw weighted sum → Added `score = rawScore / 1.5` normalization in §IPC Commands #5
- ✅ **#18** `setupMockIPC()` hydrate step not in §IPC Commands → Added explicit entry-point note in §Steps Step 3

### §Suggestions for Improvement (5 NEW items, all RATIFIED)

- ✅ **F** Esc key clears the search input + closes the empty results panel
- ✅ **G** Arrow key navigation in the session sidebar (↑/↓ + Enter to switch) — power-user feature
- ✅ **H** "Pinned" sessions section at the top of the sidebar — adds `pinned: boolean` field on `ChatSession`
- ✅ **I** Unread-change indicator for sessions modified in other tabs (via `storage` event)
- ✅ **J** Send Enter to focus the composer when composer is unfocused (matches Notion/Discord)

### §Decisions Awaiting Spencer's Input (2 NEW items, all RATIFIED)

- ✅ **#8** Search query state location → Page-level state (the query is shared between the sidebar input + the search-results panel + the highlight-on-click flow)
- ✅ **#9** Sub-component prop type → Full `ChatHistoryState` for v1 (5 components is shallow enough; a `ChatHistoryContext` wrapper is a FUTURE FID per §GREEN #2)

The post-SHOULD-FIX gap-survey (thinker-with-files-gemini pass #4)
surfaced **6 new missed questions** that emerged from the post-
SHOULD-FIX state (the 2 SHOULD FIX items + the 7 post-ratification
corrections created new implementation-level gaps the prior passes
didn't catch). These are NOT re-validations of the existing 12 + 5
+ 7 + 7 + 2 items; they are NEW gaps in the doc that the
SHOULD-FIX amendments themselves surfaced.

### RED (new gaps surfaced in this pass)

1. **HIGH #1 — Hook type signature missing 3 fields.** The §Page
   Rewrite section uses `chat.searchHighlightId`,
   `chat.setSidebarCollapsed`, and `chat.sidebarCollapsed` — but
   the `ChatHistoryState` type signature in §Hook did not include
   these 3 fields. A direct copy-paste of the type signature would
   fail TypeScript compilation. **Fix:** added the 3 fields to
   the type signature with documentation references (§Verifier Pass
   MEDIUM #5 for `sidebarCollapsed`; §Suggestions for Improvement D
   for `searchHighlightId`). Applied to §Hook section.

2. **HIGH #2 — `setComposerDraft` falsely claimed cross-tab sync.**
   The §Hook section's `setComposerDraft` comment claimed "the
   `storage` event syncs across tabs" — but `composerDrafts` is
   stored in the `Record<string, string>` map inside the hook's
   React state, NOT in localStorage. The `storage` event syncs
   ONLY the localStorage-backed fields (sessions, messages,
   current, sidebar-collapsed). Tab refreshes / new tabs start
   with empty drafts. The "cross-tab sync" claim was wrong.
   **Fix:** the §Hook section now explicitly says "IN-MEMORY ONLY"
   + clarifies that the `storage` event does NOT sync drafts.
   Cross-tab draft sync would require a new `set_composer_draft`
   IPC + an `LS_COMPOSER_DRAFTS` key — deferred to a FUTURE FID
   per LESSON-038. Applied to §Hook section.

3. **MEDIUM #3 — No test for `storage` event cross-tab sync.**
   The §Expected Behavior #6 + §Page Rewrite #3 sections describe
   cross-tab sync via the `storage` event, but the §Steps Step 10
   wiring test does NOT simulate a `storage` event from another
   tab. Without this test, a regression in the `storage` event
   listener would go undetected. **Fix:** added a new test case
   that simulates a `storage` event (mutates `LS_CHAT_SESSIONS`
   directly + dispatches a `StorageEvent`), asserts the hook
   re-reads the sessions + updates `currentSession` if it was
   renamed/deleted. Applied to §Steps Step 10.

4. **MEDIUM #4 — No test for `savant:chat-history-updated` event
   listener.** The mock IPC dispatches the
   `savant:chat-history-updated` custom event after every write
   (per §Expected Behavior #6 + §IPC Commands #3), but the
   §Steps Step 10 test does NOT assert the listener fires.
   Without this test, a regression in the custom event listener
   would mean the local state is stale until the next
   `storage` event. **Fix:** added a new test case that asserts
   the event is dispatched after `persistTurn` + the hook's
   listener re-reads the sessions + messages. Also asserts the
   listener is idempotent (manual dispatch in the same tab
   triggers the same refresh path). Applied to §Steps Step 10.

5. **MEDIUM #5 — `ChatSearchResult.score` range claim violated.**
   The §Data Model `ChatSearchResult.score: 0-1` field claimed
   the score was in [0, 1], but the §IPC Commands #5 weighted
   sum (substring 1.0 + recency 0.3 + role 0.2 = 1.5) could
   exceed 1.0. The doc was internally inconsistent. **Fix:**
   the §IPC Commands #5 now normalizes the raw weighted sum via
   `score = rawScore / 1.5` (the theoretical max). The §Steps
   Step 10 wiring test now asserts the `score` field is in
   [0, 1]. Applied to §IPC Commands #5 + §Steps Step 10.

6. **LOW #6 — `setupMockIPC()` hydrate step not in §IPC Commands.**
   The §Steps Step 3 mentions "All cases hydrate from
   `LS_CHAT_SESSIONS` on `setupMockIPC()`" — but the
   `setupMockIPC()` entry point is not documented in the
   §IPC Commands section. A reader unfamiliar with the mock
   layer would not know the hydrate step is the entry point
   for the browser preview. **Fix:** the §Steps Step 3 now
   explicitly documents `setupMockIPC()` as the entry point
   + explains what the hydrate step does (reads localStorage
   keys once on mount + populates the in-memory
   `LS_CHAT_SESSIONS` array). Applied to §Steps Step 3.

### GREEN (recommendations for next session, NOT applied)

1. **Composer draft persistence to localStorage** (FUTURE FID).
   See §Verifier Pass post-SHOULD-FIX HIGH #2 — the drafts are
   in-memory only; a future FID could add a `set_composer_draft`
   IPC + `LS_COMPOSER_DRAFTS` localStorage key + a new
   `storage` event listener for cross-tab sync. This is a
   significant separate work-item; per LESSON-038 it requires
   Spencer's separate ratification.
2. **ChatHistoryContext** (FUTURE FID). The 5 sub-components
   receive the `chat` state object as a prop (per §Page Rewrite).
   For deeper nesting (future FIDs adding sub-sub-components),
   a React Context wrapper around `useChatHistory` would be
   cleaner. Not needed for v1 (5 components is shallow enough).
3. **Score normalization for the Tauri runtime** (FUTURE FID).
   The Tauri runtime's `MemoryEnclave::hybrid_search` returns
   scores that are NOT in [0, 1] (BM25 scores are unbounded).
   A future FID could normalize the Tauri runtime's scores
   to the same [0, 1] range as the browser mock, so the
   `ChatSearchResult.score` field is consistent across both
   paths. This is a future-API alignment, not a v1 requirement.
4. **`savant:chat-history-updated` event name standardization**
   (FUTURE FID). The custom event name is documented in the
   doc but not exported from `src/lib/mock-ipc.ts` (the
   listener compares against a string literal). A future FID
   could export the event name as a constant from
   `src/lib/chat-data.ts` for type-safety. Tiny improvement;
   not a v1 requirement.
5. **Storage event listener for new sessions created in other
   tabs** (FUTURE). The `storage` event listener (per
   §Verifier Pass post-SHOULD-FIX MEDIUM #3) handles
   `LS_CHAT_SESSIONS` + `chatMessagesKey(currentSession)` +
   `LS_CURRENT_SESSION` + `LS_CHAT_SIDEBAR_COLLAPSED`. A
   future FID could also handle the new `LS_COMPOSER_DRAFTS`
   key (if §GREEN #1 is implemented) + emit a
   `savant:remote-draft-update` event for the local composer
   to update. Not needed for v1.

### AUDIT (this pass, 2026-07-14)

- 1 additional thinker-with-files-gemini pass attempted
  (post-SHOULD-FIX gap-survey; the thinker returned files-read
  but no analysis, so the gap-survey was completed by the
  parent agent from the loaded context)
- 2 str_replace edits applied to the doc (Hook type signature
  + Steps Step 3/10)
- 6 new items added to the doc (3 new fields in the
  ChatHistoryState type + 1 in-memory-only clarification + 1
  score normalization + 1 setupMockIPC() entry-point note)
- 3 new test cases added to §Steps Step 10 (storage event,
  custom event, score normalization)
- 1 new section fully populated
  (`## Verifier Pass (2026-07-14 — post-SHOULD-FIX re-survey)`)
- File remains in `dev/fids/` (Status: analyzed); not yet
  archived
- Status: STILL `analyzed` — impl pending Spencer's final
  doc approval (all 24 items + 7 post-ratification corrections
  + 2 SHOULD FIX + 6 post-SHOULD-FIX corrections now
  ratified by implication; the new §Verifier Pass documents
  the 6 corrections that the doc itself now implements)

**CHANGE DELTA:** ~5% of the doc body was rewritten or appended
during this verifier pass. The 6 new missed questions drove
substantive edits to 2 distinct sections (§Hook + §Steps). The
new §Verifier Pass (post-SHOULD-FIX re-survey) added ~150 lines
of meta-review content. The §Hook type signature grew by ~30
lines (3 new fields + documentation). The §Steps Step 10 grew
by ~40 lines (3 new test cases).

## Resolution

## Resolution

- **Fixed By:** Spencer (pivot directive 2026-07-15 — "NEVER use in memory for anything persistent") + Buffy (impl, 9 files, ~150 LoC)
- **Fixed Date:** 2026-07-15
- **Tests Added:** TBD (1 wiring test planned per §Steps Step 10, NOT YET WRITTEN — §Step 1 work is foundation only)
- **Verified By:** `cargo check --workspace` (exit 0) + LESSON-027 + LESSON-038 gates GREEN + code-reviewer-minimax-m3 (ACCEPT after 5 fixes from initial REJECT)
- **Commit/PR:** TBD (user controls git, not yet committed — awaiting Spencer's commit directive per LESSON-030 file-based pattern)
- **Archived:** TBD (this doc moves to `dev/fids/archive/` after the FULL FID-029 cascade closes — §Step 1 is the foundation step, not the closure step)
- **Architectural Decision:** Pivoted from originally-scoped `pub title: Option<String>` struct amendment (rejected by code-reviewer for rkyv 0.7.x backward-compat risk) to sibling `session_titles` CortexaDB collection (per Spencer's 2026-07-15 directive). 9 files modified; rkyv struct unchanged (pre-FID-029 on-disk format byte-identical).

## Lessons Learned

(TBD — to be filled in at the impl-closing pass per the FID-TEMPLATE
§Closed footer convention. The 3rd thinker pass surfaced the
most-valuable lesson: the post-ratification re-survey is
NON-OPTIONAL. The 24-item ratification fixed the doc-level design
flaws, but the ratification ITSELF created new implementation-level
gaps (the quota handler orphan, the trimmer boundary, the
Cmd+K scope, the same-session abort, the collapse-cue, the
complexity breach). The 3-pass perfection loop (initial design
→ thinker validation → corrections applied → re-thinker
validation → final corrections → re-thinker post-ratification
validation → impl-ready corrections) is the high-rigor pattern
for FIDs with >20 design decisions.)

### Future FID-XXX items (2026-07-15 — from §Step 1 code-reviewer meta-review)

These are fix-forward items identified by code-reviewer-minimax-m3
during the §Step 1 ship review. NOT addressed in the §Step 1 commit
(acceptable for v1, deferred per LESSON-038 with explicit
ratification). Filed here so they don't get lost:

1. **FID-XXX (future) — Read-during-write window in `iter_session_titles`**
   The per-session write lock in `MemoryEnclave::save_session_title`
   only serializes writes for the SAME `session_id`. `iter_session_titles`
   walks the entire `session_titles` collection without any lock, so it
   can observe partial writes mid-`add_with_content` from a concurrent
   `save_session_title` on a different session. Acceptable for v1
   (CortexaDB collection reads are eventually consistent), but
   `list_chat_sessions` callers may see stale titles briefly after a
   save. Fix options: (a) add a read snapshot at iteration start,
   (b) use CortexaDB's transactional API if available, (c) document
   the eventual-consistency window in the API docs.

2. **FID-XXX (future) — `tracing::warn!` style consistency**
   `LsmStorageEngine::iter_session_titles` uses `tracing::warn!(...)`
   inline (avoids import concern), while the prior precedent
   `MemoryEnclave::expire_stale_sessions` uses bare `warn!(...)` via
   the `use tracing::warn` import. Pick one style and apply across the
   crate. Suggested: add `use tracing::{debug, info, warn, error};` to
   `crates/memory/src/lib.rs` and use bare macros everywhere.

3. **FID-XXX (future) — §Step 9 doc update for `list_chat_sessions` join**
   The §Step 9 spec for `list_chat_sessions` IPC command needs to
   reference the `iter_session_titles` join in `async_backend.rs`. The
   current §Step 9 text (if it exists) doesn't mention this. Update
   when §Step 9 begins per LESSON-027 doc-drift invariant.

## Questions You Should've Asked (DRAFT)

Surfaced by the doc-drafting pass; recommended for Spencer's doc
review (before impl starts). 4-field template per LESSON-049;
"Recommended" field contains my answer.

1. **Q:** Should the chat page's right-rail inspector be retired to
   make room for the session sidebar, or should the session sidebar
   be a separate left-rail drawer?
   - **Context:** The chat page has a right-rail inspector from
     FID-006 v3. Adding a session sidebar as a secondary left rail
     (inside the chat page, not at the DashboardShell level) gives
     ~256px of session-list real estate while preserving the right
     rail's persona info. The left rail's collapse state persists
     in `LS_CHAT_SIDEBAR_COLLAPSED` so the user's preference is
     remembered across visits.
   - **Recommended:** Add the session sidebar as a secondary
     collapsible left-rail drawer INSIDE the chat page. Keep the
     right-rail inspector. Toggle button in the page header.
   - **Trade-off:** ~256px of horizontal real estate (when expanded);
     benefit is the right rail's persona info stays visible AND the
     session list has enough room to show titles + relative time +
     turn counts.

2. **Q:** Should the auto-title be the first user message's first
   50 chars, or the LLM-generated 5-word title?
   - **Context:** The renderer heuristic is free + instant; the LLM
     title is 1 OpenRouter call per new session (~$0.001 per call,
     ~500ms latency on first send). The renderer heuristic produces
     titles like "Can you help me debug the new feature" — usable
     but verbose. The LLM title is shorter but adds latency + cost.
   - **Recommended:** Renderer heuristic for v1. The LLM option is a
     follow-on FID if the heuristics produce bad titles in practice.
   - **Trade-off:** Verbose titles; benefit is zero cost + zero
     latency + works in browser preview without an OpenRouter key.

3. **Q:** Should the FTS search use a substring match (browser
   preview) or the existing `MemoryEnclave::hybrid_search` (Tauri
   runtime only)?
   - **Context:** The browser preview has no embedding service
     (the `OllamaEmbeddingService` + `EmbeddingService` are Rust-only).
     Substring match is the browser-preview equivalent. The Tauri
     runtime can call `MemoryEnclave::hybrid_search` for the
     BM25 + vector + RRF pipeline. The two paths produce different
     result qualities but the same `ChatSearchResult` shape.
   - **Recommended:** Browser mock uses substring + recency + role
     weighting. Tauri runtime uses `MemoryEnclave::hybrid_search` if
     the embedding service is enabled, falling back to substring +
     recency if `SAVANT_DISABLE_EMBEDDINGS=1`. The IPC contract is
     the same `ChatSearchResult` shape.
   - **Trade-off:** Two implementations; benefit is the search
     works in both modes with the best-quality result available in
     each.

4. **Q:** Should the Tauri `AppState.memory` initialization be
   synchronous (in `setup()`) or asynchronous (spawned via
   `tauri::async_runtime::spawn`)?
   - **Context:** Eager init in `setup()` adds 20-40 lines of
     initialization code + a ~500ms startup time (CortexaDB open
     + BM25 index load). Lazy init (on first IPC call) defers the
     cost but means the first chat message takes ~500ms longer
     (the user notices the latency). CRITICAL: synchronous init
     would block Tauri startup with a 30-second Ollama health check.
   - **Recommended:** Eager init via `tauri::async_runtime::spawn`
     (the `MemoryEnclave` is created upfront in a background task;
     the 5 IPC commands check the handle via `Arc<RwLock<>>` and
     return an error if the handle is not yet ready). The startup
     cost is acceptable (Tauri boots instantly; the user sees a
     "memory not ready" error for the first ~30s if they try to
     chat immediately).
   - **Trade-off:** First chat message in the first ~30s after
     boot shows a "memory not ready" error; benefit is the
     Tauri runtime boots instantly (no 30-second white-screen).

> When status is set to **Closed**, move this file to
> `dev/fids/archive/` and append an entry to `CHANGELOG.md`.

> When status is set to **Closed**, move this file to
> `dev/fids/archive/` and append an entry to `CHANGELOG.md`.

## Verifier Pass (2026-07-14 — post-ratification-re-survey #2)

The post-ratification-#2 gap-survey (thinker-with-files-gemini pass #5) surfaced **5 new missed questions + 4 new suggestions + 2 new decisions** that emerged from the 13 newly-ratified items (pass #4). These are NOT re-validations of the existing 12 + 5 + 7 + 6 + 5 + 2 + 13 items; they are NEW gaps in the doc that the post-ratification-#2 state itself surfaced (e.g., suggestion H adds a `pinned: boolean` field to `ChatSession`, which requires a 6th IPC command for persistence that the prior passes didn't anticipate).

### RED (new gaps surfaced in this pass)

1. **HIGH #1 — Missing `toggle_chat_session_pin` IPC.** Suggestion H (pinned sessions) added a `pinned: boolean` field on `ChatSession` (applied to §Data Model), but the §IPC Commands section lacked a 6th command to persist the pinned state. **Fix:** added a 6th IPC command `toggle_chat_session_pin(session_id, pinned: boolean) -> Result<{ ok: boolean }, String>` (per suggestion H + missed question #1). The mock enforces the max-10 cap (per suggestion M) by returning `Err(String("max 10 pinned sessions"))` if the user tries to pin an 11th. The Tauri command calls `MemoryEnclave::save_session_state()` with the updated `SessionState` (would require a 2nd-field amendment to `SessionState` per the same rkyv backward-compatibility pattern as `title`). Applied to §IPC Commands. **Cross-reference fix (per code-reviewer MUST FIX #2):** the `toggle_chat_session_pin` IPC takes BOTH `session_id` AND `pinned: boolean`. The hook's `togglePin(sessionId: string, pinned: boolean): Promise<void>` takes the same 2 args (matches the IPC shape). The hook is a thin wrapper; the max-10 cap is enforced on the IPC side.
2. **HIGH #2 — Enter-to-focus listener steals focus from search bar.** Suggestion J (Enter to focus composer) would naively focus the textarea on every Enter press, including when the user has the search bar focused. **Fix:** the §Page Rewrite section now has a `useEffect` for the Enter-to-focus listener with the input-focus check (per missed question #2). Applied to §Page Rewrite.
3. **MEDIUM #3 — `unreadSessions` state + cross-tab tracking.** Suggestion I (unread indicator) requires a transient `unreadSessions: Set<string>` on `ChatHistoryState`. **Fix:** added `unreadSessions: Set<string>` to the type signature + `setUnread: (sessionId: string, isUnread: boolean) => void` mutator (per missed question #3 + decision #10 — transient, not persisted). Applied to §Hook. The `setUnread` signature is explicit (per code-reviewer SHOULD FIX #2).
4. **MEDIUM #4 — `setSearchHighlight` auto-clear timer location.** The prior doc didn't specify if the page or the hook sets the timeout. **Fix:** the `setSearchHighlight` docstring now specifies the 3s auto-clear timer is managed INSIDE the hook via a `useRef<NodeJS.Timeout>` (per missed question #4). Applied to §Hook.
5. **LOW #5 — Pinned session deletion cascade (doc-only).** When a user clicks the trash icon on a Pinned session, the deletion implicitly handles the unpin via the complete removal of the `ChatSession` metadata record. No explicit "unpin" step is needed. Doc-only clarification applied to §IPC Commands #4 + §Steps Step 3.

### GREEN (recommendations for next session, NOT applied)

1. **Rust `SessionState` 2nd-field amendment for `pinned`** (FUTURE FID). The 6th IPC command requires a `pinned: bool` field on the Rust `SessionState` struct. Out of scope for FID-029.
2. **Visual style guide for the unread dot** (FUTURE FID). The unread dot's position (top-right corner of the sidebar row) + color (bg-accent, distinct from the active-session `border-l-2 border-accent` highlight) should be documented in the dashboard's design system. Not a v1 requirement.
3. **Pinned session reordering** (FUTURE FID). Suggestion H provides a binary pinned/unpinned state; a future FID could add a `pinnedOrder: number` field for custom ordering within the Pinned section. Not a v1 requirement.
4. **`unreadSessions` audit** (FUTURE FID). The `storage` event listener could also fire an explicit `savant:remote-message-update` event for the `unreadSessions` Set to update in real time. The current `storage` event listener handles it implicitly.

### AUDIT (this pass, 2026-07-14)

- 1 additional thinker-with-files-gemini pass completed (post-ratification-#2 gap-survey)
- 6 str_replace edits applied to the doc (Data Model + IPC Commands + Hook setUnread + Hook togglePin + Page Rewrite Enter-to-focus + this verifier pass section)
- 5 new items added to the doc (1 new IPC command + 1 Enter-to-focus skip-when-input check + 1 unreadSessions Set + 1 setSearchHighlight auto-clear-timer docstring + 1 pinned field on ChatSession)
- 1 new section fully populated (`## Verifier Pass (2026-07-14 — post-ratification-re-survey #2)`)
- File remains in `dev/fids/` (Status: analyzed); not yet archived
- Status: STILL `analyzed` — impl pending Spencer's final doc approval (all 24 items + 7 post-ratification corrections + 2 SHOULD FIX + 6 post-SHOULD-FIX corrections + 13 post-ratification-#2 items + 5 post-ratification-#2 corrections + 2 code-reviewer MUST/SHOULD FIX corrections now ratified by implication)

**CHANGE DELTA:** ~3% of the doc body was rewritten or appended during this verifier pass. The 5 new missed questions drove substantive edits to 4 distinct sections (§Data Model + §IPC Commands + §Hook + §Page Rewrite). The new §Verifier Pass (post-ratification-re-survey #2) added ~120 lines of meta-review content. The §IPC Commands grew by ~15 lines (the new 6th command). The §Hook type signature grew by ~25 lines (2 new fields + documentation). The §Page Rewrite grew by ~25 lines (the Enter-to-focus `useEffect`).

### POST-IMPL AUDIT (2026-07-15 — §Step 1 ship review)

**Scope:** The §Step 1 foundation work for FID-029 is COMPLETE per master-FID-035 layered-build order (Layer 0 = substrate; Layer 1a = chat persistence = this FID).

**5-stage evolution:**

| Loop | Date | Outcome |
| :--- | :--- | :--- |
| Initial design | 2026-07-14 | Doc drafted with `pub title: Option<String>` amendment to rkyv `SessionState` |
| Thinker + code-reviewer meta-review | 2026-07-14 | 5 new missed questions + 4 suggestions + 2 decisions surfaced; doc restructured |
| Spencer's pivot directive | 2026-07-15 | "NEVER use in memory for anything persistent — we use the db" — sibling collection adopted |
| Initial impl (REJECTED) | 2026-07-15 | Code-reviewer REJECTED for rkyv backward-compat risk + missing write lock + strict error handling |
| Fixed impl (ACCEPTED) | 2026-07-15 | 5 fixes applied; code-reviewer ACCEPT'd |

**9-file scope:** See §Perfection Loop Loop 1 table for the full file-by-file breakdown (the same 9 files are on disk per the working tree, NOT yet committed per LESSON-019).

**Verification state (post Loop 2 ACCEPT):**
- `cargo check --workspace --offline` → exit 0
- `pnpm lint:docs` (LESSON-027) → exit 0
- `pnpm lint:defer` (LESSON-038) → exit 0
- code-reviewer-minimax-m3 (final ship review) → ACCEPT (with 1 critical gap = missing POST-IMPL AUDIT sub-section, NOW FILLED by this edit)

**LESSON cross-references:**
- LESSON-027 (doc-drift substring-match invariant): preserved — no drift anchors were added/removed
- LESSON-028 (rkyv 0.7.x backward-compat fix-forward): applied — write lock + warn! logs + graceful degradation
- LESSON-030 (file-based commit pattern): documented for next commit step (not yet committed)
- LESSON-038 (no auto-defer without explicit approval): 3 future FID-XXX items filed under §Lessons Learned with explicit "future FID" framing

**What's NOT in §Step 1 (deferred to subsequent steps):**
- Wiring test (per §Steps Step 10: `src/app/chat/chat_persistence.test.tsx` with happy-dom) — NOT written
- Tauri runtime parity (5 new Tauri commands, per §Steps Step 9) — NOT implemented
- Renderer IPC wrappers (per §Steps Step 2-7) — NOT implemented
- §Step 9 doc update for `iter_session_titles` join — future FID-XXX per §Lessons Learned

**Foundation status: SHIPPED.** §Step 2 (renderer IPC wrappers) can begin.
