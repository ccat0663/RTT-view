/**
 * RTT View frontend.
 *
 * Receives batches of log lines over a Tauri IPC Channel and renders them as
 * real DOM text nodes (so mouse selection and Ctrl+C work), capping the number
 * of retained lines to keep long sessions responsive.
 */
import { invoke } from "@tauri-apps/api/core";
import { Channel } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { nextActive, searchDevices, formatBytes, type DeviceEntry } from "./device-filter.ts";
import { humanRate, humanSize, num } from "./format.ts";
import { formatTimestamp } from "./format.ts";
import {
  DEFAULT_LOG_FONT_SIZE,
  LOG_FONT_SIZES,
  logFontLabel,
  normalizeLogFontSize,
} from "./log-font.ts";
import { atBottom, classifyScrollEvent, followStep } from "./smooth-scroll.ts";
import { ctrlABelongsToTextField, hintActionFor } from "./log-view.ts";
import { badgeForState, stateEndsSession, stateMeansConnected } from "./connection-state.ts";

// ---------------------------------------------------------------------------
// Types mirroring the Rust side (serde camelCase).
// ---------------------------------------------------------------------------
interface ProbeInfo {
  serial: number;
  product: string;
  nickname: string;
  firmware: string;
  connection: number;
}

interface RttBufferInfo {
  index: number;
  direction: number;
  name: string;
  size: number;
}

interface LogLine {
  tsMs: number;
  relS: number;
  /// RTT up channel label; only present when capturing all channels.
  ch?: string;
  text: string;
}

interface Settings {
  dllPath: string;
  probeSerial: number;
  device: string;
  interface: string;
  speedKhz: number;
  upChannel: number;
  captureAll: boolean;
  resetOnConnect: boolean;
  autoConnect: boolean;
  showDate: boolean;
  showTime: boolean;
  showRelative: boolean;
  autoscroll: boolean;
  logFontSize: number;
  maxLines: number;
  saveFormat: string;
}

type AcqEvent =
  | {
      type: "state";
      state: string;
      message: string;
      dllVersion?: string;
      probes?: ProbeInfo[];
      buffers?: RttBufferInfo[];
    }
  | { type: "lines"; lines: LogLine[] }
  | {
      type: "devices";
      devices: DeviceEntry[];
    }
  | {
      type: "stats";
      bytesPerSec: number;
      linesPerSec: number;
      hostOverflow: number;
      totalLines: number;
      savedBytes: number;
      saving: boolean;
    }
  | { type: "error"; message: string };

// ---------------------------------------------------------------------------
// Element lookup
// ---------------------------------------------------------------------------
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const els = {
  dllPath: $<HTMLInputElement>("dll-path"),
  browseDll: $<HTMLButtonElement>("browse-dll"),
  probe: $<HTMLSelectElement>("probe"),
  refreshProbes: $<HTMLButtonElement>("refresh-probes"),
  device: $<HTMLInputElement>("device"),
  devicePick: $<HTMLButtonElement>("device-pick"),
  deviceModal: $<HTMLElement>("device-modal"),
  deviceClose: $<HTMLButtonElement>("device-close"),
  deviceSearch: $<HTMLInputElement>("device-search"),
  deviceSearchClear: $<HTMLButtonElement>("device-search-clear"),
  deviceRows: $<HTMLTableSectionElement>("device-rows"),
  deviceTotal: $<HTMLElement>("device-total"),
  deviceShown: $<HTMLElement>("device-shown"),
  deviceOk: $<HTMLButtonElement>("device-ok"),
  deviceCancel: $<HTMLButtonElement>("device-cancel"),
  iface: $<HTMLSelectElement>("interface"),
  speed: $<HTMLInputElement>("speed"),
  upChannel: $<HTMLInputElement>("up-channel"),
  upCaptureAll: $<HTMLInputElement>("up-capture-all"),
  connectReset: $<HTMLButtonElement>("connect-reset"),
  connect: $<HTMLButtonElement>("connect"),
  test: $<HTMLButtonElement>("test"),
  reset: $<HTMLButtonElement>("reset"),
  disconnect: $<HTMLButtonElement>("disconnect"),

  optDate: $<HTMLInputElement>("opt-date"),
  optTime: $<HTMLInputElement>("opt-time"),
  optRelative: $<HTMLInputElement>("opt-relative"),
  optAutoscroll: $<HTMLInputElement>("opt-autoscroll"),
  optDisplay: $<HTMLInputElement>("opt-display"),
  copySelected: $<HTMLButtonElement>("copy-selected"),
  copyAll: $<HTMLButtonElement>("copy-all"),
  clear: $<HTMLButtonElement>("clear"),
  logFontSize: $<HTMLSelectElement>("log-font-size"),
  saveFormat: $<HTMLSelectElement>("save-format"),
  saveAppend: $<HTMLInputElement>("save-append"),
  saveToggle: $<HTMLButtonElement>("save-toggle"),

  log: $<HTMLElement>("log"),
  statusState: $<HTMLElement>("status-state"),
  statusMessage: $<HTMLElement>("status-message"),
  statRate: $<HTMLElement>("stat-rate"),
  statLines: $<HTMLElement>("stat-lines"),
  statOverflow: $<HTMLElement>("stat-overflow"),
  statSaved: $<HTMLElement>("stat-saved"),
};

// ---------------------------------------------------------------------------
// Local UI state
// ---------------------------------------------------------------------------
let settings: Settings;
let connected = false;
let saving = false;
let maxLines = 5000;
let showDate = true;
let showTime = true;
let showRelative = true;
let logFontSize = DEFAULT_LOG_FONT_SIZE;
/// Whether a probe scan has returned at least once (probes are scanned lazily).
let probesScanned = false;
/// Resolver awaiting the next probe-scan result, if any.
let probeScanResolver: (() => void) | null = null;

/// "All" captures every up channel, so the single-channel number is moot.
/// Called at wire time, on toggle, and after settings are restored at boot.
function syncCaptureAll() {
  els.upChannel.disabled = els.upCaptureAll.checked;
}

/// Scan probes once, on demand, and resolve when the result arrives.
///
/// Connect/Test call this so the user does not have to press 「刷新」 first, while
/// startup stays quiet. Enumeration starts the SEGGER GUI helper, so it is only
/// ever triggered by an explicit user action.
async function ensureProbesScanned(): Promise<void> {
  if (probesScanned) return;
  const done = new Promise<void>((resolve) => {
    probeScanResolver = resolve;
  });
  await invoke("refresh_probes", { dllPath: els.dllPath.value.trim() });
  // Guard against a scan that never reports back (e.g. DLL missing).
  const timeout = new Promise<void>((resolve) => setTimeout(resolve, 4000));
  await Promise.race([done, timeout]);
}

// Batched rendering: lines arrive in batches, we paint at most once per frame.
let renderQueue: LogLine[] = [];
let rafScheduled = false;
let lineCount = 0;

// Source of truth for the displayed lines, so display toggles can re-render
// without re-parsing the DOM. Trimmed to `maxLines` alongside the DOM.
let history: LogLine[] = [];

// Whether the log view should keep following the newest line. Set from the
// scroll position: scrolling up to read stops the follow, returning to the
// bottom resumes it. Used instead of re-deciding from scratch every frame,
// which made the view fight the user's scroll.
let pinnedToBottom = true;

// ---------------------------------------------------------------------------
// Log rendering
// ---------------------------------------------------------------------------

/** Build the font-size options once, from the single source of truth. */
function populateLogFontSizes() {
  const frag = document.createDocumentFragment();
  for (const size of LOG_FONT_SIZES) {
    const opt = document.createElement("option");
    opt.value = String(size);
    opt.textContent = logFontLabel(size);
    frag.appendChild(opt);
  }
  els.logFontSize.appendChild(frag);
  // Select the default here, not only in boot(): a native select shows its first
  // option until told otherwise, so without this the control reads 11.5 while the
  // log is still at 12.5 whenever the persisted value has not loaded yet.
  applyLogFontSize(DEFAULT_LOG_FONT_SIZE);
}

/**
 * Apply the chosen log font size.
 *
 * Driven through a CSS variable on the root rather than an inline style on
 * `#log`, so the value sits in the same place as the rest of the theme and a
 * future element (say a second pane) can opt in by using the variable.
 */
function applyLogFontSize(size: unknown) {
  logFontSize = normalizeLogFontSize(size);
  document.documentElement.style.setProperty("--log-font-size", `${logFontSize}px`);
  els.logFontSize.value = String(logFontSize);
}

const EMPTY_HINT_TEXT =
  "等待 RTT 数据…（连接后，目标通过 SEGGER_RTT_printf 输出的内容会显示在这里）";

function createHint(): HTMLElement {
  const hint = document.createElement("div");
  hint.className = "empty-hint";
  hint.textContent = EMPTY_HINT_TEXT;
  return hint;
}

/**
 * Keep the empty-state hint in step with the line count: it belongs on screen
 * only while nothing is rendered.
 *
 * Must run *after* the count is updated, not before. Reading the count ahead of
 * the incoming batch is what left the hint up through the first lines of a
 * session, since that batch was judged against a zero it had not yet changed.
 */
function syncHint() {
  const hint = els.log.querySelector(".empty-hint");
  switch (hintActionFor(hint !== null, lineCount)) {
    case "add":
      els.log.appendChild(createHint());
      break;
    case "remove":
      hint?.remove();
      break;
    case "keep":
      break;
  }
}

/** True when the viewport is at (or within a few px of) the newest line. */
function logAtBottom(): boolean {
  return atBottom(els.log.scrollTop, els.log.scrollHeight, els.log.clientHeight);
}

// ---------------------------------------------------------------------------
// Smooth tail following
//
// Data arrives in bursts (the backend flushes on a 30 ms timer) while the screen
// repaints at 60 fps, so writing `scrollTop = scrollHeight` per burst moves the
// view in uneven jumps at an irregular cadence. Instead we ease toward the tail
// a little each frame, which turns each burst into continuous motion.
//
// The loop writes `scrollTop` itself, so its own scroll events must not be read
// as "the user took over". `userScrollIntent` is set only by real input events,
// and `lastWrittenScrollTop` records where our own last write left the viewport
// (see `setScrollTop`); a scroll event matching it is ours.
// ---------------------------------------------------------------------------
let followRafId: number | null = null;
let userScrollIntent = false;
let lastWrittenScrollTop: number | null = null;
let lastFollowAt = 0;

/// Above this many retained lines the log is considered heavy, and following
/// snaps to the tail instead of easing. Measured on a 5000-line DOM: easing adds
/// ~8% total scroll cost (75 fps and zero long frames in both cases), which is
/// worth paying to remove the stutter on a normal-sized log — but on a very
/// large one the extra per-frame layout work is no longer free, so we fall back
/// to a single write per batch.
const SMOOTH_FOLLOW_MAX_LINES = 3000;

/**
 * Move the viewport ourselves, and remember where we put it.
 *
 * Scroll events do not say who caused them, so recording the offset here is what
 * lets the `scroll` handler tell our own writing apart from the user's. Read back
 * after the write because the browser clamps an offset past the end.
 */
function setScrollTop(value: number) {
  els.log.scrollTop = value;
  lastWrittenScrollTop = els.log.scrollTop;
}

/** Continue easing the view toward the newest line, frame by frame. */
function followTick(now: number) {
  followRafId = null;
  const el = els.log;
  if (!(els.optAutoscroll.checked && pinnedToBottom)) return;

  // Scale motion by real elapsed time so uneven frame times do not produce
  // uneven scrolling speed.
  const dt = lastFollowAt === 0 ? 1000 / 60 : now - lastFollowAt;
  lastFollowAt = now;

  // One geometry read, reused for both the step and the continue test.
  const maxScrollTop = el.scrollHeight - el.clientHeight;
  const remaining = maxScrollTop - el.scrollTop;
  if (remaining <= 0) return;

  const step = followStep(remaining, el.clientHeight, dt);
  setScrollTop(el.scrollTop + step);

  // Keep easing only while a visible gap is left. The threshold is one pixel:
  // sub-pixel remainders are not worth another frame, and stopping here means a
  // quiet stream costs nothing.
  if (maxScrollTop - el.scrollTop > 1) {
    followRafId = requestAnimationFrame(followTick);
  }
}

function requestFollow() {
  if (followRafId === null) {
    lastFollowAt = 0;
    followRafId = requestAnimationFrame(followTick);
  }
}

/** Jump straight to the tail and resume following (used on clear / toggle-on). */
function snapToBottom() {
  pinnedToBottom = true;
  setScrollTop(els.log.scrollHeight);
  requestFollow();
}

/**
 * Start following the tail after a batch was appended.
 *
 * Small logs ease over the next frames (smooth); large logs snap in one write,
 * because the per-frame geometry work stops being free once the DOM is big.
 */
function startFollowing() {
  if (lineCount <= SMOOTH_FOLLOW_MAX_LINES) {
    requestFollow();
  } else {
    setScrollTop(els.log.scrollHeight);
  }
}

/**
 * Append rendered lines, trimming the oldest once the cap is reached.
 *
 * Trimming is skipped while the user is scrolled up: removing rows from the top
 * shifts everything the user is reading, which is a jump they did not ask for.
 * Growth is still bounded by `hardLimit` so a long pause cannot leak memory.
 */
function appendLines(lines: LogLine[], follow: boolean) {
  els.log.appendChild(buildFragment(lines));
  lineCount += lines.length;
  // After the count, so the first batch of a session drops the hint immediately.
  syncHint();

  const softLimit = maxLines;
  const hardLimit = maxLines * 2;
  if (lineCount <= softLimit) return;
  // When following, keep to the soft cap. When the user has scrolled up, allow
  // the buffer to grow to the hard cap and defer trimming to avoid moving their
  // viewport, then trim anyway.
  const limit = follow ? softLimit : hardLimit;
  while (lineCount > limit && els.log.firstChild) {
    els.log.removeChild(els.log.firstChild);
    lineCount--;
  }
}

/** Build a document fragment for a batch of lines. */
function buildFragment(lines: LogLine[]): DocumentFragment {
  const frag = document.createDocumentFragment();
  for (const line of lines) {
    const div = document.createElement("div");
    div.className = "line";
    // Empty when both toggles are off, so nothing is emitted for the timestamp.
    const stamp = formatTimestamp(line.tsMs, showDate, showTime);
    if (stamp) {
      const ts = document.createElement("span");
      ts.className = "ts";
      ts.textContent = `[${stamp}] `;
      div.appendChild(ts);
    }
    if (showRelative) {
      const rel = document.createElement("span");
      rel.className = "rel";
      rel.textContent = `[+${line.relS.toFixed(3)}s] `;
      div.appendChild(rel);
    }
    if (line.ch) {
      const ch = document.createElement("span");
      ch.className = "ch";
      ch.textContent = `[${line.ch}] `;
      div.appendChild(ch);
    }
    const body = document.createElement("span");
    body.className = "body";
    body.textContent = line.text; // textContent, never innerHTML — RTT data is untrusted
    div.appendChild(body);
    frag.appendChild(div);
  }
  return frag;
}

function scheduleRender(lines: LogLine[]) {
  renderQueue.push(...lines);
  if (rafScheduled) return;
  rafScheduled = true;
  requestAnimationFrame(() => {
    rafScheduled = false;
    const batch = renderQueue;
    renderQueue = [];
    if (batch.length === 0) return;

    // Follow only when auto-scroll is on and the view is at the bottom. Deciding
    // once, before the DOM changes, is what keeps the follow steady: it no longer
    // toggles on and off between frames as rows are added and trimmed.
    const follow = els.optAutoscroll.checked && (pinnedToBottom || logAtBottom());

    appendLines(batch, follow);
    // Keep history in lockstep with the DOM so a re-render cannot resurrect
    // rows that were trimmed away.
    history.push(...batch);
    if (history.length > lineCount) history.splice(0, history.length - lineCount);

    if (follow) {
      // Ease to the tail over the next frames (or snap, on a large log); this is
      // what removes the burst-driven stutter.
      startFollowing();
    }
    updateCopyButtons();
  });
}

function clearLog() {
  els.log.innerHTML = "";
  lineCount = 0;
  renderQueue = [];
  history = [];
  pinnedToBottom = true;
  // Zero the status bar immediately (rather than waiting up to a second for the
  // next stats tick) and tell the backend to reset its own counter, so the two
  // cannot disagree.
  els.statLines.textContent = "0 行";
  invoke("clear_log").catch(() => { /* backend not up yet: the view still cleared */ });
  syncHint();
  updateCopyButtons();
}

// ---------------------------------------------------------------------------
// Copy support — relies on native selection of DOM text.
// ---------------------------------------------------------------------------
function selectionText(): string {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed) return "";
  // Only honour selections that actually live inside the log.
  if (sel.rangeCount === 0) return "";
  const range = sel.getRangeAt(0);
  if (!els.log.contains(range.commonAncestorContainer)) return "";
  return sel.toString();
}

async function copyToClipboard(text: string) {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    flashStatus("已复制到剪贴板");
  } catch {
    // Fallback for restricted webview clipboard contexts.
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
    flashStatus("已复制到剪贴板");
  }
}

function logText(): string {
  return Array.from(els.log.querySelectorAll(".line"))
    .map((el) => (el as HTMLElement).innerText)
    .join("\n");
}

function updateCopyButtons() {
  els.copyAll.disabled = lineCount === 0;
}

/**
 * Show a message in the status bar.
 *
 * It stays until something replaces it — no auto-clear timer. Messages here are
 * often the only record of why a connect failed, and a few seconds is not enough
 * to read them. This touches only the text: the connection badge is owned by
 * {@link setConnected}.
 */
function flashStatus(msg: string) {
  els.statusMessage.textContent = msg;
}

// ---------------------------------------------------------------------------
// Connection panel wiring
// ---------------------------------------------------------------------------
/// Arguments for a connect attempt. `resetOnConnect` is chosen by the button the
/// user pressed, not by a stored preference: 「复位并连接」 passes true, 「连接」
/// passes false. Auto-connect uses the persisted preference.
function currentArgs(resetOnConnect: boolean) {
  return {
    dllPath: els.dllPath.value.trim(),
    probeSerial: Number(els.probe.value) || 0,
    device: els.device.value.trim(),
    interface: els.iface.value,
    speedKhz: Number(els.speed.value) || 0,
    upChannel: Number(els.upChannel.value) || 0,
    captureAll: els.upCaptureAll.checked,
    resetOnConnect,
  };
}

function setConnected(on: boolean) {
  connected = on;
  els.connect.disabled = on;
  els.test.disabled = on;
  // Reset needs a live session: it goes through the probe.
  els.reset.disabled = !on;
  els.disconnect.disabled = !on;
  els.probe.disabled = on;
  els.refreshProbes.disabled = on;
  els.device.disabled = on;
  els.iface.disabled = on;
  els.speed.disabled = on;
  els.upChannel.disabled = on || els.upCaptureAll.checked;
  els.upCaptureAll.disabled = on;
  els.connectReset.disabled = on;
  // The badge is set from the state event via `badgeForState`, so there is one
  // place that decides what it says.
}

function populateProbes(probes: ProbeInfo[]) {
  const prev = els.probe.value;
  els.probe.innerHTML = "";
  if (probes.length === 0) {
    const o = document.createElement("option");
    o.value = "0";
    o.textContent = "未检测到调试器";
    els.probe.appendChild(o);
    return;
  }
  if (probes.length > 1) {
    const o = document.createElement("option");
    o.value = "0";
    o.textContent = "请选择调试器…";
    els.probe.appendChild(o);
  }
  for (const p of probes) {
    const o = document.createElement("option");
    // With a single probe the value stays "0" (automatic): JLINKARM_Open picks it
    // without a call to JLINKARM_EMU_SelectByUSBSN, which is unreliable in
    // practice (it returns -1 even for an attached, correctly-enumerated probe).
    // The serial is still shown in the label so the user knows what is used.
    o.value = probes.length === 1 ? "0" : String(p.serial);
    const label = p.nickname
      ? `${p.nickname} (SN ${p.serial})`
      : `${p.product || "J-Link"} (SN ${p.serial})`;
    o.textContent = label;
    // The select has a fixed width, so a long label is ellipsised; expose the
    // full text on hover.
    o.title = label;
    els.probe.appendChild(o);
  }
  if (probes.length === 1) {
    // Automatic: the sole probe is implied.
    els.probe.value = "0";
  } else if (prev) {
    // Preserve an explicit choice across refreshes when the probe is still there.
    const stillThere = probes.some((p) => String(p.serial) === prev);
    els.probe.value = stillThere ? prev : "0";
  }
}

/// Show a non-interactive hint in the probe dropdown before any scan has run.
function setProbeHint(text: string) {
  els.probe.innerHTML = "";
  const o = document.createElement("option");
  o.value = "0";
  o.textContent = text;
  els.probe.appendChild(o);
}

// ---------------------------------------------------------------------------
// Target-device picker (modal)
//
// The J-Link DLL's built-in device database has thousands of entries with
// vendor / core / RAM / Flash, which needs a real table with search rather than
// an inline dropdown. Nothing here touches the probe, so the picker works before
// any hardware is attached.
// ---------------------------------------------------------------------------
let deviceCatalogue: DeviceEntry[] = [];
let deviceMatches: DeviceEntry[] = [];
let deviceActive = -1;
/// The row a click selected within the dialog; committed on 「选择」.
let deviceChecked: DeviceEntry | null = null;

/// How many rows the table renders at once (the catalogue is ~8000 entries).
const DEVICE_MATCH_LIMIT = 300;

function deviceModalOpen(): boolean {
  return !els.deviceModal.hidden;
}

/** Render the table for the current query and update the footer counters. */
function renderDeviceTable() {
  const matches = searchDevices(deviceCatalogue, els.deviceSearch.value, DEVICE_MATCH_LIMIT);
  deviceMatches = matches;

  els.deviceTotal.textContent = String(deviceCatalogue.length);
  els.deviceShown.textContent = String(matches.length);

  const tbody = els.deviceRows;
  tbody.innerHTML = "";

  if (deviceCatalogue.length === 0) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = 5;
    td.className = "empty";
    td.textContent = "尚未读取设备库（需要先定位到 J-Link DLL）";
    tr.appendChild(td);
    tbody.appendChild(tr);
    updateDeviceOk();
    return;
  }
  if (matches.length === 0) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = 5;
    td.className = "empty";
    td.textContent = `没有匹配「${els.deviceSearch.value.trim()}」的设备`;
    tr.appendChild(td);
    tbody.appendChild(tr);
    updateDeviceOk();
    return;
  }

  const frag = document.createDocumentFragment();
  matches.forEach((d, i) => {
    const tr = document.createElement("tr");
    tr.dataset.index = String(i);
    if (deviceChecked && d.name === deviceChecked.name) tr.classList.add("active");
    if (i === deviceActive) tr.classList.add("active");

    const cell = (text: string, cls?: string) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (cls) td.className = cls;
      return td;
    };
    tr.append(
      cell(d.vendor),
      cell(d.name),
      cell(d.core),
      cell(formatBytes(d.ramBytes), "col-ram"),
      cell(formatBytes(d.flashBytes), "col-flash"),
    );
    frag.appendChild(tr);
  });
  tbody.appendChild(frag);
  updateDeviceOk();
}

function updateDeviceOk() {
  els.deviceOk.disabled = deviceChecked === null;
}

/** Highlight row `index` (keyboard navigation) and keep it in view. */
function setDeviceActive(index: number) {
  if (deviceMatches.length === 0) return;
  deviceActive = index;
  const rows = els.deviceRows.querySelectorAll<HTMLTableRowElement>("tr[data-index]");
  rows.forEach((el, i) => el.classList.toggle("active", i === deviceActive));
  rows[deviceActive]?.scrollIntoView({ block: "nearest" });
  if (deviceMatches[deviceActive]) {
    deviceChecked = deviceMatches[deviceActive];
    updateDeviceOk();
  }
}

function openDeviceModal() {
  deviceChecked = null;
  deviceActive = -1;
  els.deviceModal.hidden = false;
  els.deviceSearch.value = "";
  renderDeviceTable();
  els.deviceSearch.focus();
}

function closeDeviceModal() {
  els.deviceModal.hidden = true;
  deviceActive = -1;
}

/** Apply the dialog's selection to the read-only field on the main page. */
function commitDeviceChoice(d: DeviceEntry) {
  els.device.value = d.name;
  closeDeviceModal();
  // Keep the DLL-side validation behaviour.
  invoke("validate_device", { name: d.name });
}

function populateDevices(devices: DeviceEntry[]) {
  deviceCatalogue = devices;
  if (deviceModalOpen()) renderDeviceTable();
  flashStatus(`已加载 ${devices.length} 个设备`);
  // Persist so the picker is populated immediately on next launch.
  try {
    localStorage.setItem("rttstudio.devices", JSON.stringify(devices));
  } catch {
    /* quota errors are non-fatal */
  }
}

function wireDevicePicker() {
  els.devicePick.addEventListener("click", () => openDeviceModal());
  els.deviceClose.addEventListener("click", () => closeDeviceModal());
  els.deviceCancel.addEventListener("click", () => closeDeviceModal());

  els.deviceSearch.addEventListener("input", () => {
    deviceActive = -1;
    renderDeviceTable();
  });
  els.deviceSearchClear.addEventListener("click", () => {
    els.deviceSearch.value = "";
    els.deviceSearch.focus();
    deviceActive = -1;
    renderDeviceTable();
  });

  els.deviceSearch.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setDeviceActive(nextActive(deviceActive, +1, deviceMatches.length));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setDeviceActive(nextActive(deviceActive, -1, deviceMatches.length));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (deviceChecked) commitDeviceChoice(deviceChecked);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeDeviceModal();
    }
  });

  // Select a row on click.
  els.deviceRows.addEventListener("click", (e) => {
    const tr = (e.target as HTMLElement).closest<HTMLTableRowElement>("tr[data-index]");
    if (!tr) return;
    const idx = Number(tr.dataset.index);
    const d = deviceMatches[idx];
    if (!d) return;
    deviceChecked = d;
    deviceActive = idx;
    els.deviceRows
      .querySelectorAll("tr[data-index]")
      .forEach((el) => el.classList.toggle("active", el === tr));
    updateDeviceOk();
  });

  // Double-click commits immediately, matching the reference dialog.
  els.deviceRows.addEventListener("dblclick", (e) => {
    const tr = (e.target as HTMLElement).closest<HTMLTableRowElement>("tr[data-index]");
    if (!tr) return;
    const d = deviceMatches[Number(tr.dataset.index)];
    if (d) commitDeviceChoice(d);
  });

  els.deviceOk.addEventListener("click", () => {
    if (deviceChecked) commitDeviceChoice(deviceChecked);
  });

  // Click the dimmed backdrop to dismiss.
  els.deviceModal.addEventListener("mousedown", (e) => {
    if (e.target === els.deviceModal) closeDeviceModal();
  });
}

// ---------------------------------------------------------------------------
// Event channel from the backend
// ---------------------------------------------------------------------------
function handleEvent(ev: AcqEvent) {
  switch (ev.type) {
    case "state":
      if (ev.probes) {
        populateProbes(ev.probes);
        // Multi-probe: restore the remembered choice if that probe is present.
        if (ev.probes.length > 1 && settings.probeSerial) {
          const opt = Array.from(els.probe.options).find(
            (o) => o.value === String(settings.probeSerial),
          );
          if (opt) els.probe.value = String(settings.probeSerial);
        }
        probesScanned = true;
        if (probeScanResolver) {
          probeScanResolver();
          probeScanResolver = null;
        }
      }
      // Button enable/disable, and the badge, both follow from the state. The
      // classification lives in `connection-state.ts` so every state is accounted
      // for — notably `testOk`, which a completed test emits and which previously
      // left the badge stuck on "测试中".
      if (stateMeansConnected(ev.state)) setConnected(true);
      else if (stateEndsSession(ev.state)) setConnected(false);

      const badge = badgeForState(ev.state);
      if (badge) {
        els.statusState.className = `badge ${badge.kind}`;
        els.statusState.textContent = badge.text;
      }
      // The message persists until the next event replaces it (see flashStatus).
      if (ev.message) flashStatus(ev.message);
      break;
    case "lines":
      scheduleRender(ev.lines);
      break;
    case "devices":
      populateDevices(ev.devices);
      break;
    case "stats": {
      const overflow = num(ev.hostOverflow);
      const savedBytes = num(ev.savedBytes);
      els.statRate.textContent = humanRate(ev.bytesPerSec);
      els.statLines.textContent = `${num(ev.totalLines)} 行`;
      els.statOverflow.textContent = `溢出 ${overflow}`;
      els.statOverflow.style.color = overflow > 0 ? "var(--warn)" : "";
      saving = ev.saving;
      updateSaveButton();
      els.statSaved.textContent = saving
        ? `保存中 · ${humanSize(savedBytes)}`
        : savedBytes > 0
          ? `已保存 ${humanSize(savedBytes)}`
          : "";
      break;
    }
    case "error":
      // Report the message; do not infer the connection state from it — a save
      // failure says nothing about whether a probe is attached.
      flashStatus(ev.message);
      break;
  }
}

// ---------------------------------------------------------------------------
// Save-to-file
// ---------------------------------------------------------------------------
function updateSaveButton() {
  els.saveToggle.textContent = saving ? "停止保存" : "开始保存";
  els.saveToggle.classList.toggle("primary", !saving);
}

async function toggleSave() {
  if (saving) {
    await invoke("stop_save");
    saving = false;
    updateSaveButton();
    return;
  }
  const ext = els.saveFormat.value === "csv" ? "csv" : "log";
  const path = await save({
    title: "保存 RTT 日志",
    defaultPath: `rtt-${Date.now()}.${ext}`,
    filters: [
      { name: ext === "csv" ? "CSV" : "日志文件", extensions: [ext] },
      { name: "全部文件", extensions: ["*"] },
    ],
  });
  if (!path) return;
  await invoke("start_save", {
    args: {
      path,
      format: els.saveFormat.value,
      append: els.saveAppend.checked,
    },
  });
  saving = true;
  updateSaveButton();
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------
async function boot() {
  settings = await invoke<Settings>("get_settings");

  // Pre-fill the connection panel from persisted settings.
  els.dllPath.value = settings.dllPath ?? "";
  els.device.value = settings.device ?? "";
  els.iface.value = settings.interface || "SWD";
  els.speed.value = String(settings.speedKhz ?? 4000);
  els.upChannel.value = String(settings.upChannel ?? 0);
  els.upCaptureAll.checked = settings.captureAll ?? false;
  els.saveFormat.value = settings.saveFormat || "timestamp";
  // Clamps an absent or off-scale persisted value to a real step.
  applyLogFontSize(settings.logFontSize);
  // wireUi ran before these values were restored, so re-apply the enable state.
  syncCaptureAll();

  showDate = settings.showDate;
  showTime = settings.showTime;
  showRelative = settings.showRelative;
  maxLines = settings.maxLines || 5000;
  els.optDate.checked = showDate;
  els.optTime.checked = showTime;
  els.optRelative.checked = showRelative;
  els.optAutoscroll.checked = settings.autoscroll;
  pinnedToBottom = settings.autoscroll;

  clearLog();

  // Bind the backend event channel and start the acquisition thread.
  const channel = new Channel<AcqEvent>();
  channel.onmessage = handleEvent;
  await invoke("start_acquisition", { channel });

  // Paint the picker from last session's cache immediately (it holds thousands
  // of entries, so a re-read is worth avoiding on every launch), then refresh
  // from the DLL in the background.
  restoreCachedDevices();
  setProbeHint("未扫描，点「刷新」");

  // Probe enumeration is deliberately NOT run at startup: it starts the SEGGER
  // GUI helper, which pops an "Emulator selection" dialog when no probe is
  // attached. The backend also suppresses that dialog, but scanning on demand
  // keeps startup quiet and puts probe selection in this UI.
  await invoke("refresh_devices", { dllPath: els.dllPath.value.trim() });

  if (settings.autoConnect && els.device.value.trim()) {
    await invoke("refresh_probes", { dllPath: els.dllPath.value.trim() });
    await invoke("connect", { args: currentArgs(settings.resetOnConnect ?? true) });
  }
}

/** Load the device catalogue cached from the previous session, if any. */
function restoreCachedDevices() {
  try {
    const raw = localStorage.getItem("rttstudio.devices");
    if (!raw) return;
    const parsed = JSON.parse(raw) as DeviceEntry[];
    if (Array.isArray(parsed) && parsed.length > 0) {
      deviceCatalogue = parsed;
      if (deviceModalOpen()) renderDeviceTable();
    }
  } catch {
    /* a corrupt cache just means we wait for the DLL read */
  }
}

function wireUi() {
  els.browseDll.addEventListener("click", async () => {
    const picked = await open({
      title: "选择 JLink_x64.dll",
      multiple: false,
      filters: [{ name: "J-Link DLL", extensions: ["dll"] }],
    });
    if (typeof picked === "string") {
      els.dllPath.value = picked;
      await invoke("refresh_probes", { dllPath: picked });
      await invoke("refresh_devices", { dllPath: picked });
    }
  });

  els.refreshProbes.addEventListener("click", () =>
    invoke("refresh_probes", { dllPath: els.dllPath.value.trim() }),
  );

  const doConnect = async (resetFirst: boolean) => {
    els.statusState.className = "badge busy";
    els.statusState.textContent = "连接中";
    await ensureProbesScanned();
    await invoke("connect", { args: currentArgs(resetFirst) });
  };

  els.connect.addEventListener("click", () => doConnect(false));
  // Reset-and-run then connect: the recovery path when the previous tool left the
  // target halted at the reset vector (e.g. Keil without "Reset and Run").
  els.connectReset.addEventListener("click", () => doConnect(true));
  els.test.addEventListener("click", async () => {
    await ensureProbesScanned();
    await invoke("test_connection", { args: currentArgs(false) });
  });
  els.disconnect.addEventListener("click", () => invoke("disconnect"));
  els.reset.addEventListener("click", async () => {
    // Stay usable but reflect that the target is restarting; the backend reports
    // when RTT is re-armed.
    els.reset.disabled = true;
    els.statusState.className = "badge busy";
    els.statusState.textContent = "复位中";
    try {
      await invoke("reset_target");
    } catch (e) {
      els.statusMessage.textContent = `复位失败：${e}`;
      els.reset.disabled = false;
    }
    // Re-enable once the reset command has been accepted; the async event tells
    // us whether RTT came back.
    window.setTimeout(() => { els.reset.disabled = !connected; }, 600);
  });

  wireDevicePicker();

  els.upCaptureAll.addEventListener("change", syncCaptureAll);
  syncCaptureAll();

  populateLogFontSizes();
  els.logFontSize.addEventListener("change", () => {
    applyLogFontSize(els.logFontSize.value);
    invoke("set_prefs", { logFontSize });
    // A size change re-flows every row, so re-pin the tail rather than leaving
    // the viewport wherever the old metrics put it.
    if (pinnedToBottom) snapToBottom();
  });


  // Date and time are separate toggles over one timestamp; each re-renders the
  // retained lines with the new prefix and is persisted under its own key.
  const timestampToggles: Array<[HTMLInputElement, "showDate" | "showTime"]> = [
    [els.optDate, "showDate"],
    [els.optTime, "showTime"],
  ];
  for (const [input, key] of timestampToggles) {
    input.addEventListener("change", () => {
      if (key === "showDate") showDate = input.checked;
      else showTime = input.checked;
      invoke("set_prefs", { [key]: input.checked });
      rerenderAll();
    });
  }
  els.optRelative.addEventListener("change", () => {
    showRelative = els.optRelative.checked;
    invoke("set_prefs", { showRelative });
    rerenderAll();
  });
  els.optAutoscroll.addEventListener("change", () => {
    settings.autoscroll = els.optAutoscroll.checked;
    // Re-enabling means "follow the tail again", even if the view was scrolled up
    // when it was switched off.
    if (settings.autoscroll) snapToBottom();
    invoke("set_prefs", { autoscroll: settings.autoscroll });
  });
  els.optDisplay.addEventListener("change", () =>
    invoke("set_display", { enabled: els.optDisplay.checked }),
  );

  els.copySelected.addEventListener("click", () => copyToClipboard(selectionText()));
  els.copyAll.addEventListener("click", () => copyToClipboard(logText()));
  els.clear.addEventListener("click", clearLog);
  els.saveToggle.addEventListener("click", () => toggleSave());

  // Any of these means the user is driving the scroll position themselves.
  // Programmatic scrolls from the follow loop do not set this, so easing toward
  // the tail is never mistaken for user input.
  for (const ev of ["wheel", "touchmove", "mousedown", "keydown"]) {
    els.log.addEventListener(ev, () => { userScrollIntent = true; }, { passive: true });
  }

  // Re-evaluate following only for scrolls the user actually drove. Our own
  // writes are recognised by their offset, and an input event that produced no
  // movement (a wheel while the log still fits, a keypress after clicking in) is
  // dropped here rather than left latched — a stale latch would otherwise be
  // attached to our own first write once output overflowed, and stop the follow.
  els.log.addEventListener("scroll", () => {
    const action = classifyScrollEvent(
      els.log.scrollTop,
      lastWrittenScrollTop,
      userScrollIntent,
      logAtBottom(),
    );
    if (action === "ignore") {
      // Consume it only for real input, so a wheel with nowhere to scroll does
      // not stay pending and switch auto-scroll off on the next append.
      if (userScrollIntent) userScrollIntent = false;
      return;
    }
    userScrollIntent = false;
    pinnedToBottom = action === "keep";
    if (!pinnedToBottom) {
      els.optAutoscroll.checked = false;
      settings.autoscroll = false;
    }
  });

  // Selecting text is a strong signal the user wants to read or copy, so stop
  // following as well (otherwise the tail would tug the selection away).
  els.log.addEventListener("mouseup", () => {
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed) {
      els.optAutoscroll.checked = false;
      settings.autoscroll = false;
      pinnedToBottom = false;
    }
  });

  window.addEventListener("keydown", (e) => {
    const mod = e.ctrlKey || e.metaKey;
    if (!mod) return;
    const key = e.key.toLowerCase();

    if (key === "c") {
      const sel = selectionText();
      if (sel) {
        e.preventDefault();
        copyToClipboard(sel);
      }
      return;
    }

    // Scope Ctrl+A to the log. Left to the browser it selects the whole document
    // (toolbar + connection panel + status bar included), which is not what
    // pressing it over the log should do. Real text fields keep the native
    // select-all so editing still works — see `ctrlABelongsToTextField`.
    if (key === "a") {
      const active = document.activeElement as HTMLInputElement | null;
      const inTextField = ctrlABelongsToTextField(
        active?.tagName ?? "",
        active?.getAttribute?.("type") ?? "",
        active?.isContentEditable ?? false,
      );
      if (inTextField) return; // let the browser select the field's text
      e.preventDefault();
      selectLogContents();
    }
  });
}

/** Select every line in the log view, and keep following paused. */
function selectLogContents() {
  if (lineCount === 0) return;
  const range = document.createRange();
  range.selectNodeContents(els.log);
  const sel = window.getSelection();
  if (!sel) return;
  sel.removeAllRanges();
  sel.addRange(range);
  // Selecting is a signal the user wants to read or copy, so stop the tail from
  // tugging the selection away — same rule as an explicit mouse selection.
  els.optAutoscroll.checked = false;
  settings.autoscroll = false;
  pinnedToBottom = false;
}

/** Re-render all retained lines from history (used when a display toggle changes). */
function rerenderAll() {
  els.log.innerHTML = "";
  els.log.appendChild(buildFragment(history));
  lineCount = history.length;
  syncHint();
  // Rebuilding the DOM loses the scroll offset; restore the previous intent.
  if (pinnedToBottom) snapToBottom();
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------
window.addEventListener("DOMContentLoaded", async () => {
  try {
    wireUi();
    await boot();
  } catch (e) {
    els.statusMessage.textContent = `启动失败：${e}`;
    els.statusState.className = "badge err";
    els.statusState.textContent = "错误";
  }
});
