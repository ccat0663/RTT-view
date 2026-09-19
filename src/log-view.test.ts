import { test } from "node:test";
import assert from "node:assert/strict";
import { hintActionFor } from "./log-view.ts";

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
