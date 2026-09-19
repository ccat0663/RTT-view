/**
 * Smooth tail-following for the log view, as pure functions so the behaviour can
 * be unit-tested without a browser.
 *
 * Why this exists: RTT lines arrive in bursts (the backend flushes on a 30 ms
 * timer), and a log that prints at a steady rate therefore lands as an uneven
 * number of lines per burst. Writing `scrollTop = scrollHeight` on each burst
 * makes the view jump by a varying amount at an irregular cadence, which reads
 * as stutter. Approaching the target over several frames instead turns each
 * burst into continuous motion; the per-frame cap keeps a large backlog scrolling
 * steadily rather than snapping.
 */

/** Closest the view may get to the bottom and still count as "at the bottom". */
export const PIN_EPSILON_PX = 12;

/** True when the scroll offset is within {@link PIN_EPSILON_PX} of the newest line. */
export function atBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
): boolean {
  const maxScrollTop = scrollHeight - clientHeight;
  if (maxScrollTop <= 0) return true; // content fits: nothing to scroll
  return maxScrollTop - scrollTop <= PIN_EPSILON_PX;
}

/**
 * How far an event's offset may differ from the one we last wrote and still be
 * considered ours. Browsers report fractional offsets at non-100% zoom, so a
 * write can read back a hair off; real movement is at least a wheel notch.
 */
export const OWN_SCROLL_TOLERANCE_PX = 1;

/** True when this scroll offset is the one our own last write produced. */
export function isOwnScrollEvent(
  scrollTop: number,
  lastWritten: number | null,
): boolean {
  if (lastWritten === null) return false; // nothing written yet: it is the user's
  return Math.abs(scrollTop - lastWritten) <= OWN_SCROLL_TOLERANCE_PX;
}

/** What a scroll event means for tail-following. */
export type ScrollEventAction =
  /** Not a user action: leave the follow as it is. */
  | "ignore"
  /** The user scrolled back to the tail: resume following. */
  | "keep"
  /** The user scrolled away from the tail: stop following. */
  | "stop";

/**
 * Decide what a scroll event means, given the offset we last wrote ourselves
 * (`lastWritten`), whether real input happened recently (`userInputPending`), and
 * where the event left the viewport (`atBottom`).
 *
 * The log is scrolled both by the user and by us, and the browser reports both
 * the same way, so an event alone cannot be trusted as "the user took over".
 * Attributing it wrongly is how auto-scroll used to switch itself off on its own:
 * an input event latched while the log was still short (a wheel over it, or a
 * keypress after clicking into it) produced no scroll event to consume it, so it
 * was still pending when output first overflowed — and there it got attached to
 * our own first write, which lands mid-ease and therefore reads as "scrolled up".
 */
export function classifyScrollEvent(
  scrollTop: number,
  lastWritten: number | null,
  userInputPending: boolean,
  atBottom: boolean,
): ScrollEventAction {
  if (isOwnScrollEvent(scrollTop, lastWritten)) return "ignore";
  // Nothing the user did: bookkeeping events (growth, trimming) land here.
  if (!userInputPending) return "ignore";
  return atBottom ? "keep" : "stop";
}

/**
 * Distance to move this frame, given how far the tail is below the viewport.
 *
 * Motion is scaled by elapsed time, not by frame count. Frame times vary (the
 * DOM is thousands of nodes and reflows are not perfectly regular), so a fixed
 * per-frame step produces uneven speed — which reads as stutter even though the
 * viewport ends up in the right place. Using `dt` keeps the pixels-per-second
 * steady.
 *
 * * `remaining <= 0` → 0 (already there).
 * * A small distance still moves a little, so a slow trickle advances evenly
 *   instead of crawling by sub-pixel amounts.
 * * Otherwise approach the target exponentially, with the rate expressed per
 *   second, so a burst is spread over several frames.
 * * `dt` is clamped so a long stall (background tab, GC pause) cannot turn the
 *   next frame into a teleport.
 * * Never overshoots the remaining distance.
 */
export function followStep(
  remaining: number,
  viewport: number,
  dtMs: number,
): number {
  if (!(remaining > 0)) return 0;

  // Normalise to a 60 Hz frame, clamped so a stall cannot cause a big jump.
  const frames = Math.min(Math.max(dtMs, 0), 64) / (1000 / 60);

  // Per-second approach rate: ~18 doublings... expressed as a per-frame fraction
  // of 0.22 at 60 Hz, which closes 95% of a gap in ~12 frames.
  const FRACTION_PER_FRAME = 0.22;
  const fraction = 1 - Math.pow(1 - FRACTION_PER_FRAME, frames);

  // Speed limit, also time-scaled, so a large backlog scrolls at a readable rate.
  const maxStepPerFrame = Math.max(28, viewport / 2);
  const maxStep = maxStepPerFrame * frames;

  // Floor so a small trickle still visibly advances.
  const minStep = Math.min(2 * frames, remaining);

  return Math.min(remaining, maxStep, Math.max(minStep, remaining * fraction));
}
