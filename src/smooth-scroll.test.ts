import { test } from "node:test";
import assert from "node:assert/strict";
import {
  atBottom,
  classifyScrollEvent,
  followStep,
  isOwnScrollEvent,
  OWN_SCROLL_TOLERANCE_PX,
  PIN_EPSILON_PX,
} from "./smooth-scroll.ts";

const FRAME = 1000 / 60; // 16.67 ms

test("atBottom treats content that fits as at the bottom", () => {
  assert.equal(atBottom(0, 500, 500), true);
  assert.equal(atBottom(0, 400, 500), true);
});

test("atBottom uses the epsilon band", () => {
  const h = 1000;
  const v = 400; // max scrollTop = 600
  assert.equal(atBottom(600, h, v), true);
  assert.equal(atBottom(600 - PIN_EPSILON_PX, h, v), true);
  assert.equal(atBottom(600 - PIN_EPSILON_PX - 1, h, v), false);
  assert.equal(atBottom(0, h, v), false);
});

test("isOwnScrollEvent tolerates sub-pixel drift but not real movement", () => {
  assert.equal(isOwnScrollEvent(22, 22), true);
  assert.equal(isOwnScrollEvent(22 + OWN_SCROLL_TOLERANCE_PX, 22), true);
  assert.equal(isOwnScrollEvent(22 + OWN_SCROLL_TOLERANCE_PX + 0.5, 22), false);
  assert.equal(isOwnScrollEvent(0, null), false);
});

test("our own follow write is not mistaken for a latched user scroll", () => {
  // The first burst that overflows the viewport: 100px of new lines land, and the
  // ease has written 22px of it, leaving the view 78px above the tail. An input
  // event latched while the log was still short used to be attached to this
  // event, and since 78px is outside the at-bottom band, auto-scroll switched
  // itself off at the exact moment output began to overflow.
  assert.equal(classifyScrollEvent(22, 22, true, false), "ignore");
});

test("a real scroll away from the tail stops the follow", () => {
  assert.equal(classifyScrollEvent(0, 22, true, false), "stop");
});

test("a scroll with no input behind it is ignored", () => {
  // Bookkeeping events (content growth, trimming, our own snaps) must not stop it.
  assert.equal(classifyScrollEvent(120, 22, false, false), "ignore");
});

test("a user scroll that lands at the tail keeps the follow", () => {
  assert.equal(classifyScrollEvent(500, 22, true, true), "keep");
});

test("before anything was written, the offset can only be the user's", () => {
  assert.equal(classifyScrollEvent(40, null, true, false), "stop");
  assert.equal(classifyScrollEvent(40, null, false, true), "ignore");
});

test("followStep stays put when already there", () => {
  assert.equal(followStep(0, 700, FRAME), 0);
  assert.equal(followStep(-5, 700, FRAME), 0);
});

test("followStep never overshoots the remaining distance", () => {
  for (const remaining of [0.5, 1, 2, 10, 100, 1000, 100000]) {
    for (const dt of [0, 4, FRAME, 33, 64]) {
      const step = followStep(remaining, 700, dt);
      assert.ok(step <= remaining, `step ${step} > remaining ${remaining} (dt ${dt})`);
      assert.ok(step >= 0);
    }
  }
});

test("followStep moves nothing when no time has elapsed", () => {
  assert.equal(followStep(500, 700, 0), 0);
});

test("followStep advances at least a little so a slow trickle still moves", () => {
  // A 4px gap at 60 Hz: 0.22 * 4 = 0.88, lifted to the 2px floor.
  assert.equal(followStep(4, 700, FRAME), 2);
  assert.equal(followStep(1, 700, FRAME), 1); // clamped by remaining
});

test("followStep is monotonic in dt for a given gap", () => {
  // A longer frame must not move less than a shorter one (constant speed look).
  const gap = 400;
  let prev = 0;
  for (const dt of [4, 8, 16.7, 33, 50, 64]) {
    const step = followStep(gap, 700, dt);
    assert.ok(step >= prev, `dt ${dt}: ${step} < previous ${prev}`);
    prev = step;
  }
});

test("followStep gives the same pixels-per-second across frame rates", () => {
  // The point of dt scaling: 60 Hz and 30 Hz should cover a similar distance in
  // the same wall-clock time.
  const gap = 200;
  const at60 = followStep(gap, 700, FRAME) * 60; // px per second at 60 fps
  const at30 = followStep(gap, 700, 33.3) * 30; // px per second at ~30 fps
  const ratio = at30 / at60;
  assert.ok(ratio > 0.75 && ratio < 1.33, `speed ratio ${ratio.toFixed(2)} too far from 1`);
});

test("followStep caps speed for a large backlog", () => {
  const viewport = 700;
  const step = followStep(1_000_000, viewport, FRAME);
  // Cap is max(28, viewport/2) per frame at 60 Hz.
  assert.equal(step, Math.max(28, viewport / 2));
});

test("followStep clamps dt so a stall cannot teleport", () => {
  // A 5-second stall is treated as at most 64 ms, so the step stays bounded.
  const stalled = followStep(1_000_000, 700, 5000);
  const capped = followStep(1_000_000, 700, 64);
  assert.equal(stalled, capped);
});

test("repeated frames converge on the target without oscillating", () => {
  let scrollTop = 0;
  const target = 500;
  const viewport = 400;
  let frames = 0;
  while (frames < 300) {
    const remaining = target - scrollTop;
    if (remaining <= 0) break;
    scrollTop += followStep(remaining, viewport, FRAME);
    frames++;
  }
  assert.equal(scrollTop, target);
  assert.ok(frames < 120, `took ${frames} frames to converge`);
});

test("a steady stream of arrivals yields steady per-second motion", () => {
  // Simulate: data adds `arrival` px per frame; the follow loop should settle at
  // roughly that rate, with no frame moving dramatically more than another.
  const arrivalPerFrame = 120;
  const viewport = 700;
  let scrollTop = 0;
  let contentHeight = 0;
  const steps: number[] = [];
  for (let f = 0; f < 120; f++) {
    contentHeight += arrivalPerFrame;
    const remaining = contentHeight - viewport - scrollTop;
    const step = followStep(remaining, viewport, FRAME);
    scrollTop += step;
    if (f > 20) steps.push(step); // ignore the ramp-up
  }
  const mean = steps.reduce((a, b) => a + b, 0) / steps.length;
  const sd = Math.sqrt(steps.reduce((a, b) => a + (b - mean) ** 2, 0) / steps.length);
  // Steady-state steps should be close to the arrival rate, with low spread.
  assert.ok(Math.abs(mean - arrivalPerFrame) < 10, `mean step ${mean} vs arrival ${arrivalPerFrame}`);
  assert.ok(sd < 5, `step spread ${sd} too high`);
});
