# xon benchmark — 2026-09-17 13:34

## Environment

- OS: Microsoft Windows 11 家庭版 中文版
- CPU: Intel(R) Core(TM) Ultra 7 255H
- RAM: 31 GB
- App: xon 0.7.1 (D:\projects\x-on\src-tauri\target\release\xon.exe)
- Runs: hotkey x25, typing x15

## Results

| Metric | xon 0.7.1 |
| --- | --- |
| Hotkey -> window visible (p50) | 4.2 ms |
| Hotkey -> window visible (p95) | 7.0 ms |
| Keystroke -> results (p50) | 64.3 ms |
| Keystroke -> results (p95) | 65.7 ms |
| Cold start -> usable | n/a |
| Memory, idle (private) | 254.8 MB |
| Memory, idle (working set) | 505.3 MB |
| CPU while idle | 0.00 % |
| Installer size | 1.47 MB |

## Raw samples

- hotkey (ms, sorted): 2.7, 2.9, 3.0, 3.0, 3.1, 3.3, 3.7, 3.8, 3.8, 3.9, 3.9, 4.1, 4.2, 4.3, 4.5, 4.5, 4.6, 4.6, 4.6, 4.7, 4.7, 5.0, 6.3, 7.0, 19.0
  - min 2.7 / mean 4.8 / max 19.0
- typing (ms, sorted): 56.5, 62.9, 63.2, 63.4, 63.8, 64.0, 64.2, 64.3, 64.4, 64.6, 64.7, 65.1, 65.4, 65.4, 65.7
  - min 56.5 / mean 63.8 / max 65.7
- process group: 7 process(es) — full tree, recursive

## Process breakdown at idle

  - msedgewebview2 (pid 17392): private 90.0 MB, working set 108.0 MB
  - msedgewebview2 (pid 39520): private 76.5 MB, working set 169.2 MB
  - msedgewebview2 (pid 27204): private 51.4 MB, working set 100.2 MB
  - xon (pid 29244): private 14.6 MB, working set 59.1 MB
  - msedgewebview2 (pid 22736): private 11.6 MB, working set 37.1 MB
  - msedgewebview2 (pid 37676): private 7.7 MB, working set 18.4 MB
  - msedgewebview2 (pid 16452): private 3.0 MB, working set 13.3 MB
- installer measured: xon_0.7.1_x64-setup.exe (distribution artifact; excludes the
  WebView2 runtime, which Windows already provides)

Reproduce with: pwsh -File benchmarks/bench.ps1
See benchmarks/README.md for what each metric does and does not capture.
