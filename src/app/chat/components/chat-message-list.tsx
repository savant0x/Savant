"use client";

// FID-029 §Step 6 component 3/5 — chat-message-list.tsx.
//
// Message rendering with `data-message-id` + 3s yellow highlight on
// search-result click (per FID §Suggestions for Improvement D).

import { useEffect, useRef } from "react";
import { useChatHistory } from "@/lib/hooks/use-chat-history";
import { formatRelativeTime } from "@/lib/format-relative-time";
import type { ChatMessage } from "@/lib/chat-data";

export function ChatMessageList() {
	const { messages, searchHits } = useChatHistory();
	const listRef = useRef<HTMLDivElement>(null);
	// Auto-scroll on new messages.
	useEffect(() => {
		listRef.current?.scrollTo({
			top: listRef.current.scrollHeight,
			behavior: "smooth",
		});
	}, [messages.length]);

	// 3s yellow highlight when a search-result is clicked.
	useEffect(() => {
		if (searchHits.length === 0) return;
		const first = searchHits[0];
		const el = listRef.current?.querySelector(
			`[data-message-id="${first.message_id}"]`,
		);
		if (!el) return;
		el.classList.add("bg-accent/20");
		const t = window.setTimeout(
			() => el.classList.remove("bg-accent/20"),
			3000,
		);
		return () => window.clearTimeout(t);
	}, [searchHits]);

	return (
		<div
			ref={listRef}
			className="flex-1 overflow-y-auto pr-2"
			aria-label="Chat history"
		>
			{messages.length === 0 ? (
				<div className="flex h-full items-center justify-center">
					<p className="font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-muted">
						Empty session \u2014 send a message to begin.
					</p>
				</div>
			) : (
				<ul className="flex flex-col gap-3">
					{messages.map((m) => (
						<MessageRow key={m.id} message={m} />
					))}
				</ul>
			)}
		</div>
	);
}

function MessageRow({ message }: { message: ChatMessage }) {
	const isUser = message.role === "user";
	return (
		<li
			data-message-id={message.id}
			className={
				isUser
					? "ml-auto max-w-[80%] rounded-md border border-default/40 bg-gradient-to-br from-surface/50 to-surface/30 px-4 py-3 transition-colors"
					: "mr-auto max-w-[80%] rounded-md border border-accent/40 bg-gradient-to-br from-accent/10 to-accent/5 px-4 py-3 shadow-[0_0_12px_-8px_var(--accent)] transition-colors"
			}
		>
			<div className="mb-1.5 flex items-center justify-between gap-2">
				<span className="font-mono text-[9px] font-semibold uppercase tracking-[0.25em] text-muted">
					{isUser ? "You" : "Savant"}
				</span>
				<span
					className="font-mono text-[9px] uppercase tracking-[0.25em] text-muted"
					title={new Date(message.ts).toISOString()}
				>
					{formatRelativeTime(message.ts)}
				</span>
			</div>
			<p className="whitespace-pre-wrap text-sm text-foreground">
				{message.content}
			</p>
		</li>
	);
}
