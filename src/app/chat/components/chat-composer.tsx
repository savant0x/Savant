"use client";

// FID-029 §Step 6 component 4/5 — chat-composer.tsx.
//
// Textarea + per-session draft (hooked into
// `savant.chat.draft.${currentSessionId}` via the hook's
// setComposerDraft mutator) + send handler + Clear link.
//
// Send: appends user message + assistant placeholder (the actual
// assistant text is filled in by the page.tsx-level OpenRouter
// fetch; here we only emit the user message). See §Step 7 wiring
// notes.

import { type KeyboardEvent, useState, useEffect } from "react";
import { useChatHistory } from "@/lib/hooks/use-chat-history";

export type ChatComposerProps = {
	onSend: (text: string) => Promise<void> | void;
	disabled?: boolean;
};

export function ChatComposer({ onSend, disabled = false }: ChatComposerProps) {
	const { composerDraft, setComposerDraft } = useChatHistory();
	const [sending, setSending] = useState(false);
	const send = async (): Promise<void> => {
		const text = composerDraft.trim();
		if (!text || sending || disabled) return;
		setSending(true);
		try {
			await onSend(text);
			setComposerDraft("");
		} finally {
			setSending(false);
		}
	};
	const onKeyDown = (
		e: KeyboardEvent<HTMLTextAreaElement>,
	): void => {
		if (e.key === "Enter" && !e.shiftKey) {
			e.preventDefault();
			void send();
		}
	};
	// Reset sending when the disabled flag flips (page.tsx sets
	// `disabled` while OpenRouter is in flight).
	useEffect(() => {
		if (!disabled) setSending(false);
	}, [disabled]);
	return (
		<div className="flex items-end gap-3 border-t border-default/40 pt-4">
			<textarea
				value={composerDraft}
				onChange={(e) => setComposerDraft(e.target.value)}
				onKeyDown={onKeyDown}
				placeholder="Ask Savant\u2026 (Enter to send, Shift+Enter for newline)"
				disabled={sending || disabled}
				rows={2}
				className="flex-1 resize-none rounded-md border border-[color:var(--input-border-color)] bg-surface/30 px-3 py-2 font-mono text-sm text-foreground placeholder:text-muted focus:border-accent focus:outline-none disabled:opacity-50"
			/>
			<button
				type="button"
				onClick={() => void send()}
				disabled={!composerDraft.trim() || sending || disabled}
				className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-accent bg-accent/10 text-accent transition-colors hover:bg-accent/20 disabled:cursor-not-allowed disabled:opacity-40"
				aria-label="Send message"
				title="Send"
			>
				<i className="fas fa-paper-plane" aria-hidden />
			</button>
		</div>
	);
}
