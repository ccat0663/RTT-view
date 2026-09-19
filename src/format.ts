/**
 * Display formatting helpers, kept pure so they can be unit-tested.
 *
 * These deliberately tolerate missing or non-finite input. The backend sends
 * numbers over IPC, and a single renamed/missing field used to render as
 * `NaN MB/s` or `undefined 行` in the status bar; a formatter should degrade to
 * something readable instead of leaking JavaScript's numeric sentinels into the UI.
 */

/** Coerce anything to a finite, non-negative number, defaulting to 0. */
export function num(value: unknown): number {
  const n = typeof value === "number" ? value : Number(value);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** Byte-per-second rate → `0 B/s`, `1.5 KB/s`, `2.34 MB/s`. */
export function humanRate(bytesPerSec: unknown): string {
  const n = num(bytesPerSec);
  if (n < 1024) return `${Math.round(n)} B/s`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB/s`;
  return `${(n / 1024 / 1024).toFixed(2)} MB/s`;
}

/** Same as {@link humanRate} without the `/s` suffix, for saved-byte totals. */
export function humanSize(bytes: unknown): string {
  return humanRate(bytes).replace("/s", "");
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

/**
 * Local calendar date of a host receive time: `2026-09-15`.
 *
 * Local, not UTC: the point of these timestamps is to line the log up with what
 * the engineer saw on their own screen when the target printed.
 */
export function formatDate(tsMs: number): string {
  const d = new Date(tsMs);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Local wall-clock time of a host receive time, milliseconds included: `22:20:33.123`. */
export function formatTime(tsMs: number): string {
  const d = new Date(tsMs);
  return (
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.` +
    `${pad(d.getMilliseconds(), 3)}`
  );
}

/**
 * The timestamp prefix, from the two display toggles.
 *
 * Both on gives the historical `2026-09-15 22:20:33.123`; date alone, or time
 * alone, gives just that part; both off gives the empty string, so the caller can
 * use the result being empty as "render no timestamp at all". The parts are
 * derived from the same `Date`, so the two toggles can never disagree about the
 * day boundary.
 */
export function formatTimestamp(tsMs: number, showDate: boolean, showTime: boolean): string {
  const parts: string[] = [];
  if (showDate) parts.push(formatDate(tsMs));
  if (showTime) parts.push(formatTime(tsMs));
  return parts.join(" ");
}
