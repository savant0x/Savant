"use client";

// FID-029 §Step 3 — chat-persistence mock helper module.
//
// Single source of truth for the 5 mock cases that `src/lib/mock-ipc.ts`
// dispatches. Lives in its own module per ECHO Law 13 (single-purpose
// utility). The browser-preview path never reaches the
// `savant_memory::MemoryEnclave` DAEMON — it reads / writes the
// `LS_CHAT_SESSIONS` + `chatMessagesKey(...)` localStorage keys via
// these helpers.
//
// Hydration (per FID §Verifier Pass post-SHOULD-FIX LOW #6):
// `hydrateChatMockState()` reads the localStorage keys on first
// access + populates the in-memory `mockChatSessions` array +
// `mockChatMessagesPerSession` map. Without this hydrate the 5 cases
// would return `[]` / no-op for every invocation. Auto-hydrated at
// module load (idempotent — re-reads on every setter as a
// belt-and-braces).

import { randomHex } from "@/lib/ids";
import {
	autoTitleFromContent,
	chatMessagesKey,
	CHAT_HISTORY_UPDATED_EVENT,
	LS_CHAT_SESSIONS,
	MESSAGE_SCORE_MAX_RAW,
	MESSAGE_SCORE_RECENCY,
	MESSAGE_SCORE_ROLE_BOOST,
	MESSAGE_SCORE_SUBSTRING,
	QUOTA_DROP_RATIO,
	substringOffset,
	type ChatMessage,
	type ChatSearchResult,
	type ChatSession,
} from "@/lib/chat-data";

// Note: CHAT_HISTORY_UPDATED_EVENT is imported (not redeclared) from
// chat-data.ts per ECHO Law 13 single-source-of-truth.

let mockChatSessions: ChatSession[] = [];
let mockChatMessagesPerSession: Map<string, ChatMessage[]> = new Map();
let hydrated = false;

// ─── Hydration ───────────────────────────────────────────────────────────

/** Read the localStorage keys + populate the in-memory state. Idempotent. */
function hydrateChatMockState(): void {
	if (typeof window === "undefined") return;
	if (hydrated) return;
	try {
		const raw = window.localStorage.getItem(LS_CHAT_SESSIONS);
		mockChatSessions = raw ? (JSON.parse(raw) as ChatSession[]) : [];
	} catch {
		mockChatSessions = [];
	}
	mockChatMessagesPerSession = new Map();
	for (const session of mockChatSessions) {
		try {
			const raw = window.localStorage.getItem(
				chatMessagesKey(session.session_id),
			);
			if (raw) {
				mockChatMessagesPerSession.set(
					session.session_id,
					JSON.parse(raw) as ChatMessage[],
				);
			}
		} catch {
			/* ignore per-session corruption */
		}
	}
	hydrated = true;
}

function persistSessions(): void {
	if (typeof window === "undefined") return;
	try {
		window.localStorage.setItem(
			LS_CHAT_SESSIONS,
			JSON.stringify(mockChatSessions),
		);
	} catch {
		/* quota fail handled below */
	}
}

function persistMessages(sessionId: string): void {
	if (typeof window === "undefined") return;
	const messages = mockChatMessagesPerSession.get(sessionId) ?? [];
	try {
		window.localStorage.setItem(
			chatMessagesKey(sessionId),
			JSON.stringify(messages),
		);
	} catch {
		dropOldestAndRetry(sessionId, messages);
	}
}

/**
 * On QuotaExceededError, drop the oldest QUOTA_DROP_RATIO of sessions
 * (by last_active_at) AND remove each dropped session's per-session
 * messages key. Then retry the write ONCE per FID §Verifier Pass
 * HIGH #1.
 */
function dropOldestAndRetry(
	sessionId: string,
	messages: ChatMessage[],
): void {
	if (typeof window === "undefined") return;
	const dropCount = Math.max(
		1,
		Math.floor(mockChatSessions.length * QUOTA_DROP_RATIO),
	);
	const sorted = [...mockChatSessions].sort(
		(a, b) => a.last_active_at - b.last_active_at,
	);
	const toDrop = sorted.slice(0, dropCount);
	for (const s of toDrop) {
		try {
			window.localStorage.removeItem(chatMessagesKey(s.session_id));
		} catch {
			/* keep going */
		}
		mockChatMessagesPerSession.delete(s.session_id);
	}
	mockChatSessions = mockChatSessions.filter(
		(s) => !toDrop.some((d) => d.session_id === s.session_id),
	);
	persistSessions();
	try {
		window.localStorage.setItem(
			chatMessagesKey(sessionId),
			JSON.stringify(messages),
		);
	} catch {
		/* second failure — accept the data loss (rare private-mode bug) */
	}
}

function dispatchUpdated(): void {
	if (typeof window === "undefined") return;
	window.dispatchEvent(new Event(CHAT_HISTORY_UPDATED_EVENT));
}

// ─── Public API (the 5 dispatchable helpers) ─────────────────────────────

/** `list_chat_sessions` — sorted by `last_active_at` desc. */
export function sortedSessions(): ChatSession[] {
	hydrateChatMockState();
	return [...mockChatSessions].sort(
		(a, b) => b.last_active_at - a.last_active_at,
	);
}

/** `load_chat_history` — capped at `limit` (default 100), sorted asc. */
export function loadChatHistoryMock(
	sessionId: string,
	limit = 100,
): ChatMessage[] {
	hydrateChatMockState();
	const messages = mockChatMessagesPerSession.get(sessionId);
	if (!messages) return [];
	const sorted = [...messages].sort((a, b) => a.ts - b.ts);
	return sorted.slice(-limit);
}

/** `persist_chat_turn` — append + update session metadata + auto-title. */
export function persistTurnMock(
	sessionId: string,
	userMessage: ChatMessage,
	assistantMessage: ChatMessage,
): void {
	hydrateChatMockState();
	const trimmedUser: ChatMessage = {
		...userMessage,
		id: userMessage.id || `u-${Date.now()}-${randomHex(4)}`,
	};
	const trimmedAssistant: ChatMessage = {
		...assistantMessage,
		id:
			assistantMessage.id ||
			`a-${Date.now()}-${randomHex(4)}`,
	};
	const messages = mockChatMessagesPerSession.get(sessionId) ?? [];
	messages.push(trimmedUser, trimmedAssistant);
	mockChatMessagesPerSession.set(sessionId, messages);

	const now = Date.now();
	const idx = mockChatSessions.findIndex(
		(s) => s.session_id === sessionId,
	);
	if (idx >= 0) {
		mockChatSessions[idx] = {
			...mockChatSessions[idx],
			title:
				mockChatSessions[idx].title === "New chat"
					? autoTitleFromContent(trimmedUser.content)
					: mockChatSessions[idx].title,
			turn_count: mockChatSessions[idx].turn_count + 1,
			message_count: mockChatSessions[idx].message_count + 2,
			last_active_at: now,
		};
	} else {
		mockChatSessions.push({
			session_id: sessionId,
			title: autoTitleFromContent(trimmedUser.content),
			created_at: now,
			last_active_at: now,
			turn_count: 1,
			message_count: 2,
		});
	}
	persistSessions();
	persistMessages(sessionId);
	dispatchUpdated();
}

/** `delete_chat_session` — remove per-session key + metadata. */
export function deleteSessionMock(sessionId: string): void {
	hydrateChatMockState();
	mockChatMessagesPerSession.delete(sessionId);
	mockChatSessions = mockChatSessions.filter(
		(s) => s.session_id !== sessionId,
	);
	if (typeof window !== "undefined") {
		try {
			window.localStorage.removeItem(chatMessagesKey(sessionId));
		} catch {
			/* private-mode fail */
		}
	}
	persistSessions();
	dispatchUpdated();
}

/** `search_chat_history` — substring + recency + role score, normalized. */
export function searchHistoryMock(
	query: string,
	limit = 50,
): ChatSearchResult[] {
	hydrateChatMockState();
	if (!query.trim()) return [];
	const hits: ChatSearchResult[] = [];
	const now = Date.now();
	for (const session of mockChatSessions) {
		const messages =
			mockChatMessagesPerSession.get(session.session_id) ?? [];
		for (const message of messages) {
			const offset = substringOffset(message.content, query);
			if (offset < 0) continue;
			const recency =
				(now - message.ts) / (1000 * 60 * 60 * 24) < 7
					? MESSAGE_SCORE_RECENCY
					: 0;
			const roleBoost =
				message.role === "user" ? MESSAGE_SCORE_ROLE_BOOST : 0;
			const raw =
				MESSAGE_SCORE_SUBSTRING + recency + roleBoost; // 1.0 + 0.3 + 0.2
			const score = Math.min(raw / MESSAGE_SCORE_MAX_RAW, 1);
			hits.push({
				session_id: session.session_id,
				message_id: message.id,
				score,
				match_offset: offset,
			});
		}
	}
	hits.sort((a, b) => b.score - a.score);
	return hits.slice(0, limit);
}
