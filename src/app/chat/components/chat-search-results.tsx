"use client";

// FID-029 \u00a7Step 6 component 5/5 \u2014 chat-search-results.tsx.
//
// Search-results tray (Cmd/Ctrl+K palette body) + empty state.
//
// The snippet-content slot is intentionally absent \u2014 per FID
// \u00a7Verifier Pass post-SHOULD-FIX LOW #3, we render session title +
// score + msg-id + offset. The matched-message-body snippet lands
// in FID-032 (Layer 3) when the DAEMON IPC bridge ships the search-
// substring engine result with the source text inline. Per ECHO Law 5,
// no placeholder content for production.

import { useChatHistory } from "@/lib/hooks/use-chat-history";

export function ChatSearchResults() {
	const { searchHits, searchQuery, clearSearch, sessions } =
		useChatHistory();
	if (searchHits.length > 0) {
		return (
			<div
				className="border-t border-default/40 pt-3"
				aria-label="Search results"
			>
				<ul className="flex flex-col gap-1.5">
					{searchHits.map((hit) => {
						const title =
							sessions.find(
								(s) => s.session_id === hit.session_id,
							)?.title ??
							`session ${hit.session_id.slice(-6)}`;
						return (
							<li
								key={`${hit.session_id}-${hit.message_id}`}
								className="rounded-md border border-default/40 px-3 py-2 transition-colors hover:border-accent/60"
							>
								<div className="mb-1 flex items-center justify-between text-[9px] uppercase tracking-[0.2em] text-muted">
									<span className="truncate font-mono">
										{title}
									</span>
									<span className="font-mono">
										score {(hit.score * 100).toFixed(0)}
									</span>
								</div>
								<p className="font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
									msg {hit.message_id.slice(-6)} \u00b7 offset{" "}
									{hit.match_offset}
								</p>
							</li>
						);
					})}
				</ul>
			</div>
		);
	}
	if (searchQuery.trim().length > 0) {
		return (
			<div className="border-t border-default/40 pt-3 text-center">
				<p className="mb-3 font-mono text-[10px] uppercase tracking-[0.2em] text-muted">
					No results for &lsquo;{searchQuery}&rsquo;
				</p>
				<button
					type="button"
					onClick={clearSearch}
					className="rounded-md border border-default/60 px-3 py-1.5 font-mono text-[10px] uppercase tracking-[0.2em] text-muted transition-colors hover:border-accent hover:text-accent"
				>
					Clear
				</button>
			</div>
		);
	}
	return null;
}
