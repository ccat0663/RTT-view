/**
 * The log view's font-size choices.
 *
 * Five steps: the size the log has always used (the default), one step below it,
 * and three above — so the default sits near the small end and most of the range
 * is available for making dense output readable. Every value is a multiple of
 * 0.5px, which is exactly representable in binary floating point, so a value that
 * round-trips through the settings file compares equal to its table entry.
 */

/** Ascending steps, in px. Index 1 is the original/default size. */
export const LOG_FONT_SIZES = [11.5, 12.5, 14, 15.5, 17] as const;

/** The log's historical size: 12.5px (see `.log` in styles.css). */
export const DEFAULT_LOG_FONT_SIZE = 12.5;

/**
 * Coerce a persisted value to one of {@link LOG_FONT_SIZES}.
 *
 * Anything off the scale — a missing field, an older file, a hand-edited number —
 * snaps to the nearest step rather than being rejected, so a settings file can
 * never leave the log at an unreadable size. Non-finite input falls back to the
 * default.
 */
export function normalizeLogFontSize(value: unknown): number {
  // Only a real number, or a string that actually holds one, counts as a size.
  // `Number("")` and `Number(null)` are both 0 — finite, so a plain coercion
  // would snap an absent value to the *smallest* step instead of the default.
  let n: number;
  if (typeof value === "number") n = value;
  else if (typeof value === "string" && value.trim() !== "") n = Number(value);
  else return DEFAULT_LOG_FONT_SIZE;
  if (!Number.isFinite(n)) return DEFAULT_LOG_FONT_SIZE;
  let best: number = LOG_FONT_SIZES[0];
  for (const size of LOG_FONT_SIZES) {
    if (Math.abs(size - n) < Math.abs(best - n)) best = size;
  }
  return best;
}

/** Option text for a step, marking the default so it is identifiable in the list. */
export function logFontLabel(size: number): string {
  const text = String(size);
  return size === DEFAULT_LOG_FONT_SIZE ? `${text}（默认）` : text;
}
