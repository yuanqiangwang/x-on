# xon benchmark — 2026-09-10 14:54

## Environment

- OS: Microsoft Windows 11 家庭版 中文版
- CPU: Intel(R) Core(TM) Ultra 7 255H
- RAM: 31 GB
- App: xon 0.6.0 (D:\projects\x-on\src-tauri\target\release\xon.exe)
- Runs: hotkey x25, typing x15

## Results

| Metric | xon 0.6.0 |
| --- | --- |
| Hotkey -> window visible (p50) | 8.9 ms |
| Hotkey -> window visible (p95) | 14.6 ms |
| Keystroke -> results (p50) | 63.3 ms |
| Keystroke -> results (p95) | 72.5 ms |
| Cold start -> usable | n/a |
| Memory, idle (private) | 276.7 MB |
| Memory, idle (working set) | 523.6 MB |
| CPU while idle | 0.10 % |
| Installer size | 1.58 MB |

## Raw samples

- hotkey (ms, sorted): 5.7, 6.8, 6.9, 7.3, 7.6, 7.8, 7.9, 7.9, 8.0, 8.2, 8.3, 8.7, 8.9, 8.9, 8.9, 9.5, 9.6, 9.7, 9.8, 9.9, 10.8, 10.9, 12.8, 14.6, 27.1
  - min 5.7 / mean 9.7 / max 27.1
- typing (ms, sorted): 54.5, 54.9, 61.1, 61.3, 62.8, 62.8, 63.1, 63.3, 63.7, 64.3, 65.1, 67.0, 69.1, 70.8, 72.5
  - min 54.5 / mean 63.8 / max 72.5
- process group: 7 process(es) — full tree, recursive

## Process breakdown at idle

  - msedgewebview2 (pid 11948): private 95.3 MB, working set 112.2 MB
  - msedgewebview2 (pid 29880): private 77.4 MB, working set 171.9 MB
  - msedgewebview2 (pid 35432): private 60.4 MB, working set 106.3 MB
  - xon (pid 21216): private 20.6 MB, working set 63.3 MB
  - msedgewebview2 (pid 28568): private 11.8 MB, working set 37.4 MB
  - msedgewebview2 (pid 32696): private 8.4 MB, working set 19.7 MB
  - msedgewebview2 (pid 34784): private 2.9 MB, working set 12.9 MB
- installer measured: xon_0.6.0_x64-setup.exe (distribution artifact; excludes the
  WebView2 runtime, which Windows already provides)

Reproduce with: pwsh -File benchmarks/bench.ps1
See benchmarks/README.md for what each metric does and does not capture.
