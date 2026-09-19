import { test } from "node:test";
import assert from "node:assert/strict";
import {
  filterDevices,
  searchDevices,
  formatBytes,
  nextActive,
  type DeviceEntry,
} from "./device-filter.ts";

const dev = (
  name: string,
  vendor = "ST",
  core = "Cortex-M4",
  ramBytes = 0,
  flashBytes = 0,
): DeviceEntry => ({ name, vendor, core, ramBytes, flashBytes });

const catalogue: DeviceEntry[] = [
  dev("STM32F407VG", "ST", "Cortex-M4", 128 * 1024, 1024 * 1024),
  dev("STM32F407ZG", "ST", "Cortex-M4", 192 * 1024, 1024 * 1024),
  dev("STM32F103C8", "ST", "Cortex-M3", 20 * 1024, 64 * 1024),
  dev("STM32G431RB", "ST", "Cortex-M4", 32 * 1024, 128 * 1024),
  dev("GD32F407VG", "GigaDevice", "Cortex-M4", 128 * 1024, 1024 * 1024),
  dev("nRF52840_xxAA", "Nordic Semi", "Cortex-M4", 256 * 1024, 1028 * 1024),
];

// --- filterDevices: name-only matching (used by the picker's ranking) -------

test("empty query returns the catalogue head, bounded by limit", () => {
  assert.equal(filterDevices(catalogue, "", 3).length, 3);
  assert.equal(filterDevices(catalogue, "   ", 3).length, 3);
});

test("name matching is case-insensitive", () => {
  assert.deepEqual(
    filterDevices(catalogue, "stm32f407", 10).map((d) => d.name),
    ["STM32F407VG", "STM32F407ZG"],
  );
});

test("name prefix matches rank before name substring matches", () => {
  const cat = [dev("XGD32F407"), dev("GD32F407")];
  assert.deepEqual(
    filterDevices(cat, "gd32f407", 10).map((d) => d.name),
    ["GD32F407", "XGD32F407"],
  );
});

test("limit caps the result and no match yields empty", () => {
  assert.equal(filterDevices(catalogue, "32", 2).length, 2);
  assert.deepEqual(filterDevices(catalogue, "zzzz", 10), []);
});

// --- searchDevices: searches vendor + name + core --------------------------

test("searchDevices matches the vendor column", () => {
  assert.deepEqual(
    searchDevices(catalogue, "Nordic", 10).map((d) => d.name),
    ["nRF52840_xxAA"],
  );
  assert.deepEqual(
    searchDevices(catalogue, "GigaDevice", 10).map((d) => d.name),
    ["GD32F407VG"],
  );
});

test("searchDevices matches the core column", () => {
  assert.deepEqual(
    searchDevices(catalogue, "Cortex-M3", 10).map((d) => d.name),
    ["STM32F103C8"],
  );
});

test("searchDevices prefers name-prefix hits over other-column hits", () => {
  // "ST" appears in the vendor of every ST part and as a name prefix of some.
  const first = searchDevices(catalogue, "st", 10);
  // Name-prefix matches (STM32*) must come before any other matches.
  assert.ok(first[0].name.startsWith("STM32"));
});

test("searchDevices honours the limit", () => {
  assert.equal(searchDevices(catalogue, "Cortex", 2).length, 2);
});

// --- formatBytes: the RAM / Flash column text ------------------------------

test("formatBytes rounds to KB, including non-multiples of 1024", () => {
  assert.equal(formatBytes(128 * 1024), "128 KB");
  assert.equal(formatBytes(1024 * 1024), "1024 KB");
  assert.equal(formatBytes(20 * 1024), "20 KB");
  assert.equal(formatBytes(0), "-");
  // Real value from the DLL for nRF9160_xxAA (1024.8 KB) — must not show bytes.
  assert.equal(formatBytes(1049392), "1025 KB");
  // Sub-KB values stay in bytes instead of rounding away to "0 KB".
  assert.equal(formatBytes(500), "500 B");
});

// --- nextActive: dialog keyboard navigation --------------------------------

test("nextActive starts at the first row on down, last row on up", () => {
  assert.equal(nextActive(-1, +1, 5), 0);
  assert.equal(nextActive(-1, -1, 5), 4);
});

test("nextActive wraps in both directions", () => {
  assert.equal(nextActive(4, +1, 5), 0);
  assert.equal(nextActive(0, -1, 5), 4);
  assert.equal(nextActive(2, +1, 5), 3);
  assert.equal(nextActive(2, -1, 5), 1);
});

test("nextActive is -1 when there is nothing to select", () => {
  assert.equal(nextActive(-1, +1, 0), -1);
  assert.equal(nextActive(3, -1, 0), -1);
});

test("single-row list stays on row 0", () => {
  assert.equal(nextActive(-1, +1, 1), 0);
  assert.equal(nextActive(0, +1, 1), 0);
  assert.equal(nextActive(0, -1, 1), 0);
});
