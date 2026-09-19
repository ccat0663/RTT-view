/**
 * Log-view state that is worth testing away from the DOM.
 *
 * The empty-state hint ("等待 RTT 数据…") is shown only while the log holds no
 * lines. Getting that wrong is quiet and easy: the hint used to be cleared from
 * a count read *before* the incoming batch was added, so the first lines of a
 * session appeared underneath a hint that stayed until the second batch arrived.
 */

/** What to do with the hint element, given the current state. */
export type HintAction = "add" | "remove" | "keep";

/**
 * Decide the hint's fate from whether it is on screen and how many lines are now
 * rendered. One function, so the two callers (appending, re-rendering) cannot
 * drift apart, and the "first batch" case is stated once instead of implied by
 * statement order.
 */
export function hintActionFor(hintPresent: boolean, renderedLines: number): HintAction {
  const wanted = renderedLines === 0;
  if (wanted === hintPresent) return "keep";
  return wanted ? "add" : "remove";
}
