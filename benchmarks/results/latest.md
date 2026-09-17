# xon benchmark — 2026-09-17 12:42

## Environment

- OS: Microsoft Windows 11 家庭版 中文版
- CPU: Intel(R) Core(TM) Ultra 7 255H
- RAM: 31 GB
- App: xon 0.7.1 (D:\projects\x-on\src-tauri\target\release\xon.exe)
- Runs: hotkey x25, typing x15

## Results

| Metric | xon 0.7.1 |
| --- | --- |
| Hotkey -> window visible (p50) | 4.3 ms |
| Hotkey -> window visible (p95) | 8.2 ms |
| Keystroke -> results (p50) | 69.0 ms |
| Keystroke -> results (p95) | 81.1 ms |
| Cold start -> usable | n/a |
| Memory, idle (private) | 251.3 MB |
| Memory, idle (working set) | 518.4 MB |
| CPU while idle | 0.21 % |
| Installer size | 1.47 MB |

## Raw samples

- hotkey (ms, sorted): 2.3, 2.7, 2.8, 2.9, 3.1, 3.1, 3.4, 3.4, 3.7, 3.7, 3.8, 4.0, 4.3, 4.4, 4.4, 4.5, 4.7, 4.7, 5.1, 5.4, 6.3, 7.3, 7.4, 8.2, 27.0
  - min 2.3 / mean 5.3 / max 27.0
- typing (ms, sorted): 63.8, 64.2, 64.7, 65.1, 65.6, 65.6, 65.9, 69.0, 79.1, 79.8, 79.8, 80.1, 80.9, 80.9, 81.1
  - min 63.8 / mean 72.4 / max 81.1
- process group: 7 process(es) — full tree, recursive

## Process breakdown at idle

  - msedgewebview2 (pid 41628): private 88.8 MB, working set 109.3 MB
  - msedgewebview2 (pid 36608): private 78.3 MB, working set 178.0 MB
  - msedgewebview2 (pid 36656): private 47.4 MB, working set 95.6 MB
  - xon (pid 42264): private 14.5 MB, working set 62.7 MB
  - msedgewebview2 (pid 31412): private 11.6 MB, working set 39.9 MB
  - msedgewebview2 (pid 8156): private 7.7 MB, working set 18.5 MB
  - msedgewebview2 (pid 3524): private 2.9 MB, working set 14.4 MB
- installer measured: xon_0.7.1_x64-setup.exe (distribution artifact; excludes the
  WebView2 runtime, which Windows already provides)

Reproduce with: pwsh -File benchmarks/bench.ps1
See benchmarks/README.md for what each metric does and does not capture.
