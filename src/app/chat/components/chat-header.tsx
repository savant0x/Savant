"use client";

// FID-029 §Step 6 component 1/5 — chat-header.tsx.
//
// Page header with sidebar toggle + current-session title (collapsed
// cue per FID §Verifier Pass MEDIUM #5).

import { useChatHistory } from "@/lib/hooks/use-chat-history";

export function ChatHeader() {
	const {
		currentSessionId,
		sessions,
		sidebarCollapsed,
		toggleSidebar,
	} = useChatHistory();
	const active = sessions.find((s) => s.session_id === currentSessionId);
	return (
		<div className="flex items-center justify-between gap-3 border-b border-default/40 pb-3">
			<div className="flex items-center gap-2">
				<button
					type="button"
					onClick={toggleSidebar}
					className="rounded-md border border-default/60 px-2.5 py-1.5 font-mono text-[10px] uppercase tracking-[0.2em] text-muted transition-colors hover:border-accent hover:text-accent"
					aria-label={sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
					title={sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
				>
					<i
						className={`fas ${sidebarCollapsed ? "fa-bars" : "fa-xmark"}`}
						aria-hidden
					/>
				</button>
				<h2 className="truncate font-mono text-xs font-semibold uppercase tracking-[0.18em] text-foreground">
					{active ? active.title : "No session"}
				</h2>
			</div>
			<span className="font-mono text-[9px] uppercase tracking-[0.25em] text-muted">
				{currentSessionId
					? `session ${currentSessionId.slice(-6)}`
					: ""}
			</span>
		</div>
	);
}
