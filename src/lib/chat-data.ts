"use client";

// FID-029 §Step 4 — chat-data.ts.
//
// Single source of truth for the chat-persistence types, localStorage
// keys, magic-number caps, + the small set of helpers used across the
// IPC layer (`src/lib/ipc.ts`), the browser-preview mock
// (`src/lib/mock-ipc.ts`), the hook (`use-chat-history.ts`), the 5
// sub-components in `src/app/chat/components/`, and the page rewrite
// at `src/app/chat/page.tsx`.
//
// Per ECHO Law 13 (\"build modular, combine overlap, one function, one
// truth\"), the renderer does NOT inline these constants in any of the
// downstream files — they import from here. A change to
// `LS_CHAT_SESSIONS` propagates to all consumers without per-file
// hunt-and-replace.
//
// The DAEMON-side backing for `chat-data.ts`'s `ChatMessage` /
// `ChatSession` shapes is `savant_memory::AgentMessage` + the
// `session_titles` sibling collection at
// `crates/memory/src/engine.rs` (per FID-029 §Step 1 sibling-collection
// pivot, shipped in v0.0.7). v0.0.8 ships the renderer-side wire-up;
// the DAEMON IPC bridge lands in FID-032 (Layer 3) per master FID-035
// §Layered Build Order.

// ─── Types ──────────────────────────────────────────────────────────────

/** Render-side mirror of `savant_memory::MessageRole`. */
export type Role = "user" | "assistant" | "system" | "tool";

/** One chat message. `ts` is Unix epoch milliseconds. */
export type ChatMessage = {
	id: string;
	role: Role;
	content: string;
	ts: number;
};

/** Sidebar entry — session metadata only. */
export type ChatSession = {
	session_id: string;
	title: string;
	created_at: number;
	last_active_at: number;
	turn_count: number;
	message_count: number;
};

/** Search-result shape. `score` is the normalized [0, 1] ranking. */
export type ChatSearchResult = {
	session_id: string;
	message_id: string;
	score: number;
	match_offset: number;
};

// ─── LocalStorage keys (UI-only; chat content NEVER lives in
// localStorage in the Tauri runtime — DAEMON is the source of truth per
// Spencer's \"We NEVER use in memory for anything persistent\" doctrine) ─

/** All chat sessions: array<ChatSession>. */
export const LS_CHAT_SESSIONS = "savant.chat.sessions";

/** The active tab's currently-focused session_id (renderer-only state). */
export const LS_CURRENT_SESSION = "savant.chat.current_session_id";

/** UI-only sidebar collapsed-vs-expanded layout flag. */
export const LS_CHAT_SIDEBAR_COLLAPSED = "savant.chat.sidebar.collapsed";

/**
 * Per-session composer drafts — the in-flight textarea text per
 * session_id. Persisted to localStorage so a tab refresh restores
 * the user's unfinished message.
 */
export const CHAT_DRAFT_PREFIX = "savant.chat.draft.";

/**
 * Custom cross-tab broadcast event. Emitted after every successful
 * `persist_chat_turn` / `delete_chat_session` (in the DAEMON-route
 * and the mock-ipc route) so other tabs + other subscribers re-render
 * without waiting on the next passive `storage` polling cycle.
 *
 * Lives here as the SINGLE source of truth (per ECHO Law 13) — both
 * `mock-chat.ts` (the producer) and `use-chat-history.ts` (the
 * consumer) import from this canonical location.
 */
export const CHAT_HISTORY_UPDATED_EVENT = "savant:chat-history-updated";

/**
 * Per-session messages key. The session_id is substituted into the
 * helper below at write time so sessions can't collide on key prefix.
 */
export function chatMessagesKey(sessionId: string): string {
	return `savant.chat.messages.${sessionId}`;
}

// ─── Magic-number caps ───────────────────────────────────────────────────

/** Render-time cap per session — older messages get lazy-trimmed
 *  client-side after loadCompletion. Aligned to OpenRouter's typical
 *  context-window guardrails. */
export const MAX_LOADED_MESSAGES = 100;

/** Per-message persist cap to defend against QuotaExceededError when
 *  the user pastes a huge blob. Truncates the tail, never fails the
 *  persist. */
export const MAX_MESSAGE_BYTES = 50_000;

/** Pre-fetch trimmer target — keeps total conversation under this
 *  before POST /v1/chat/completions to avoid OpenRouter's
 *  context-window eviction mid-response. */
export const OPENROUTER_CONTEXT_MAX_CHARS = 40_000;

/** Sidebar title cap — once a session title hits this length we
 *  ellipsize. Tied to the typical sidebar width. */
export const TITLE_MAX_CHARS = 50;

/** Used by `autoTitleFromContent` when the first user message is empty
 *  (e.g. a pasted image with no alt text). */
export const NO_TITLE_FALLBACK = "New chat";

/** On QuotaExceededError, drop this fraction (sorted by last_active
 *  ASC, oldest first) of sessions, then retry the write ONCE. Also
 *  removes each dropped session's `chatMessagesKey(...)` entry to
 *  avoid orphaned per-session keys. */
export const QUOTA_DROP_RATIO = 0.1;

// ─── Search ranking weights ────────────────────────────────────────────────
//
// `score = rawScore / 1.5` normalized to [0, 1] per FID §Verifier Pass
// post-SHOULD-FIX MEDIUM #5. The raw score is a weighted sum:
//   - substring match: 1.0 per match (capped at 1.0 by per-message cap)
//   - recency:         0.3 (most-recent last_active_at)
//   - role boost:      0.2 if message.role === "user" (capture intent)

export const MESSAGE_SCORE_SUBSTRING = 1.0;
export const MESSAGE_SCORE_RECENCY = 0.3;
export const MESSAGE_SCORE_ROLE_BOOST = 0.2;
export const MESSAGE_SCORE_MAX_RAW = 1.5; // for normalization to [0, 1]

// ─── Helpers ────────────────────────────────────────────────────────────

/**
 * Derive a sidebar title from the first user message's content.
 * Truncates to TITLE_MAX_CHARS; falls back to NO_TITLE_FALLBACK for
 * empty or whitespace-only input per FID §Missed Questions #8.
 */
export function autoTitleFromContent(content: string): string {
	const trimmed = content.trim();
	if (!trimmed) return NO_TITLE_FALLBACK;
	if (trimmed.length <= TITLE_MAX_CHARS) return trimmed;
	return trimmed.slice(0, TITLE_MAX_CHARS - 1) + "…";
}

/**
 * Trim a messages array to fit under `maxChars` total character count.
 * Per FID §Missed Questions #4 + §Verifier Pass HIGH #2, drops WHOLE
 * older messages from the start of the array — never slices content
 * mid-string, which would break the OpenRouter streaming parser.
 *
 * Assumes `messages` is in chronological order (oldest first); the
 * returned array remains in chronological order (newest preserved).
 */
export function trimMessagesForContext(
	messages: ChatMessage[],
	maxChars: number,
): ChatMessage[] {
	const total = messages.reduce(
		(sum, m) => sum + m.content.length,
		0,
	);
	if (total <= maxChars) return messages;
	const trimmed: ChatMessage[] = [];
	let acc = 0;
	// Walk newest-to-oldest, accumulating until we hit the cap, then
	// reverse back to chronological order. Skips oldest first per spec.
	for (let i = messages.length - 1; i >= 0; i--) {
		const m = messages[i];
		if (acc + m.content.length > maxChars) {
			continue;
		}
		trimmed.unshift(m);
		acc += m.content.length;
	}
	return trimmed;
}

/**
 * Check whether `text` contains `query` as a substring (case-sensitive
 * — substring MVP per FID §Step 6; real Tantivy/Bleve FTS engine is a
 * future FID). Returns the byte offset of the first match, or -1 if
 * not found.
 */
export function substringOffset(haystack: string, needle: string): number {
	return haystack.indexOf(needle);
}
