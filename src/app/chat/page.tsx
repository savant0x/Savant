"use client";

// Chat with Savant \u2014 utility-first, two-tier credential architecture.
//
// FID-029 §Step 7 \u2014 rewrite of the chat page to wire
// `useChatHistory()` + the 5 chat sub-components (FID-029 \u00a7Step 6).
//
// This page is the thin composer (~150 lines per spec). Layout:
// <div className="flex h-full"> with a collapsible left-rail drawer
// (Sidebar component, hidden when `sidebarCollapsed`) + a main chat
// column. The OQ-3 strict-blocking modal pattern is preserved at the
// top (LS_DERIVED missing \u2192 <dialog> + Retry button). The Cmd/Ctrl+K
// listener is bound INSIDE the `if (derived)` branch per FID \u00a7Verifier
// Pass HIGH #3 \u2014 the shortcut must NOT fire when the blocking modal
// is up.
//
// The OpenRouter fetch logic is preserved from the prior version:
// derived.key (NOT the master) is the Authorization bearer on the
// outbound HTTP POST /v1/chat/completions.

import { useCallback, useEffect, useState, type KeyboardEvent } from "react";
import { Card } from "@heroui/react";
import { DashboardShell } from "@/components/dashboard-shell";
import {
	useLoadedConfig,
	LS_DERIVED,
	parseDerivedSession,
} from "@/lib/hooks/use-loaded-config";
import { useDerivedRotation } from "@/lib/hooks/use-derived-rotation";
import {
	provisionSessionKey,
	type SessionKey,
} from "@/lib/ipc";
import { randomHex } from "@/lib/ids";
import { formatRelativeTime } from "@/lib/format-relative-time";
import { SOUL_PROMPT } from "@/lib/soul";
import { useChatHistory } from "@/lib/hooks/use-chat-history";
import { trimMessagesForContext } from "@/lib/chat-data";
import { ChatHeader } from "./components/chat-header";
import { ChatSidebar } from "./components/chat-sidebar";
import { ChatMessageList } from "./components/chat-message-list";
import { ChatSearchResults } from "./components/chat-search-results";
import { ChatComposer } from "./components/chat-composer";

const OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions";
const PROVIDER = "openrouter";
const DEFAULT_MODEL = "meta-llama/llama-3.3-70b-instruct:free";

type ProvisioningState = {
	attempts: number;
	lastStatus: number | null;
	lastError: string | null;
};

export default function ChatPage() {
	const [derived, setDerived] = useState<SessionKey | null>(null);
	const [model, setModel] = useState<string>(DEFAULT_MODEL);
	const [composerDraft, setComposerDraft] = useState<string>("");
	const [sending, setSending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [provisional, setProvisional] = useState<ProvisioningState>({
		attempts: 0,
		lastStatus: null,
		lastError: null,
	});
	const hook = useChatHistory();
	const {
		currentSessionId,
		messages,
		sidebarCollapsed,
		sendTurn,
	} = hook;

	// Mount the daily rotation hook (FID Step 17 \u2014 OQ-4 cron).
	useDerivedRotation();

	// Hydrate derived key on mount + cross-tab sync via `storage` event.
	useEffect(() => {
		if (typeof window === "undefined") return;
		setDerived(parseDerivedSession(window.localStorage.getItem(LS_DERIVED)));
		const onStorage = (e: StorageEvent): void => {
			if (e.key === LS_DERIVED) {
				setDerived(parseDerivedSession(e.newValue));
			}
		};
		window.addEventListener("storage", onStorage);
		return () => window.removeEventListener("storage", onStorage);
	}, []);

	const loaded = useLoadedConfig();
	useEffect(() => {
		if (loaded.modelId) setModel(loaded.modelId);
	}, [loaded.modelId]);

	// Sync hook's composerDraft into this page's mirror so the
	// ChatComposer reads via the hook + this page's send reads the
	// current value. One source of truth via the hook; the local
	// mirror exists only for the Enter-to-send handler closure.
	useEffect(() => {
		setComposerDraft(hook.composerDraft);
	}, [hook.composerDraft]);

	const retryProvisioning = useCallback(async (): Promise<void> => {
		setProvisional((p) => ({ ...p, attempts: p.attempts + 1 }));
		try {
			const fresh = await provisionSessionKey({
				profile: PROVIDER,
				agentName: `savant-${randomHex(8)}`,
			});
			if (typeof window !== "undefined") {
				window.localStorage.setItem(LS_DERIVED, JSON.stringify(fresh));
			}
			setDerived(fresh);
			setProvisional({
				attempts: 0,
				lastStatus: 201,
				lastError: null,
			});
		} catch (e) {
			const statusMatch = /\b(\d{3})\b/.exec(
				e instanceof Error ? e.message : String(e),
			);
			setProvisional({
				attempts: 0,
				lastStatus: statusMatch ? Number(statusMatch[1]) : null,
				lastError: e instanceof Error ? e.message : String(e),
			});
		}
	}, []);

	const send = useCallback(async (text: string): Promise<void> => {
		if (!derived || sending) return;
		setError(null);
		setSending(true);
		try {
			const trimmed = trimMessagesForContext(
				messages,
				40000,
			);
			const response = await fetch(OPENROUTER_URL, {
				method: "POST",
				headers: {
					Authorization: `Bearer ${derived.key}`,
					"Content-Type": "application/json",
					"HTTP-Referer":
						typeof window !== "undefined"
							? window.location.origin
							: "https://savant.local",
					"X-Title": "Savant",
				},
				body: JSON.stringify({
					model,
					messages: [
						{ role: "system", content: SOUL_PROMPT },
						...trimmed.map((m) => ({
							role: m.role,
							content: m.content,
						})),
						{ role: "user", content: text },
					],
				}),
			});
			if (!response.ok) {
				const body = await response.text();
				throw new Error(
					`OpenRouter ${response.status}: ${body.slice(0, 160)}`,
				);
			}
			const data = (await response.json()) as {
				choices?: Array<{ message?: { content?: string } }>;
			};
			const reply =
				data?.choices?.[0]?.message?.content ?? "(empty reply)";
			await sendTurn(text, reply);
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		} finally {
			setSending(false);
		}
	}, [derived, model, messages, sending, sendTurn]);

	// Cmd/Ctrl+K listener \u2014 INSIDE the `if (derived)` branch per
	// FID \u00a7Verifier Pass HIGH #3 (must NOT fire when the blocking
	// modal is up). Cross-platform modifier via `metaKey || ctrlKey`.
	useEffect(() => {
		if (!derived) return;
		const onKey = (e: globalThis.KeyboardEvent): void => {
			if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
				e.preventDefault();
				const input =
					document.querySelector<HTMLInputElement>(
						'input[aria-label="Search chat history"]',
					);
				input?.focus();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [derived]);

	// OQ-3 strict-blocking UI \u2014 if LS_DERIVED is missing or invalid,
	// no outbound fetch ever fires. The chat surface is replaced with
	// the same <dialog> modal + Retry button used in the prior version.
	if (!derived) {
		return (
			<DashboardShell>
				<div className="flex h-full items-center justify-center">
					<Card
						className="max-w-md p-8"
						role="dialog"
						aria-labelledby="provision-modal-title"
						aria-describedby="provision-modal-body"
						aria-live="polite"
					>
						<p className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
							Provisioning
						</p>
						<h2
							id="provision-modal-title"
							className="mt-2 font-mono text-base font-semibold uppercase tracking-[0.18em] text-foreground"
						>
							Provisioning Session Credentials
						</h2>
						<p
							id="provision-modal-body"
							className="mt-3 text-sm text-muted"
						>
							Waiting on{" "}
							<code className="rounded bg-surface px-1.5 py-0.5 font-mono text-[11px] text-accent">
								POST /v1/keys
							</code>{" "}
							to return 201. Last attempt status:{" "}
							<span className="font-mono">
								{provisional.lastStatus ?? "\u2014"}
							</span>
							.
						</p>
						{provisional.lastError && (
							<p
								className="mt-2 font-mono text-[10px] uppercase tracking-[0.2em] text-danger"
								role="status"
							>
								{provisional.lastError.slice(0, 160)}
							</p>
						)}
						<div className="mt-6 flex gap-3">
							<button
								type="button"
								onClick={() => void retryProvisioning()}
								className="flex items-center gap-2 rounded-md border border-accent bg-accent/15 px-4 py-2 font-mono text-[10px] uppercase tracking-[0.2em] text-accent transition-colors hover:bg-accent/25"
							>
								<i className="fas fa-arrows-rotate" aria-hidden />
								Retry
							</button>
							<a
								href="/settings"
								className="inline-flex items-center gap-2 rounded-md border border-default/60 px-4 py-2 font-mono text-[10px] uppercase tracking-[0.2em] text-muted no-underline transition-colors hover:border-accent hover:text-accent"
							>
								<i className="fas fa-gear" aria-hidden />
								Open Settings
							</a>
						</div>
					</Card>
				</div>
			</DashboardShell>
		);
	}

	return (
		<DashboardShell>
			<div className="flex h-full flex-col gap-3">
				<ChatHeader />
				<div
					className="flex h-full min-h-0 flex-1 gap-4"
					data-current-session={currentSessionId ?? ""}
				>
					{!sidebarCollapsed && <ChatSidebar />}
					<div className="flex min-w-0 flex-1 flex-col gap-3">
						<ChatMessageList />
						<ChatSearchResults />
						<ChatComposer onSend={send} disabled={sending} />
					</div>
				</div>
				{error && (
					<div className="rounded-md border border-danger/40 bg-danger/10 px-4 py-3">
						<p className="mb-1 font-mono text-[9px] font-semibold uppercase tracking-[0.25em] text-danger">
							Error
						</p>
						<p className="font-mono text-xs text-foreground">
							{error}
						</p>
					</div>
				)}
			</div>
		</DashboardShell>
	);
}
