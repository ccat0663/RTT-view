import { test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_LOG_FONT_SIZE,
  LOG_FONT_SIZES,
  logFontLabel,
  normalizeLogFontSize,
} from "./log-font.ts";

test("the scale has five steps: one below the default and three above", () => {
  assert.equal(LOG_FONT_SIZES.length, 5);
  const idx = LOG_FONT_SIZES.indexOf(DEFAULT_LOG_FONT_SIZE);
  assert.equal(idx, 1, "the default must be the second step");
  assert.equal(LOG_FONT_SIZES.length - 1 - idx, 3, "three steps above the default");
});

test("the default matches the size the log already used", () => {
  // Kept in step with `.log { font-size: 12.5px }` in styles.css.
  assert.equal(DEFAULT_LOG_FONT_SIZE, 12.5);
});

test("the scale is strictly ascending", () => {
  for (let i = 1; i < LOG_FONT_SIZES.length; i++) {
    assert.ok(LOG_FONT_SIZES[i] > LOG_FONT_SIZES[i - 1], `step ${i} not ascending`);
  }
});

test("every step survives a settings round-trip unchanged", () => {
  // A stored value must land on its own entry, not snap to a neighbour.
  for (const size of LOG_FONT_SIZES) {
    assert.equal(normalizeLogFontSize(size), size);
    assert.equal(normalizeLogFontSize(Number(size.toFixed(1))), size);
  }
});

test("a value off the scale snaps to the nearest step", () => {
  assert.equal(normalizeLogFontSize(12.6), 12.5);
  assert.equal(normalizeLogFontSize(13.9), 14);
  assert.equal(normalizeLogFontSize(100), 17);
  assert.equal(normalizeLogFontSize(1), 11.5);
});

test("missing or non-numeric input falls back to the default", () => {
  // "" and null both coerce to 0, so they must be rejected explicitly rather than
  // snapping to the smallest step.
  for (const bad of [undefined, null, NaN, Infinity, -Infinity, "", "  ", "abc", {}, true]) {
    assert.equal(normalizeLogFontSize(bad), DEFAULT_LOG_FONT_SIZE, `bad input: ${String(bad)}`);
  }
  // Numeric strings are accepted, since JSON round-trips may hand us one.
  assert.equal(normalizeLogFontSize("14"), 14);
});

test("only the default step is labelled as such", () => {
  assert.equal(logFontLabel(DEFAULT_LOG_FONT_SIZE), "12.5（默认）");
  assert.equal(logFontLabel(11.5), "11.5");
  assert.equal(logFontLabel(17), "17");
});
