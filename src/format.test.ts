import { test } from "node:test";
import assert from "node:assert/strict";
import { formatDate, formatTime, formatTimestamp, humanRate, humanSize, num } from "./format.ts";

test("humanRate formats bytes, KB and MB", () => {
  assert.equal(humanRate(0), "0 B/s");
  assert.equal(humanRate(512), "512 B/s");
  assert.equal(humanRate(1536), "1.5 KB/s");
  assert.equal(humanRate(1024 * 1024), "1.00 MB/s");
  assert.equal(humanRate(2.5 * 1024 * 1024), "2.50 MB/s");
});

// Regression: a missing/renamed backend field produced "NaN MB/s" in the status
// bar because undefined failed every comparison and fell through to the MB branch.
test("humanRate never emits NaN or undefined for bad input", () => {
  for (const bad of [undefined, null, NaN, Infinity, -Infinity, "", "abc", {}]) {
    const out = humanRate(bad);
    assert.ok(!/NaN|undefined|Infinity/.test(out), `bad output for ${String(bad)}: ${out}`);
    assert.equal(out, "0 B/s");
  }
  assert.equal(humanRate(-5), "0 B/s");
  // Numeric strings are accepted, since JSON round-trips may hand us one.
  assert.equal(humanRate("2048"), "2.0 KB/s");
});

test("humanSize drops the per-second suffix", () => {
  assert.equal(humanSize(0), "0 B");
  assert.equal(humanSize(2048), "2.0 KB");
  assert.equal(humanSize(undefined), "0 B");
});

test("num coerces non-finite input to 0", () => {
  assert.equal(num(42), 42);
  assert.equal(num("42"), 42);
  assert.equal(num(undefined), 0);
  assert.equal(num(NaN), 0);
  assert.equal(num(Infinity), 0);
  assert.equal(num(null), 0);
});

// A local Date built from components, so the expected strings hold in any timezone.
const TS = new Date(2026, 8, 15, 22, 20, 33, 123).getTime();

test("formatDate renders the local calendar date", () => {
  assert.equal(formatDate(TS), "2026-09-15");
});

test("formatTime renders the local wall clock with milliseconds", () => {
  assert.equal(formatTime(TS), "22:20:33.123");
});

test("formatTime pads every field, including midnight", () => {
  const midnight = new Date(2026, 0, 2, 0, 0, 0, 7).getTime();
  assert.equal(formatTime(midnight), "00:00:00.007");
});

test("both toggles on reproduce the original combined stamp", () => {
  assert.equal(formatTimestamp(TS, true, true), "2026-09-15 22:20:33.123");
});

test("each toggle alone renders only its own part", () => {
  assert.equal(formatTimestamp(TS, true, false), "2026-09-15");
  assert.equal(formatTimestamp(TS, false, true), "22:20:33.123");
});

test("both toggles off render nothing at all", () => {
  // Empty is the signal to skip the timestamp span entirely, so it must be "".
  assert.equal(formatTimestamp(TS, false, false), "");
});
