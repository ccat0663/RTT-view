import { test } from "node:test";
import assert from "node:assert/strict";
import { badgeForState, stateMeansConnected, stateEndsSession } from "./connection-state.ts";

/**
 * Every `state` the backend can emit. Kept here as a literal list so adding a new
 * one without deciding its badge shows up as a test failure — the bug this
 * guards against (a completed test leaving the badge on "测试中") was exactly a
 * state that nobody had classified.
 */
const ALL_BACKEND_STATES = [
  "connecting",
  "connected",
  "disconnected",
  "testing",
  "testOk",
  "resetting",
  "reset",
  "devices",
  "probes",
  "deviceOk",
  "saving",
  "saved",
] as const;

test("every backend state is classified (no silent fallthrough)", () => {
  const busy = ALL_BACKEND_STATES.filter((s) => badgeForState(s)?.kind === "busy");
  const terminal = ALL_BACKEND_STATES.filter(
    (s) => badgeForState(s)?.kind === "on" || badgeForState(s)?.kind === "off",
  );
  const neutral = ALL_BACKEND_STATES.filter((s) => badgeForState(s) === null);

  // A busy state must have a matching terminal state, otherwise the badge can
  // never leave the busy appearance.
  for (const b of busy) {
    const successor =
      b === "connecting" ? "connected" : b === "testing" ? "testOk" : "reset";
    assert.ok(
      ALL_BACKEND_STATES.includes(successor as never),
      `busy state ${b} has no terminal successor ${successor}`,
    );
    assert.notEqual(badgeForState(successor)?.kind, "busy", `${successor} must not be busy`);
  }

  assert.ok(busy.length > 0 && terminal.length > 0);
  // Neutral notifications must not be busy (they would strand the badge too).
  for (const n of neutral) assert.notEqual(badgeForState(n)?.kind, "busy");
});

test("every busy state has an explicit badge", () => {
  assert.deepEqual(badgeForState("connecting"), { text: "连接中", kind: "busy" });
  assert.deepEqual(badgeForState("testing"), { text: "测试中", kind: "busy" });
  assert.deepEqual(badgeForState("resetting"), { text: "复位中", kind: "busy" });
});

test("a completed test is terminal, not busy", () => {
  // The regression: testOk used to be unclassified, so after a successful test
  // the badge stayed on "测试中" forever.
  const b = badgeForState("testOk");
  assert.ok(b, "testOk must produce a badge");
  assert.notEqual(b.kind, "busy");
  assert.deepEqual(b, { text: "未连接", kind: "off" });
});

test("connected and reset show a live session", () => {
  assert.deepEqual(badgeForState("connected"), { text: "已连接", kind: "on" });
  assert.deepEqual(badgeForState("reset"), { text: "已连接", kind: "on" });
  assert.deepEqual(badgeForState("disconnected"), { text: "未连接", kind: "off" });
});

test("neutral states leave the badge alone", () => {
  for (const s of ["devices", "probes", "deviceOk", "saving", "saved"]) {
    assert.equal(badgeForState(s), null, `${s} should not change the badge`);
  }
});

test("session flags agree with the badge", () => {
  for (const s of ALL_BACKEND_STATES) {
    const badge = badgeForState(s);
    if (stateMeansConnected(s)) {
      assert.equal(badge?.kind, "on", `${s}: connected but badge ${badge?.kind}`);
    }
    if (stateEndsSession(s)) {
      assert.equal(badge?.kind, "off", `${s}: ends session but badge ${badge?.kind}`);
    }
  }
  // A completed test ends the session (its teardown closes the probe).
  assert.equal(stateEndsSession("testOk"), true);
  assert.equal(stateMeansConnected("testOk"), false);
});
