"use client";

// FID-029 §Step 6 component 2/5 — chat-sidebar.tsx.
//
// Session list + search bar + per-session delete + "+ New" button.

import { useChatHistory } from "@/lib/hooks/use-chat-history";

export function ChatSidebar() {
	const {
		sessions,
		currentSessionId,
		switchSession,
		removeSession,
		createSession,
		search,
		searchQuery,
	} = useChatHistory();
	return (
		<aside
			className="flex w-72 shrink-0 flex-col gap-3 border-r border-default/40 pr-3"
			aria-label="Chat sessions"
		>
			<button
				type="button"
				onClick={() => void createSession()}
				className="flex items-center justify-center gap-2 rounded-md border border-accent bg-accent/10 px-3 py-2 font-mono text-[10px] font-semibold uppercase tracking-[0.2em] text-accent transition-colors hover:bg-accent/20"
			>
				<i className="fas fa-plus" aria-hidden /> New session
			</button>
			<input
				type="text"
				value={searchQuery}
				onChange={(e) => void search(e.target.value)}
				placeholder="Search… (Cmd/Ctrl+K)"
				className="w-full rounded-md border border-default/40 bg-surface/30 px-3 py-1.5 font-mono text-[11px] text-foreground placeholder:text-muted focus:border-accent focus:outline-none"
				aria-label="Search chat history"
			/>
			<ul className="flex flex-col gap-1 overflow-y-auto">
				{sessions.map((s) => (
					<li
						key={s.session_id}
						className={
							s.session_id === currentSessionId
								? "flex items-center justify-between gap-2 rounded-md border border-accent/60 bg-accent/10 px-3 py-2"
								: "flex items-center justify-between gap-2 rounded-md border border-default/40 px-3 py-2 transition-colors hover:border-default/60"
						}
					>
						<button
							type="button"
							onClick={() => void switchSession(s.session_id)}
							className="flex min-w-0 flex-1 flex-col items-start gap-0.5 text-left"
						>
							<span className="truncate font-mono text-[11px] text-foreground">
								{s.title}
							</span>
							<span className="font-mono text-[9px] uppercase tracking-[0.2em] text-muted">
								{s.turn_count} turn{s.turn_count === 1 ? "" : "s"}
							</span>
						</button>
						<button
							type="button"
							onClick={() => void removeSession(s.session_id)}
							className="rounded-md border border-default/40 px-2 py-1 font-mono text-[9px] uppercase tracking-[0.2em] text-muted transition-colors hover:border-danger hover:text-danger"
							aria-label={`Delete ${s.title}`}
							title="Delete session"
						>
							<i className="fas fa-trash" aria-hidden />
						</button>
					</li>
				))}
				{sessions.length === 0 && (
					<li className="px-3 py-4 text-center font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
						No sessions yet
					</li>
				)}
			</ul>
		</aside>
	);
}
