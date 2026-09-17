# Benchmarks

Black-box measurement of the launcher — every number is taken from outside the
app, with synthetic keystrokes and Win32 window polling. Nothing is
instrumented inside xon, so the procedure is reproducible and comparable
across runs.

```
  bench.ps1 ──▶ 1. hotkey      Alt+Space  ──▶ window visible     ──▶ results/latest.md
                2. typing      one char   ──▶ window grew        ──▶ results/table.md
                3. cold start  launch     ──▶ first usable window ──▶ README table
                4. idle        CPU + memory while hidden
                5. size        release binaries on disk
```

## What is measured, and how

| Metric | How | What it does *not* capture |
| --- | --- | --- |
| **Hotkey → window visible** | Synthetic `Alt+Space` via `keybd_event`, then busy-poll `IsWindowVisible` until the window reports visible. Median/p95 over N runs. | The gap between "window reports visible" and "first frame actually on screen". Win32 visibility flips when `ShowWindow` runs, which is *before* DWM composites the frame. Real perceived latency is a few ms higher. |
| **Keystroke → results** | Summon, let focus settle, send one character, poll `GetWindowRect` until the window grows by ≥ 20 px. Growth is a proxy for "result rows committed and the self-sizing pass ran". | Final pixel flush. It fires when the geometry changes, not when the last row's text is rasterized. It also includes the deliberate 40 ms input debounce in `main.ts`, because that is part of what the user feels. |
| **Cold start → usable** | Kill xon, launch it, then press the accelerator repeatedly until a window appears. | "Process exists" — deliberately not counted. This measures "double-click → you can type". |
| **Memory, idle (private)** | Sum of `PrivateMemorySize64` over the **recursive** process tree — the Rust process plus every WebView2 process it spawned — after a settle period with the window hidden. **This is the headline figure**: committed bytes the group owns, with shared pages counted once. | Pages shared with other applications (WebView2 ships as a shared runtime), and any memory the OS has already trimmed. |
| **Memory, idle (working set)** | Sum of `WorkingSet64` over the same tree. | It **double-counts** physical pages shared between processes, so it reads much higher — 523.6 MB vs 276.7 MB private on the baseline machine. Useful only as an upper bound. |
| **CPU while idle** | `Process.CPU` delta over the sampling window, divided by wall time. Percent of one core. | Wakeups/s. A process can idle at 0.1% CPU and still wake hundreds of times a second, which is what actually costs battery. Not measured here (no `powermetrics` equivalent on Windows without ETW). |
| **Installer size** | The newest artifact under `target/release/bundle/` (NSIS `.exe` or `.msi`); falls back to the bare `xon.exe` if no bundle was built. | The WebView2 runtime, which Windows already provides — so this is *not* comparable to a bundled-Chromium app's on-disk size. |

### Three definitions that produced wrong numbers

All of these were shipped in the script. Recording them so the same mistake is
not made twice.

- **"Size on disk" summed every `.exe`/`.dll` under `target/release` → 206 MB.**
  That is the Cargo build directory: `build/` (126 MB of `build-script-build.exe`)
  plus `deps/` (68 MB of intermediate `.rlib`/`.dll`). None of it ships. The real
  installer is **1.58 MB**.
- **"Memory" only walked one level of the process tree → 229 MB.** WebView2
  spawns its utility/gpu/renderer processes as *grandchildren*, so a single level
  captured 2 of 7 processes and missed 289 MB. Recursive walk: **276.7 MB private /
  523.6 MB working set over 7 processes.**
- **"Memory" then collected *every* descendant → 3,606 MB.** Found 2026-09-17.
  xon is a launcher, so every app started from it becomes a **direct child** and
  outlives it. On the day of the measurement Weixin, QQ, Obsidian and
  aTrustTray were all children of the xon process (creation timestamps *after*
  xon's, so genuinely spawned by it, not recycled PIDs). The recursive walk was
  right for WebView2 and wrong for everything else: it made the headline figure
  a function of "which apps did you launch from xon today". The 2026-09-10
  baseline of 276.7 MB was correct **by accident** — nothing had been launched
  from xon in that session. The walk is now restricted to `xon` +
  `msedgewebview2` + `crashpad_handler` (`$groupNames` in `bench.ps1`), and does
  not descend through anything else. Re-measured the same day: **277.4 MB private
  / 437.7 MB working set over 7 processes** — flat against the 0.6.0 baseline
  (276.7 MB), which is what the metric was always supposed to say.

## Running it

```powershell
pwsh -File benchmarks/bench.ps1                     # defaults: 25 hotkey + 15 typing runs
pwsh -File benchmarks/bench.ps1 -HotkeyRuns 50
pwsh -File benchmarks/bench.ps1 -ColdStart          # also measures cold start (kills running xon)
pwsh -File benchmarks/bench.ps1 -SkipIdle           # skip the 30 s+ idle window
pwsh -File benchmarks/bench.ps1 -UpdateReadme       # splice the table into README.md
```

Or via pnpm:

```bash
pnpm bench
```

### Prerequisites

1. **A release build.** `pnpm tauri build` first. Debug builds are
   unrepresentative — WebView2 runs with different JIT and dev-server
   behaviour, and the numbers will not match what ships.
2. **xon already running**, or pass `-ExePath` and the script starts it.
   The window is found by title (`xon`).
3. **Same integrity level.** If xon runs elevated and the terminal does not,
   UIPI silently drops the injected keystrokes and every run times out. Run
   xon non-elevated (its normal mode).
4. **A quiet machine.** Plugged in, no builds running, and do not touch
   keyboard or mouse while a run is in flight — a stray keystroke lands in
   the search box and changes what is being measured.
5. **PowerShell 7+** (`pwsh`). Windows PowerShell 5.1 will work for most of it
   but is not tested.

### Troubleshooting

- **"run N: timeout (hotkey did not summon)"** — the accelerator in
  `settings.json` is not `Alt+Space`, or another app owns it, or (most likely)
  the integrity-level mismatch above.
- **"run N: no growth"** — the probe character matched nothing, so the window
  never grew. Try another: `pwsh -File benchmarks/bench.ps1 -TypingChar p`.
- **"run N: no growth" on *every* run, with the height never changing at all** —
  the app under test never booted its frontend, so the keystroke is a no-op and
  nothing else on this page is trustworthy either. Two checks, both cheap:
  the window must resize to fit its content (a bare wake is the input box alone,
  ~94 logical px; a window sitting at exactly the `tauri.conf.json` height means
  no JS ever ran), and `msedgewebview2` processes must exist as children of the
  xon process. Observed 2026-09-17: a launcher started by an automation/tooling
  host came up with **zero** WebView2 processes — the process ran, the global
  shortcut registered, the panel appeared, but the webview never initialised,
  so ESC did nothing and typing reported 0/N. Start xon the way a user does.
- **Every hotkey number suspiciously identical** — you are probably measuring
  a debug build, or the machine is loaded enough that the poll loop dominates.

## Interpreting the numbers

- **Hotkey and typing are different budgets.** Expect typing to be several
  times slower: it includes the 40 ms debounce, a full list rebuild, and an
  icon IPC round-trip. Optimizing "hotkey → visible" does nothing for the
  number users actually feel while searching.
- **Resolution is ~1–3 ms.** The poll loop costs that much; a difference
  smaller than that between two runs is noise, not a regression.
- **Compare like with like.** Re-run the baseline on the same machine in the
  same session before and after a change. Cross-machine numbers are not
  comparable.

## Three traps this script works around

All three were found the hard way against a real xon 0.6.0 on Windows 11. If
you extend the script, do not "simplify" these away.

1. **`Process.MainWindowHandle` is not the launcher panel.** .NET hands back a
   13×13 hidden helper surface titled `com.xon.launcher-siw`. Hiding/showing
   that window does nothing useful, and every measurement looks instant.
2. **`FindWindowW(null, "xon")` returns 0** even though the panel's title *is*
   `xon`. So the script enumerates top-level windows with `EnumWindows` and
   matches on owning PID, then prefers the window whose title matches.
3. **Time from the keystroke, not from the start of polling.** `keybd_event`
   only queues the input; the window flips some milliseconds later. Starting
   the stopwatch after `keybd_event` returns measures the poll loop, not the
   launcher.

Two smaller ones: the injected character lands wherever focus is, so the script
waits on `GetForegroundWindow` instead of sleeping a fixed amount (a dropped
keystroke is indistinguishable from "no results" from the outside); and the
tree-walk cursor is named `$id`, not `$pid` — `$pid` is a read-only automatic
variable.

## Interpreting the current baseline

As of 2026-09-17 (Windows 11, Core Ultra 7 255H, xon 0.7.1 release), measured on
the same machine in the same session immediately before and after the
Tailwind v4 / shadcn-visual-layer migration:

| Metric | before | after | Δ |
| --- | --- | --- | --- |
| Hotkey → visible (p50 / p95) | 4.3 / 11.1 ms | 4.3 / 8.2 ms | 0 / −2.9 |
| Keystroke → results (p50 / p95) | 64.7 / 66.2 ms | 69.0 / 81.1 ms | +4.3 / +14.9 |
| Memory, idle (private) | 277.4 MB | 251.3 MB | −26.1 |
| Memory, idle (working set) | 437.7 MB | 518.4 MB | +80.7 |
| CPU while idle | 0.62 % | 0.21 % | −0.41 |
| Installer size | 1.47 MB | 1.47 MB | 0 |
| Bundle on disk (JS+CSS+HTML, gzip) | 150,145 B | 151,523 B | +1,378 (+0.92 %) |

- **Hotkey is not the bottleneck, and it did not move.** p50 4.3 ms is far below
  the ~100 ms threshold a user perceives as instantaneous. Little left to win.
- **Keystroke→results is ~16× slower** (p50 69.0 ms) and is where the perceived
  latency lives. Roughly 40 ms of that is the fixed input debounce; the rest is
  filter + full list rebuild + icon IPC. **Optimizing the summon path cannot
  improve this number.**
- **The `after` typing distribution is bimodal — read its p95 with that in mind.**
  Two independent 15-run samples each split roughly 50/50 between a ~64.5 ms
  cluster and a ~80 ms one (≈16 ms apart, about one 60 Hz frame). The `before`
  sample was unimodal (63.3–72.5 ms). Both `after` runs also needed 2–3 re-syncs
  where the `before` run needed none. The likely cause is machine load, not the
  migration: the `after` session had **419 processes** plus an agent working,
  against 377 for `before`, and the prerequisites above ask for a quiet machine.
  **The +4.3 ms p50 sits just above the ~1–3 ms resolution this harness can
  claim, and the p95 delta is a distribution-shape artifact.** A re-run on an
  idle machine is needed to call it either way. For scale, the migration's total
  measured cost is 1,378 bytes of gzip — it is not a plausible source of 15 ms.
- **Memory is overwhelmingly WebView2, not xon.** Of 251.3 MB private at idle,
  the Rust process accounts for **14.5 MB**; the other ~237 MB is six
  `msedgewebview2.exe` processes. That is the unavoidable cost of a webview UI on
  Windows, and it is why "rewrite the frontend in a framework" would change
  nothing. The number xon actually controls is the 14.5 MB. Note that private and
  working set moved in *opposite* directions (277.4 → 251.3 private, 437.7 → 518.4
  working set), so WebView2's allocation split is not stable enough for a 26 MB
  private-bytes delta to mean anything on its own.
- **CPU while idle is 0.21%.** Fine, and not worth chasing.
- **Cold start is `n/a`** here, as in every baseline recorded so far — it needs
  `-ColdStart`, which is opt-in because it kills the running instance.

## Files

- `bench.ps1` — the whole measurement protocol; no dependencies beyond Win32.
- `results/latest.md` — full report with environment and raw samples.
- `results/table.md` — just the Markdown table.

## Why this exists

xon's README used to claim "sub-millisecond response times". That is not
falsifiable and not reproducible. A claim like that is only worth making if
there is a script anyone can run that prints it again — this is that script.
If the numbers ever stop supporting the claim, the claim should change.
