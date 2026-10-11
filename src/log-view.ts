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

/**
 * Ctrl+A over these `<input type=...>` values is left to the browser, because the
 * element holds editable text whose select-all is meaningful. `number` counts:
 * its value is editable text even though the spinner looks different.
 *
 * Anything not listed here has no selectable text of its own — a checkbox, a
 * button — so Ctrl+A there is scoped to the log. Treating those as text fields
 * (as a plain "is it an INPUT?" check did) meant Ctrl+A right after ticking a
 * checkbox selected the whole page again, bringing back the original bug.
 */
const TEXT_INPUT_TYPES = new Set([
  "text",
  "search",
  "password",
  "email",
  "url",
  "tel",
  "number",
  "date",
  "datetime-local",
  "month",
  "week",
  "time",
]);

/**
 * Whether Ctrl+A should be left to the browser instead of being scoped to the log.
 *
 * Ctrl+A on a non-editable page selects the *whole document*, which on this layout
 * drags in the toolbar, the connection panel and the status bar — not what someone
 * pressing it over the log wants. So the app scopes it to the log itself, except
 * while focus is in a field that actually holds editable text, where native
 * select-all is correct and hijacking it would break editing.
 *
 * `inputType` is only consulted for `<input>`; an empty string means the attribute
 * was absent, which per the HTML spec defaults to `text`.
 */
export function ctrlABelongsToTextField(
  tagName: string,
  inputType: string,
  isContentEditable: boolean,
): boolean {
  if (isContentEditable) return true;
  const tag = tagName.toUpperCase();
  if (tag === "TEXTAREA") return true;
  if (tag === "INPUT") return TEXT_INPUT_TYPES.has(inputType.toLowerCase() || "text");
  return false;
}
