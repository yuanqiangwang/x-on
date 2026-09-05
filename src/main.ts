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

type Settings = {
  accelerator?: string;
  autostart?: boolean;
  font?: string | null;
};

// UI font chain: the user-supplied font name from settings.json is prepended
// (VS Code editor.fontFamily style); empty falls back to the bundled LXGW WenKai
// default in index.html's :root --font. Kept in sync with that value.
const FALLBACK_FONT_CHAIN =
  '"LXGW WenKai GB Screen", "LXGW WenKai", "PingFang SC", "Microsoft YaHei", system-ui, sans-serif';

function applyFont(font?: string | null) {
  const docStyle = document.documentElement.style;
  const name = font?.trim();
  if (name) {
    docStyle.setProperty("--font", `"${name}", ${FALLBACK_FONT_CHAIN}`);
  } else {
    // Empty => let the :root default (bundled LXGW WenKai) apply.
    docStyle.removeProperty("--font");
  }
}

const win = getCurrentWebviewWindow();
const input = document.getElementById("search") as HTMLInputElement;
const list = document.getElementById("results") as HTMLUListElement;

let apps: AppInfo[] = [];
let filtered: AppInfo[] = [];
let selected = 0;

// Cached icon data URI per launch path, or `null` once we've given up on a row.
// `iconTries` caps the retry budget so a transient extraction failure (e.g. the
// shell icon cache warming up on first render) isn't cached as a permanent miss.
// Loaded in batched IPC calls per render (not one invoke per row); `MAX_ICONS_PER_BATCH`
// is a chunk size, so the whole set is covered rather than just the first 50.
const iconCache = new Map<string, string | null>();
const iconFetching = new Set<string>();
const iconTries = new Map<string, number>();
const MAX_ICONS_PER_BATCH = 50;
const MAX_ICON_TRIES = 3;

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

// Batch-load icons for rows that don't have a cached result yet. `MAX_ICONS_PER_BATCH`
// is now a chunk size — we cover the whole set in batches rather than only the first
// 50, so a long list's tail (visible after scrolling) still gets icons. A failed
// extraction is counted against a retry budget instead of being cached as a permanent
// miss, so a one-off failure (e.g. the shell icon cache warming up) recovers.
async function loadMissingIcons(apps: AppInfo[]) {
  const pending = apps.filter(
    (a) =>
      !iconCache.has(a.launchPath) &&
      !iconFetching.has(a.launchPath) &&
      (iconTries.get(a.launchPath) ?? 0) < MAX_ICON_TRIES,
  );
  if (pending.length === 0) return;

  for (let i = 0; i < pending.length; i += MAX_ICONS_PER_BATCH) {
    const chunk = pending.slice(i, i + MAX_ICONS_PER_BATCH);
    const paths = chunk.map((a) => a.launchPath);
    paths.forEach((p) => iconFetching.add(p));
    try {
      const map = await invoke<Record<string, string | null>>("get_app_icons", { paths });
      for (const p of paths) {
        const uri = map[p] ?? null;
        if (uri) {
          iconCache.set(p, uri);
        } else {
          const t = (iconTries.get(p) ?? 0) + 1;
          iconTries.set(p, t);
          if (t >= MAX_ICON_TRIES) iconCache.set(p, null);
        }
      }
    } catch {
      // Whole-batch failure: don't blame the icons, just spend one retry each.
      paths.forEach((p) => iconTries.set(p, (iconTries.get(p) ?? 0) + 1));
    } finally {
      paths.forEach((p) => iconFetching.delete(p));
    }
  }

  // Reflect any newly-cached icons onto the in-memory apps for the render pass.
  for (const a of apps) {
    const uri = iconCache.get(a.launchPath);
    if (uri) a.icon = uri;
  }
  render();
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

// Direction keys + Enter are selection/navigation, so they should mean the same
// whether focus is on the search box, a clicked result row, or the body — the
// list's native scrolling would otherwise hijack them once focus leaves the
// input. Listen on the document in the capture phase so this runs before any
// widget handler. Keys part of an in-progress IME candidate session (isComposing)
// are passed through so arrow keys keep picking pinyin candidates and Esc keeps
// cancelling them, instead of navigating the list.
document.addEventListener(
  "keydown",
  (e) => {
    if (e.isComposing) return;

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
    } else if (e.key === "Escape") {
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

// Reload the index when it changes underneath us (tray portable add/remove, rescan).
win.listen("index-updated", () => loadApps(true));

// Hot-apply a font change the user made in settings.json (backend watches it).
win.listen<Settings>("settings-changed", (e) => {
  applyFont(e.payload.font);
});

input.addEventListener("focus", render);

(async () => {
  try {
    const settings = await invoke<Settings>("get_config");
    applyFont(settings.font);
  } catch (e) {
    console.error(e);
  }
  await loadApps(true);
  input.focus();
})();
