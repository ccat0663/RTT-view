/**
 * Mapping from backend connection `state` events to the status-bar badge.
 *
 * Kept pure and separate from `main.ts` so the mapping is testable — a missing
 * case is invisible in the UI (the badge simply keeps its previous text), and
 * that is exactly how a completed test used to stay stuck on "测试中": `testOk`
 * was absent from the handler, so nothing ever cleared the busy badge.
 */

/** CSS class suffix used by the badge (`badge on` / `badge off` / `badge busy`). */
export type BadgeKind = "on" | "off" | "busy";

export interface Badge {
  text: string;
  kind: BadgeKind;
}

/**
 * The badge for a state event, or `null` when the event should not change it
 * (progress/device/save notifications such as `devices`, `probes`, `saving`).
 */
export function badgeForState(state: string): Badge | null {
  switch (state) {
    // A live probe session exists.
    case "connected":
    case "reset":
      return { text: "已连接", kind: "on" };

    // No session: either never connected, disconnected, or a completed test —
    // a test always tears its session down, so afterwards we are not connected.
    case "disconnected":
    case "testOk":
      return { text: "未连接", kind: "off" };

    // In progress.
    case "connecting":
      return { text: "连接中", kind: "busy" };
    case "testing":
      return { text: "测试中", kind: "busy" };
    case "resetting":
      return { text: "复位中", kind: "busy" };

    // Neutral notifications (device list, probe list, device validation, file
    // saving): they say nothing about the connection badge.
    default:
      return null;
  }
}

/** Whether a state event means "a live session exists". */
export function stateMeansConnected(state: string): boolean {
  return state === "connected" || state === "reset";
}

/**
 * Whether a state event is a terminal outcome that ends a session attempt.
 *
 * Used to decide when to restore the connect/disconnect buttons; a completed
 * test is such an outcome even though it reports success.
 */
export function stateEndsSession(state: string): boolean {
  return state === "disconnected" || state === "testOk";
}
