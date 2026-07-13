// src/lib/format-relative-time.ts
//
// Pure function (FID-006 v3 v2 reopen follow-up): format a Date or
// Unix epoch milliseconds as a human-readable relative time string.
// Replaces the previous RatingBox display of `HH:MM:SSZ` + date
// hint with "2 min ago" / "3h ago" / "yesterday" / "2026-07-10"
// depending on age. Per ECHO Law 13 (utility-first) so the same
// formatter can be reused for any future "X ago" UI (e.g., chat
// history timestamps, LEARNINGS entries, evolution log).
//
// No external dependencies — uses `Date.now()` and the built-in
// `Date` constructor. Pure function, no React or DOM dependencies.
// Unit-testable via the `now` parameter (no `vi.useFakeTimers()`
// needed).

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;

/**
 * Format a date/timestamp as a human-readable relative time string.
 *
 * **Past thresholds** (relative to `now`):
 *   - < 1 min   → `"just now"` (covers past-clock-skew edge case)
 *   - < 1 hour  → `"5 min ago"` (Math.floor minutes)
 *   - < 1 day   → `"3h ago"` (Math.floor hours)
 *   - 1-2 days  → `"yesterday"`
 *   - 2-7 days  → `"3 days ago"` (Math.floor days)
 *   - ≥ 7 days  → ISO date `"2026-07-05"` (YAML-friendly, locale-free)
 *
 * **Future thresholds** (e.g., `expires_at`, scheduled events):
 *   - < 1 min   → `"in a moment"`
 *   - < 1 hour  → `"in 5 min"`
 *   - < 1 day   → `"in 3h"`
 *   - 1-2 days  → `"tomorrow"`
 *   - 2-7 days  → `"in 3 days"`
 *   - ≥ 7 days  → ISO date `"2026-07-19"`
 *
 * Invalid input (`NaN` timestamp) returns `"—"` so the UI shows a
 * visible placeholder rather than crashing or showing "NaN min ago".
 *
 * @param input  Date instance or Unix epoch milliseconds.
 * @param now    Reference "now" (defaults to `Date.now()`). Exposed
 *               for testing so tests can pass a fixed `now` value
 *               without monkey-patching `Date.now()`.
 *
 * @example
 *   const now = Date.parse("2026-07-12T12:00:00Z");
 *   formatRelativeTime(now - 30_000, now)         // "just now"
 *   formatRelativeTime(now - 5 * MINUTE, now)     // "5 min ago"
 *   formatRelativeTime(now - 3 * HOUR, now)       // "3h ago"
 *   formatRelativeTime(now - 1 * DAY, now)        // "yesterday"
 *   formatRelativeTime(now - 3 * DAY, now)        // "3 days ago"
 *   formatRelativeTime(now - 30 * DAY, now)       // "2026-06-12"
 *   formatRelativeTime(now + 5 * MINUTE, now)     // "in 5 min"
 *   formatRelativeTime(now + 3 * HOUR, now)       // "in 3h"
 *   formatRelativeTime(now + 1 * DAY, now)        // "tomorrow"
 *   formatRelativeTime(new Date("invalid"), now)  // "—"
 */
export function formatRelativeTime(
  input: Date | number,
  now: number = Date.now(),
): string {
  const ts = typeof input === "number" ? input : input.getTime();
  if (Number.isNaN(ts)) return "—";
  const diff = now - ts;

  // Future timestamps (e.g., expires_at, scheduled events). Distinct
  // from the "just now" branch so the user sees "in 5 min" rather
  // than negative values like "-5 min ago" or a misleading "just now".
  if (diff < 0) {
    const future = -diff;
    if (future < MINUTE) return "in a moment";
    if (future < HOUR) return `in ${Math.floor(future / MINUTE)} min`;
    if (future < DAY) return `in ${Math.floor(future / HOUR)}h`;
    if (future < 2 * DAY) return "tomorrow";
    if (future < WEEK) return `in ${Math.floor(future / DAY)} days`;
    return new Date(ts).toISOString().slice(0, 10);
  }
  if (diff < MINUTE) return "just now";
  if (diff < HOUR) {
    const minutes = Math.floor(diff / MINUTE);
    return `${minutes} min ago`;
  }
  if (diff < DAY) {
    const hours = Math.floor(diff / HOUR);
    return `${hours}h ago`;
  }
  if (diff < 2 * DAY) return "yesterday";
  if (diff < WEEK) {
    const days = Math.floor(diff / DAY);
    return `${days} days ago`;
  }
  // > 1 week → ISO date (locale-free, YAML-friendly, sortable).
  return new Date(ts).toISOString().slice(0, 10);
}

/**
 * FID-017 — Format an ISO-8601 timestamp as a full date+time string
 * for use as a reflection entry header. Format: "YYYY-MM-DD HH:MM:SS UTC"
 * — locale-free, sortable, matches the `## YYYY-MM-DD HH:MM:SS UTC`
 * header format used in workspace-savant/REFLECTIONS.md (per
 * `parseReflectionsMd` in use-reflections.ts).
 *
 * The format is deliberately ISO-style rather than human-friendly
 * ("Jul 13, 2026 · 12:34 PM") so:
 * - Diary entries look like a journal, not a chat timestamp
 * - The format matches the underlying file (consistency)
 * - Sort order is the visual order
 * - No locale dependency (the user might change browser locale)
 *
 * Invalid input (`NaN` timestamp) returns "—" so the UI shows a
 * visible placeholder rather than crashing.
 *
 * @example
 *   formatFullTimestamp("2026-07-13T12:34:56.789Z")  // "2026-07-13 12:34:56 UTC"
 *   formatFullTimestamp(new Date("invalid"))         // "—"
 */
export function formatFullTimestamp(input: Date | number | string): string {
  // Narrow the union explicitly so TS knows when .getTime() is
  // available (Date only) vs when the value is already an epoch
  // number vs a stringified timestamp. The three-branch pattern
  // beats chained ternaries here because each branch needs different
  // narrowing logic.
  const ts: number =
    input instanceof Date
      ? input.getTime()
      : typeof input === "string"
        ? Date.parse(input)
        : input;
  if (Number.isNaN(ts)) return "—";
  const d = new Date(ts);
  // YYYY-MM-DD HH:MM:SS UTC — pad each component to 2 digits.
  const pad = (n: number): string => String(n).padStart(2, "0");
  return (
    `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ` +
    `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())} UTC`
  );
}
