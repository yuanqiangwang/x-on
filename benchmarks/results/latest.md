# xon benchmark — 2026-09-17 10:30

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
| Hotkey -> window visible (p95) | 11.1 ms |
| Keystroke -> results (p50) | 64.7 ms |
| Keystroke -> results (p95) | 66.2 ms |
| Cold start -> usable | n/a |
| Memory, idle (private) | 277.4 MB |
| Memory, idle (working set) | 437.7 MB |
| CPU while idle | 0.62 % |
| Installer size | 1.47 MB |

## Raw samples

- hotkey (ms, sorted): 3.0, 3.0, 3.1, 3.1, 3.2, 3.2, 3.7, 3.7, 3.9, 4.0, 4.1, 4.1, 4.3, 4.5, 4.5, 4.6, 4.7, 4.8, 5.2, 5.8, 6.5, 6.8, 7.4, 11.1, 19.7
  - min 3.0 / mean 5.3 / max 19.7
- typing (ms, sorted): 55.4, 62.9, 63.7, 63.9, 64.2, 64.4, 64.5, 64.7, 64.7, 64.8, 64.9, 65.0, 65.7, 66.1, 66.2
  - min 55.4 / mean 64.1 / max 66.2
- process group: 7 process(es) — full tree, recursive

## Process breakdown at idle

  - msedgewebview2 (pid 13060): private 96.7 MB, working set 87.6 MB
  - msedgewebview2 (pid 27240): private 88.4 MB, working set 133.6 MB
  - msedgewebview2 (pid 34136): private 52.5 MB, working set 90.3 MB
  - xon (pid 21868): private 17.7 MB, working set 61.2 MB
  - msedgewebview2 (pid 23868): private 11.6 MB, working set 36.6 MB
  - msedgewebview2 (pid 22244): private 7.6 MB, working set 16.4 MB
  - msedgewebview2 (pid 23092): private 2.8 MB, working set 12.0 MB
- installer measured: xon_0.7.1_x64-setup.exe (distribution artifact; excludes the
  WebView2 runtime, which Windows already provides)

Reproduce with: pwsh -File benchmarks/bench.ps1
See benchmarks/README.md for what each metric does and does not capture.
