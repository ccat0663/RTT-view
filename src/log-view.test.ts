import { test } from "node:test";
import assert from "node:assert/strict";
import { ctrlABelongsToTextField, hintActionFor } from "./log-view.ts";

test("the first batch clears the hint in the same frame", () => {
  // The regression: with an empty log the hint is on screen, and appending the
  // first lines must drop it right away rather than on the next batch.
  assert.equal(hintActionFor(true, 1), "remove");
  assert.equal(hintActionFor(true, 40), "remove");
});

test("an empty log shows the hint", () => {
  assert.equal(hintActionFor(false, 0), "add");
});

test("matching states are left alone", () => {
  assert.equal(hintActionFor(false, 12), "keep"); // lines, no hint
  assert.equal(hintActionFor(true, 0), "keep"); // empty, hint already there
});

test("clearing brings the hint back", () => {
  assert.equal(hintActionFor(false, 0), "add");
});

test("a re-render from an empty history keeps a single hint", () => {
  // rerenderAll wipes the list itself, so the hint must be re-added once — and
  // never twice, which is why the count and not the DOM decides.
  assert.equal(hintActionFor(false, 0), "add");
  assert.equal(hintActionFor(true, 0), "keep");
});

// Ctrl+A scoping. The log's own keydown handler calls this to decide whether to
// intercept; returning true means "leave it to the browser".

test("Ctrl+A in a text field is left to the browser", () => {
  // Otherwise select-all inside an input the user is typing in would be broken.
  assert.equal(ctrlABelongsToTextField("INPUT", "text", false), true);
  assert.equal(ctrlABelongsToTextField("INPUT", "number", false), true); // spinner, but editable text
  assert.equal(ctrlABelongsToTextField("INPUT", "search", false), true);
  assert.equal(ctrlABelongsToTextField("TEXTAREA", "", false), true);
  assert.equal(ctrlABelongsToTextField("input", "text", false), true); // tagName casing varies
});

test("an <input> with no type attribute counts as text", () => {
  // The HTML spec defaults a missing type to text, and the log's handler passes ""
  // in that case.
  assert.equal(ctrlABelongsToTextField("INPUT", "", false), true);
});

test("Ctrl+A over non-text controls is taken over, so it selects the log", () => {
  // The regression this guards: a plain "is it an INPUT?" check treated a checkbox
  // as a text field, so Ctrl+A after ticking one selected the whole page again.
  assert.equal(ctrlABelongsToTextField("INPUT", "checkbox", false), false);
  assert.equal(ctrlABelongsToTextField("INPUT", "button", false), false);
  assert.equal(ctrlABelongsToTextField("INPUT", "radio", false), false);
  assert.equal(ctrlABelongsToTextField("INPUT", "range", false), false);
});

test("Ctrl+A over the log or the page is taken over", () => {
  // The log is a <main>, so it must NOT be treated as a text field, or the
  // whole-document selection bug would remain.
  assert.equal(ctrlABelongsToTextField("MAIN", "", false), false);
  assert.equal(ctrlABelongsToTextField("BODY", "", false), false);
  assert.equal(ctrlABelongsToTextField("DIV", "", false), false);
  assert.equal(ctrlABelongsToTextField("SELECT", "", false), false);
  assert.equal(ctrlABelongsToTextField("", "", false), false); // no focused element
});

test("contenteditable is treated as a text field", () => {
  assert.equal(ctrlABelongsToTextField("DIV", "", true), true);
});
