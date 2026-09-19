/**
 * Pure helpers for the device picker.
 *
 * Kept separate from `main.ts` so they can be unit-tested without a browser or
 * the Tauri backend. The picker replaced a native `<datalist>`/plain-combo
 * because the device database has thousands of entries with several columns,
 * which needs a real table with search.
 */

export interface DeviceEntry {
  vendor: string;
  name: string;
  core: string;
  /** RAM size in bytes as reported by the DLL. */
  ramBytes: number;
  /** Flash size in bytes as reported by the DLL. */
  flashBytes: number;
}

/**
 * Rank catalogue entries against a query.
 *
 * Prefix matches come first (the common case is typing the start of a part
 * number), then substring matches. Matching is case-insensitive. An empty query
 * returns the first `limit` entries in catalogue order. `limit` bounds how many
 * rows the table has to render.
 */
export function filterDevices(
  catalogue: DeviceEntry[],
  query: string,
  limit: number,
): DeviceEntry[] {
  const q = query.trim().toLowerCase();
  if (!q) return catalogue.slice(0, limit);

  const prefix: DeviceEntry[] = [];
  const contains: DeviceEntry[] = [];
  for (const d of catalogue) {
    const n = d.name.toLowerCase();
    if (n.startsWith(q)) {
      prefix.push(d);
      if (prefix.length >= limit) break;
    } else if (n.includes(q)) {
      contains.push(d);
    }
  }
  return prefix.concat(contains).slice(0, limit);
}

/**
 * Filter across every visible column, so the footer can say "当前显示 N 个"
 * for a query that matches a vendor or core but not the part number.
 */
export function searchDevices(
  catalogue: DeviceEntry[],
  query: string,
  limit: number,
): DeviceEntry[] {
  const q = query.trim().toLowerCase();
  if (!q) return catalogue.slice(0, limit);

  const prefix: DeviceEntry[] = [];
  const rest: DeviceEntry[] = [];
  for (const d of catalogue) {
    const name = d.name.toLowerCase();
    const hay = `${d.vendor} ${d.name} ${d.core}`.toLowerCase();
    if (!hay.includes(q)) continue;
    if (name.startsWith(q)) prefix.push(d);
    else rest.push(d);
    if (prefix.length >= limit) break;
  }
  return prefix.concat(rest).slice(0, limit);
}

/**
 * Bytes → the picker's RAM/Flash column text.
 *
 * Rendered in KB (rounded) so the column stays aligned and matches the J-Link
 * tooling convention; some DLL entries are not a multiple of 1024 (e.g.
 * 1049392 B) and would otherwise show an irregular byte count. Values below 1 KB
 * are shown as bytes rather than rounding to "0 KB".
 */
export function formatBytes(n: number): string {
  if (!n || n <= 0) return "-";
  if (n < 1024) return `${n} B`;
  return `${Math.round(n / 1024)} KB`;
}

/**
 * Next highlighted row for ArrowUp/ArrowDown, wrapping around.
 *
 * `current` is the previously highlighted index (-1 when none). `delta` is +1 or
 * -1. Returns -1 when there is nothing to highlight. When starting from -1,
 * ArrowDown selects the first row and ArrowUp selects the last.
 */
export function nextActive(current: number, delta: number, count: number): number {
  if (count <= 0) return -1;
  if (current < 0) return delta >= 0 ? 0 : count - 1;
  return (((current + delta) % count) + count) % count;
}
