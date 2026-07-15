"use client";

// FID-029 §Step 5 — useChatHistory hook.
//
// Central renderer-side state for the chat surface. Mirrors the
// `useReflections` boot-time-reader pattern (FID-017) but extends
// it with active mutation per FID-029 §Step 5 spec. Authoritative
// data lives in the DAEMON's `savant_memory::MemoryEnclave` in the
// Tauri runtime (per FID-029 §Step 1 sibling-collection pivot,
// shipped v0.0.7); in the browser-preview path this hook reads +
// writes the `LS_CHAT_SESSIONS` + `chatMessagesKey(sessionId)`
// localStorage keys via `src/lib/mock-ipc.ts`.
//
// Cross-tab sync (FID-029 §Step 7 specific): two event classes:
//   1. `storage` events on `LS_CHAT_SESSIONS` or the active session's
//      `chatMessagesKey(...)` \u2014 another tab mutated state.
//   2. Custom `savant:chat-history-updated` broadcast \u2014 another tab
//      finished a turn; we re-render without waiting for the next
//      passive `storage` polling cycle.
//
// The hook's `switchSession` mutator fast-returns when `id ===
// currentSession?.id` per FID \u00a7Verifier Pass MEDIUM #4 \u2014 no abort fires
// on same-session clicks (avoids racy hydration churn).

import { useCallback, useEffect, useRef, useState } from "react";
import { randomHex } from "@/lib/ids";
import {
	deleteChatSession,
	listChatSessions,
	loadChatHistory,
	persistChatTurn,
	searchChatHistory,
} from "@/lib/ipc";
import {
	CHAT_HISTORY_UPDATED_EVENT,
	LS_CHAT_SESSIONS,
	LS_CURRENT_SESSION,
	LS_CHAT_SIDEBAR_COLLAPSED,
	autoTitleFromContent,
	chatMessagesKey,
	MAX_LOADED_MESSAGES,
	MAX_MESSAGE_BYTES,
	type ChatMessage,
	type ChatSearchResult,
	type ChatSession,
	type Role,
} from "@/lib/chat-data";

/** Local re-export of the data-module event name removed — the
 *  canonical constant lives in `@/lib/chat-data` (Law 13 single
 *  source of truth). Consumers should import from there directly:
 *
 *    import { CHAT_HISTORY_UPDATED_EVENT } from "@/lib/chat-data"
 */

export type ChatHistoryState = {
	/** Sidebar list \u2014 sorted by `last_active_at` desc. */
	sessions: ChatSession[];
	/** Active session_id. `null` if the user hasn't picked one yet
	 *  (the page generates a new one on first send). */
	currentSessionId: string | null;
	/** Messages of the active session, chronological order. */
	messages: ChatMessage[];
	/** `true` while the first hydration pass is in flight. */
	loading: boolean;
	/** Last error string from any mutation; null on success. */
	error: string | null;
	/** Active-session's pending composer text (per-session draft). */
	composerDraft: string;
	/** Sidebar collapsed flag (UI-only). */
	sidebarCollapsed: boolean;
	/** Cmd/Ctrl+K palette hit list. */
	searchHits: ChatSearchResult[];
	/** Last query string. */
	searchQuery: string;

	// ─── mutators ──────────────────────────────────────────────────

	/** Switch to an existing session by id. No-op when already
	 *  active. */
	switchSession: (id: string) => Promise<void>;
	/** Append a turn (user + assistant) to the active session.
	 *  Creates a new session if `currentSessionId` is null. */
	sendTurn: (
		userText: string,
		assistantText: string,
		userRole?: Role,
	) => Promise<void>;
	/** Hard-delete a session by id. Clears `LS_CURRENT_SESSION`
	 *  when it matches the deleted one. */
	removeSession: (id: string) => Promise<void>;
	/** Start a brand-new empty session. */
	createSession: () => Promise<void>;
	/** Update the active session's composer draft. */
	setComposerDraft: (draft: string) => void;
	/** Toggle the sidebar collapsed state. */
	toggleSidebar: () => void;
	/** Run the substring MVP search (§Step 6 \u2014 real Tantivy/Bleve
	 *  FTS engine is a future FID). */
	search: (query: string) => Promise<void>;
	/** Clear any active search. */
	clearSearch: () => void;
	/** Re-hydrate from disk (manual refresh). */
	refresh: () => Promise<void>;
};

function generateSessionId(): string {
	return `sess-${randomHex(8)}`;
}

function getLsString(key: string): string {
	if (typeof window === "undefined") return "";
	try {
		return window.localStorage.getItem(key) ?? "";
	} catch {
		return "";
	}
}

function setLsString(key: string, value: string): void {
	if (typeof window === "undefined") return;
	try {
		if (value) {
			window.localStorage.setItem(key, value);
		} else {
			window.localStorage.removeItem(key);
		}
	} catch {
		/* private-mode fail \u2014 the in-memory `currentSessionId` covers. */
	}
}

export function useChatHistory(): ChatHistoryState {
	const [sessions, setSessions] = useState<ChatSession[]>([]);
	const [currentSessionId, setCurrentSessionId] = useState<string | null>(
		null,
	);
	const [messages, setMessages] = useState<ChatMessage[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);
	const [composerDraft, _setComposerDraft] = useState("");
	const [sidebarCollapsed, _setSidebarCollapsed] = useState<boolean>(
		() => getLsString(LS_CHAT_SIDEBAR_COLLAPSED) === "1",
	);
	const [searchHits, setSearchHits] = useState<ChatSearchResult[]>([]);
	const [searchQuery, setSearchQuery] = useState("");
	const [tick, setTick] = useState(0);
	// Avoid re-creating a session on every refresh.
	const initializedRef = useRef(false);

	// ─── Hydration: list sessions → read active session's messages ───
	useEffect(() => {
		if (typeof window === "undefined") return;
		let cancelled = false;
		(async () => {
			setLoading(true);
			setError(null);
			try {
				const list = await listChatSessions();
				if (cancelled) return;
				let activeId = getLsString(LS_CURRENT_SESSION);
				if (!initializedRef.current) {
					initializedRef.current = true;
					// First boot: if the persisted active id is unknown, clear it.
					if (activeId && !list.some((s) => s.session_id === activeId)) {
						setLsString(LS_CURRENT_SESSION, "");
						activeId = "";
					}
				}
				if (!activeId) {
					if (!cancelled) {
						setSessions(list);
						setCurrentSessionId(null);
						setMessages([]);
						setLoading(false);
					}
					return;
				}
				if (cancelled) return;
				setSessions(list);
				setCurrentSessionId(activeId);
				const history = await loadChatHistory(
					activeId,
					MAX_LOADED_MESSAGES,
				);
				if (cancelled) return;
				setMessages(history);
				setLoading(false);
			} catch (e) {
				if (cancelled) return;
				setError(e instanceof Error ? e.message : String(e));
				setLoading(false);
			}
		})();
		return () => {
			cancelled = true;
		};
	}, [tick]);

	// ─── Cross-tab sync ──────────────────────────────────────────────
	useEffect(() => {
		if (typeof window === "undefined") return;
		const onStorage = (e: StorageEvent): void => {
			if (
				e.key === LS_CHAT_SESSIONS ||
				(currentSessionId !== null &&
					e.key === chatMessagesKey(currentSessionId))
			) {
				setTick((t) => t + 1);
			}
		};
		const onLocalChange = (): void => {
			setTick((t) => t + 1);
		};
		window.addEventListener("storage", onStorage);
		window.addEventListener(CHAT_HISTORY_UPDATED_EVENT, onLocalChange);
		return () => {
			window.removeEventListener("storage", onStorage);
			window.removeEventListener(
				CHAT_HISTORY_UPDATED_EVENT,
				onLocalChange,
			);
		};
	}, [currentSessionId]);

	const broadcastChanged = useCallback((): void => {
		if (typeof window === "undefined") return;
		window.dispatchEvent(new Event(CHAT_HISTORY_UPDATED_EVENT));
	}, []);

	const refresh = useCallback(async (): Promise<void> => {
		setTick((t) => t + 1);
	}, []);

	const switchSession = useCallback(
		async (id: string): Promise<void> => {
			if (id === currentSessionId) {
				// Fast-return per FID \u00a7Verifier Pass MEDIUM #4 \u2014 same-session
				// clicks do not fire a hydration pass.
				return;
			}
			setLsString(LS_CURRENT_SESSION, id);
			setCurrentSessionId(id);
			setMessages([]);
			setSearchHits([]);
			setSearchQuery("");
			setError(null);
			_composerDraft(id);
			try {
				const history = await loadChatHistory(id, MAX_LOADED_MESSAGES);
				setMessages(history);
			} catch (e) {
				setError(e instanceof Error ? e.message : String(e));
			}
		},
		[currentSessionId],
	);

	const createSession = useCallback(async (): Promise<void> => {
		const id = generateSessionId();
		setLsString(LS_CURRENT_SESSION, id);
		setCurrentSessionId(id);
		setMessages([]);
		setSearchHits([]);
		setSearchQuery("");
		setError(null);
		_composerDraft(id);
		// No empty `persist_chat_turn` call \u2014 sessions are created lazily
		// on the first `sendTurn`, not on tab-open.
		broadcastChanged();
	}, [broadcastChanged]);

	const sendTurn = useCallback(
		async (
			userText: string,
			assistantText: string,
			_userRole: Role = "user",
		): Promise<void> => {
			const target = currentSessionId ?? generateSessionId();
			if (!currentSessionId) {
				setLsString(LS_CURRENT_SESSION, target);
				setCurrentSessionId(target);
			}
			const ts = Date.now();
			const userMessage: ChatMessage = {
				id: `u-${ts}-${randomHex(4)}`,
				role: "user",
				content: userText.slice(0, MAX_MESSAGE_BYTES),
				ts,
			};
			const assistantMessage: ChatMessage = {
				id: `a-${ts}-${randomHex(4)}`,
				role: "assistant",
				content: assistantText.slice(0, MAX_MESSAGE_BYTES),
				ts,
			};
			// Optimistic UI \u2014 the DAEMON route writes-on-success; on
			// failure we roll back.
			setMessages((m) => [...m, userMessage, assistantMessage]);
			try {
				await persistChatTurn(target, userMessage, assistantMessage);
				// First turn \u2192 derive sidebar title from the user content.
				if (messages.length === 0) {
					try {
						const list = await listChatSessions();
						const idx = list.findIndex(
							(s) => s.session_id === target,
						);
						if (idx >= 0) {
							list[idx] = {
								...list[idx],
								title: autoTitleFromContent(userText),
								turn_count: list[idx].turn_count + 1,
								message_count:
									list[idx].message_count + 2,
								last_active_at: ts,
							};
						}
						setSessions(list);
					} catch {
						/* sidebar update is best-effort */
					}
				}
				broadcastChanged();
			} catch (e) {
				setMessages((m) =>
					m.filter(
						(x) =>
							x.id !== userMessage.id &&
							x.id !== assistantMessage.id,
					),
				);
				setError(e instanceof Error ? e.message : String(e));
				throw e;
			}
		},
		[currentSessionId, messages.length, broadcastChanged],
	);

	const removeSession = useCallback(
		async (id: string): Promise<void> => {
			try {
				await deleteChatSession(id);
				if (currentSessionId === id) {
					setLsString(LS_CURRENT_SESSION, "");
					setCurrentSessionId(null);
					setMessages([]);
				}
				broadcastChanged();
			} catch (e) {
				setError(e instanceof Error ? e.message : String(e));
			}
		},
		[currentSessionId, broadcastChanged],
	);

	const setComposerDraft = useCallback(
		(draft: string): void => {
			_setComposerDraft(draft);
			// Per-session draft persisted to localStorage via a
			// composerDrafts map key; the page.tsx rewrites this on
			// switch. Kept inline here for parity with use-derived-rotation.
			if (currentSessionId) {
				try {
					window.localStorage.setItem(
						`savant.chat.draft.${currentSessionId}`,
						draft,
					);
				} catch {
					/* quota fail \u2014 in-memory state covers. */
				}
			}
		},
		[currentSessionId],
	);

	const toggleSidebar = useCallback((): void => {
		_setSidebarCollapsed((c) => {
			const next = !c;
			setLsString(LS_CHAT_SIDEBAR_COLLAPSED, next ? "1" : "");
			return next;
		});
	}, []);

	const search = useCallback(async (query: string): Promise<void> => {
		setSearchQuery(query);
		if (!query.trim()) {
			setSearchHits([]);
			return;
		}
		try {
			const hits = await searchChatHistory(query, 50);
			setSearchHits(hits);
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
			setSearchHits([]);
		}
	}, []);

	const clearSearch = useCallback((): void => {
		setSearchQuery("");
		setSearchHits([]);
	}, []);

	// Per-session draft loader \u2014 when `currentSessionId` flips, the
	// following inline lambda pulls the stored draft so the composer
	// is hydrated to the right text. Inline (not `useEffect`) per
	// fast-return-on-no-op preference.
	function _composerDraft(id: string | null): void {
		if (typeof window === "undefined" || !id) {
			_setComposerDraft("");
			return;
		}
		try {
			_setComposerDraft(
				window.localStorage.getItem(`savant.chat.draft.${id}`) ?? "",
			);
		} catch {
			_setComposerDraft("");
		}
	}

	return {
		sessions,
		currentSessionId,
		messages,
		loading,
		error,
		composerDraft,
		sidebarCollapsed,
		searchHits,
		searchQuery,
		switchSession,
		sendTurn,
		removeSession,
		createSession,
		setComposerDraft,
		toggleSidebar,
		search,
		clearSearch,
		refresh,
	};
}

// Event constant `CHAT_HISTORY_UPDATED_EVENT` is imported from
// `@/lib/chat-data` at the top of this file (single source of
// truth per ECHO Law 13). Do NOT re-declare it here -- TS2300
// duplicate-identifier error otherwise. The mock-ipc.ts mutators
// dispatch this event after every successful persist / delete;
// the storage-event listener above keeps the hook state synced.
