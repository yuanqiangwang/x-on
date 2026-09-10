# Benchmarks

Black-box measurement of the launcher — every number is taken from outside the
app, with synthetic keystrokes and Win32 window polling. Nothing is
instrumented inside xon, so the same procedure could be pointed at any other
launcher (Raycast, uTools, PowerToys Run) and the comparison stays fair.

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
| **Memory, idle (working set)** | Sum of `WorkingSet64` over the same tree. | It **double-counts** physical pages shared between processes, so it reads much higher — 521 MB vs 278 MB private on the baseline machine. Useful only as an upper bound. |
| **CPU while idle** | `Process.CPU` delta over the sampling window, divided by wall time. Percent of one core. | Wakeups/s. A process can idle at 0.1% CPU and still wake hundreds of times a second, which is what actually costs battery. Not measured here (no `powermetrics` equivalent on Windows without ETW). |
| **Installer size** | The newest artifact under `target/release/bundle/` (NSIS `.exe` or `.msi`); falls back to the bare `xon.exe` if no bundle was built. | The WebView2 runtime, which Windows already provides — so this is *not* comparable to a bundled-Chromium app's on-disk size. |

### Two definitions that produced wrong numbers

Both of these were shipped in the first version of this script. Recording them
so the same mistake is not made twice.

- **"Size on disk" summed every `.exe`/`.dll` under `target/release` → 206 MB.**
  That is the Cargo build directory: `build/` (126 MB of `build-script-build.exe`)
  plus `deps/` (68 MB of intermediate `.rlib`/`.dll`). None of it ships. The real
  installer is **1.58 MB**.
- **"Memory" only walked one level of the process tree → 229 MB.** WebView2
  spawns its utility/gpu/renderer processes as *grandchildren*, so a single level
  captured 2 of 7 processes and missed 289 MB. Recursive walk: **278 MB private /
  521 MB working set over 7 processes.**

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
parameter is named `$procId`, not `$pid` — `$pid` is a read-only automatic
variable.

## Interpreting the current baseline

As of 2026-09-10 (Windows 11, Core Ultra 7 255H, xon 0.6.0 release):

- **Hotkey is not the bottleneck.** p50 8.6 ms / p95 14.9 ms — comfortably
  ahead of the 12–17 ms an Electron-free Swift/AppKit launcher reports for the
  same measurement. There is little left to win here.
- **Keystroke→results is ~8× slower** (p50 66.8 ms) and is where the perceived
  latency lives. Roughly 40 ms of that is the fixed input debounce; the rest is
  filter + full list rebuild + icon IPC. **Optimizing the summon path cannot
  improve this number.**
- **Memory is overwhelmingly WebView2, not xon.** Of 278 MB private at idle,
  the Rust process accounts for **19 MB**; the other **259 MB** is six
  `msedgewebview2.exe` processes. That is the unavoidable cost of a webview UI
  on Windows, and it is why "rewrite the frontend in a framework" would change
  nothing. The number that xon actually controls is the 19 MB.
- **CPU while idle is ~0.3%**, in the same band as asyar's 0.66%. Fine, and not
  worth chasing.

## Files

- `bench.ps1` — the whole measurement protocol; no dependencies beyond Win32.
- `results/latest.md` — full report with environment and raw samples.
- `results/table.md` — just the Markdown table.

## Why this exists

xon's README used to claim "sub-millisecond response times". That is not
falsifiable and not reproducible. A claim like that is only worth making if
there is a script anyone can run that prints it again — this is that script.
If the numbers ever stop supporting the claim, the claim should change.
