import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { pinyin } from "pinyin-pro";

type AppInfo = {
  name: string;
  comment?: string | null;
  launchPath: string;
  targetPath: string;
  icon?: string | null;
};

const win = getCurrentWebviewWindow();
const input = document.getElementById("search") as HTMLInputElement;
const list = document.getElementById("results") as HTMLUListElement;

let apps: AppInfo[] = [];
let filtered: AppInfo[] = [];
let selected = 0;

// Cached icon data URI per launch path, or `null` for a proven miss so we never
// re-request a row whose icon failed to extract. Loaded in one batched IPC call
// per render (not one invoke per row), and capped so a large result set doesn't
// trigger a huge extraction burst on the first keystroke.
const iconCache = new Map<string, string | null>();
const iconFetching = new Set<string>();
const MAX_ICONS_PER_BATCH = 50;

async function loadApps(force = false) {
  const cmd = force ? "rescan" : "scan_apps";
  const data = await invoke<AppInfo[]>(cmd);
  apps = data ?? [];
  buildPinyinIndex();
  render();
}

// ---------------------------------------------------------------------------
// Pinyin + abbreviation (fuzzy) matching.
// Precomputed once per launch path, so typing never re-runs the converter.
// ---------------------------------------------------------------------------
type PinyinFields = { full: string; init: string; words: string };
const pinyinMap = new Map<string, PinyinFields>();

// CJK char ranges: Unified, Extension A, Compatibility Ideographs.
const CJK_CLASS = `一-鿿㐀-䶿豈-﫿`;
const CJK_RE = new RegExp(`[${CJK_CLASS}]`);
function isCJK(ch: string): boolean {
  return CJK_RE.test(ch);
}

// Split a name into maximal CJK / non-CJK runs so each CJK run is fed whole to
// pinyin-pro (word-level polyphone disambiguation), while non-CJK chars stay
// under our control.
function computePinyin(name: string): PinyinFields {
  const runs =
    name.match(new RegExp(`[${CJK_CLASS}]+|[^${CJK_CLASS}]+`, "g")) ?? [name];

  let full = "";
  let init = "";
  for (const seg of runs) {
    if (isCJK(seg[0])) {
      const arr = pinyin(seg, { type: "all", toneType: "none" }) as {
        pinyin: string;
      }[];
      for (const it of arr) {
        const syl = (it.pinyin || seg).toLowerCase();
        full += syl;
        init += syl[0] ?? "";
      }
    } else {
      for (const ch of seg) {
        if (/\s/.test(ch)) continue; // drop spaces so multi-word prefixes match cleanly
        const lc = ch.toLowerCase();
        full += lc;
        init += lc;
      }
    }
  }

  return { full, init, words: wordInitials(name) };
}

// First letter of each whitespace-separated token (Google Chrome -> gc). Only for
// multi-token names, so it adds no noise to single CJK words.
function wordInitials(name: string): string {
  const tokens = name.split(/[\s　-]+/).filter(Boolean);
  if (tokens.length < 2) return "";
  let out = "";
  for (const w of tokens) {
    const first = w[0] ?? "";
    out += isCJK(first)
      ? (pinyin(first, { toneType: "none" }) || first).charAt(0)
      : first.toLowerCase();
  }
  return out;
}

function buildPinyinIndex() {
  pinyinMap.clear();
  for (const a of apps) pinyinMap.set(a.launchPath, computePinyin(a.name));
}

// Is `q` a subsequence of `s` (chars in order, gaps allowed)?
function isSubsequence(q: string, s: string): boolean {
  let i = 0;
  for (const c of s) {
    if (c === q[i]) i++;
    if (i === q.length) return true;
  }
  return i === q.length;
}

// Rank an app against a query. Tiers, strongest first:
// name exact > name prefix > full-pinyin prefix > word initials > pinyin initials
// > name/full substring > subsequence. Within a tier, a *tighter* hit wins: the
// query accounts for more of the actual field it matched, so a short field that
// the query fully covers (微信 init "wx") outranks a long field it only prefixes
// (微信开发者工具 init "wxkfzgj"). -1 never returned; use null for no match.
function scoreMatch(a: AppInfo, t: string): { tier: number; len: number } | null {
  const name = a.name.toLowerCase();
  const f = pinyinMap.get(a.launchPath);
  const full = f?.full ?? name;
  const init = f?.init ?? "";
  const words = f?.words ?? "";

  if (name === t) return { tier: 1, len: name.length };
  if (name.startsWith(t)) return { tier: 2, len: name.length };
  if (full.startsWith(t)) return { tier: 3, len: full.length };
  if (words && (words === t || words.startsWith(t))) return { tier: 4, len: words.length };
  if (init && (init === t || init.startsWith(t))) return { tier: 5, len: init.length };
  if (name.includes(t) || full.includes(t)) {
    const len = Math.min(
      name.includes(t) ? name.length : Infinity,
      full.includes(t) ? full.length : Infinity,
    );
    return { tier: 6, len };
  }
  if (isSubsequence(t, name) || isSubsequence(t, full)) {
    const len = Math.min(
      isSubsequence(t, name) ? name.length : Infinity,
      isSubsequence(t, full) ? full.length : Infinity,
    );
    return { tier: 7, len };
  }
  return null;
}

function filterApps(q: string): AppInfo[] {
  const t = q.trim().toLowerCase();
  if (!t) return apps;
  const scored: { a: AppInfo; tier: number; len: number }[] = [];
  for (const a of apps) {
    const s = scoreMatch(a, t);
    if (s) scored.push({ a, tier: s.tier, len: s.len });
  }
  // Tighter match first, then shorter matched field; stable so equal pairs keep
  // index order.
  scored.sort((x, y) => x.tier - y.tier || x.len - y.len);
  return scored.map((s) => s.a);
}

function render() {
  filtered = filterApps(input.value);
  if (selected >= filtered.length) selected = Math.max(0, filtered.length - 1);

  list.textContent = "";
  filtered.forEach((a, i) => {
    const li = document.createElement("li");
    li.className = i === selected ? "active" : "";

    li.appendChild(avatarEl(a));

    const meta = document.createElement("div");
    meta.className = "meta";
    const nameEl = document.createElement("span");
    nameEl.className = "name";
    nameEl.textContent = a.name;
    meta.appendChild(nameEl);
    if (a.comment) {
      const c = document.createElement("span");
      c.className = "comment";
      c.textContent = a.comment;
      meta.appendChild(c);
    }
    li.appendChild(meta);

    li.addEventListener("click", () => {
      selected = i;
      render();
    });

    list.appendChild(li);
  });

  const active = list.querySelector("li.active") as HTMLElement | null;
  active?.scrollIntoView({ block: "nearest" });

  loadMissingIcons(filtered);
}

function avatarEl(a: AppInfo): HTMLElement {
  if (a.icon) {
    const img = document.createElement("img");
    img.src = a.icon;
    return img;
  }
  const av = document.createElement("div");
  av.className = "avatar";
  av.textContent = (a.name.trim()[0] ?? "?").toUpperCase();
  return av;
}

// Batch-load icons for the rows that don't have one cached yet. One IPC call with
// the whole set of missing paths, rather than N calls. When it resolves we cache
// every result (misses included) and re-render to swap letter avatars for icons.
async function loadMissingIcons(apps: AppInfo[]) {
  const missing = apps
    .filter((a) => !iconCache.has(a.launchPath) && !iconFetching.has(a.launchPath))
    .slice(0, MAX_ICONS_PER_BATCH)
    .map((a) => a.launchPath);
  if (missing.length === 0) return;

  missing.forEach((p) => iconFetching.add(p));
  try {
    const map = await invoke<Record<string, string | null>>("get_app_icons", { paths: missing });
    for (const [p, uri] of Object.entries(map)) {
      iconCache.set(p, uri ?? null);
      const app = apps.find((a) => a.launchPath === p);
      if (app) app.icon = uri ?? null;
    }
  } catch {
    // Extraction failed wholesale — don't retry these, stay on letter avatars.
    missing.forEach((p) => iconCache.set(p, null));
  } finally {
    missing.forEach((p) => iconFetching.delete(p));
    render();
  }
}

function launch(app: AppInfo) {
  invoke("launch_app", { appPath: app.launchPath }).catch((e) => console.error(e));
}

let debounceTimer: number | undefined;
input.addEventListener("input", () => {
  selected = 0;
  // Debounce so fast typing triggers a single filter/render + icon batch, not one
  // per keystroke — that's what made "to" feel laggy while icons loaded.
  if (debounceTimer !== undefined) clearTimeout(debounceTimer);
  debounceTimer = window.setTimeout(() => render(), 40);
});

input.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    if (filtered.length) selected = (selected + 1) % filtered.length;
    render();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (filtered.length) selected = (selected - 1 + filtered.length) % filtered.length;
    render();
  } else if (e.key === "Enter") {
    const pick = filtered[selected];
    if (pick) launch(pick);
  }
});

// Escape hides the launcher no matter which element holds focus inside the
// document (the input, a clicked result row, or the body). Listen on the document
// in the capture phase so it fires before any widget handler and works even after
// focus drifts off the search box.
document.addEventListener(
  "keydown",
  (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      win.hide();
    }
  },
  true,
);

// Whenever the backend wakes us (global shortcut / single-instance), clear and refocus.
win.listen("wake", () => {
  input.value = "";
  selected = 0;
  render();
  input.focus();
});

input.addEventListener("focus", render);

(async () => {
  await loadApps(true);
  input.focus();
})();
