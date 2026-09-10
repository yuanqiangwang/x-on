#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Black-box performance benchmark for the xon launcher (Windows).

.DESCRIPTION
    Measures the launcher the same way a user experiences it, from the outside:
    no instrumentation inside xon, no dev hooks, just synthetic keystrokes and
    Win32 window polling.

      hotkey   Alt+Space  -> window visible
      typing   one char   -> window grew (results painted)
      cold     process start -> first usable window
      idle     CPU + memory while hidden
      size     installer / exe size on disk

    Everything is measured on one machine in one session; the report embeds the
    hardware, OS and app version so a number is never quoted without context.

.EXAMPLE
    pwsh -File benchmarks/bench.ps1
    pwsh -File benchmarks/bench.ps1 -HotkeyRuns 50 -UpdateReadme

.NOTES
    - Run a RELEASE build. Debug builds are unrepresentative and slower.
    - xon must already be running (or pass -ExePath to have this script start it).
    - Run from a quiet machine, on AC power, and do not touch mouse/keyboard
      while the run is in flight.
    - The terminal must run at the same integrity level as xon. If xon runs
      elevated, UIPI drops the synthetic keystrokes and every run times out.
#>
[CmdletBinding()]
param(
    [int]$HotkeyRuns = 25,
    [int]$TypingRuns = 15,
    [string]$ExePath = "",
    [string]$WindowTitle = "xon",
    [int]$TimeoutMs = 3000,
    [int]$SettleSeconds = 15,
    [int]$IdleSeconds = 15,
    [char]$TypingChar = 'c',
    [switch]$ColdStart,
    [switch]$SkipIdle,
    [switch]$UpdateReadme
)

$ErrorActionPreference = "Stop"

# --------------------------------------------------------------------------
# Win32 interop. keybd_event (not SendInput) keeps this to four flat calls:
# no INPUT struct size to get wrong, and injected input still trips
# RegisterHotKey, which is what tauri-plugin-global-shortcut uses.
# --------------------------------------------------------------------------
if (-not ("XonBench.Native" -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
namespace XonBench {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    public static class Native {
        [DllImport("user32.dll")]
        public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

        [DllImport("user32.dll")]
        public static extern bool IsWindowVisible(IntPtr hWnd);

        [DllImport("user32.dll")]
        public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

        [DllImport("user32.dll")]
        public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, IntPtr dwExtraInfo);

        [DllImport("user32.dll")]
        public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);

        [DllImport("user32.dll")]
        public static extern bool SetForegroundWindow(IntPtr hWnd);

        [DllImport("user32.dll")]
        public static extern IntPtr GetForegroundWindow();
    }
}
'@
}

$KEYEVENTF_KEYUP = 0x0002
$VK_MENU         = 0x12   # Alt
$VK_SPACE        = 0x20
$VK_ESCAPE       = 0x1B

function Send-Vk([byte]$vk) {
    [XonBench.Native]::keybd_event($vk, 0, 0, [IntPtr]::Zero)
    [XonBench.Native]::keybd_event($vk, 0, $KEYEVENTF_KEYUP, [IntPtr]::Zero)
}

function Send-Summon {
    # Alt down, Space down/up, Alt up — one synthetic Alt+Space.
    [XonBench.Native]::keybd_event($VK_MENU, 0, 0, [IntPtr]::Zero)
    [XonBench.Native]::keybd_event($VK_SPACE, 0, 0, [IntPtr]::Zero)
    [XonBench.Native]::keybd_event($VK_SPACE, 0, $KEYEVENTF_KEYUP, [IntPtr]::Zero)
    [XonBench.Native]::keybd_event($VK_MENU, 0, $KEYEVENTF_KEYUP, [IntPtr]::Zero)
}

function Get-WindowText([IntPtr]$h) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][XonBench.Native]::GetWindowTextW($h, $sb, 256)
    return $sb.ToString()
}

function Get-TopLevelWindows([uint32]$ownerPid) {
    $found = New-Object System.Collections.Generic.List[IntPtr]
    $cb = [XonBench.EnumWindowsProc]{
        param([IntPtr]$h, [IntPtr]$l)
        $p = [uint32]0
        [void][XonBench.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ownerPid) { $found.Add($h) }
        return $true
    }.GetNewClosure()
    [void][XonBench.Native]::EnumWindows($cb, [IntPtr]::Zero)
    return , $found.ToArray()
}

function Get-XonHwnd {
    # Two traps here, both verified on a real xon 0.6.0:
    #   1. Process.MainWindowHandle points at a 13x13 hidden helper surface
    #      (title "com.xon.launcher-siw"), NOT the launcher panel.
    #   2. FindWindowW never finds the panel even though its title IS "xon".
    # So: enumerate top-level windows owned by the process, prefer the one whose
    # title matches, then the largest titled window, then the largest overall.
    $p = $null
    if ($script:xonProc) {
        try { if (-not $script:xonProc.HasExited) { $p = $script:xonProc } } catch { }
    }
    if (-not $p) {
        $p = Get-Process -Name $procName -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($p) { $script:xonProc = $p }
    }
    if (-not $p) { return $null }

    $wins = Get-TopLevelWindows ([uint32]$p.Id)
    if (-not $wins -or $wins.Count -eq 0) { return $null }

    foreach ($h in $wins) {
        if ((Get-WindowText $h) -eq $WindowTitle) { return $h }
    }

    $best = $null; $bestArea = -1
    foreach ($h in $wins) {
        if (-not (Get-WindowText $h)) { continue }   # skip title-less helper surfaces
        $r = New-Object XonBench.RECT
        [void][XonBench.Native]::GetWindowRect($h, [ref]$r)
        $area = ($r.Right - $r.Left) * ($r.Bottom - $r.Top)
        if ($area -gt $bestArea) { $bestArea = $area; $best = $h }
    }
    if ($best) { return $best }

    $best = $null; $bestArea = -1
    foreach ($h in $wins) {
        $r = New-Object XonBench.RECT
        [void][XonBench.Native]::GetWindowRect($h, [ref]$r)
        $area = ($r.Right - $r.Left) * ($r.Bottom - $r.Top)
        if ($area -gt $bestArea) { $bestArea = $area; $best = $h }
    }
    return $best
}

function Test-Visible([IntPtr]$h) {
    if ($h -eq [IntPtr]::Zero) { return $false }
    return [XonBench.Native]::IsWindowVisible($h)
}

function Get-WindowHeight([IntPtr]$h) {
    $r = New-Object XonBench.RECT
    [void][XonBench.Native]::GetWindowRect($h, [ref]$r)
    return ($r.Bottom - $r.Top)
}

function Wait-Until([scriptblock]$pred, [int]$limitMs) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalMilliseconds -lt $limitMs) {
        if (& $pred) { return $sw.Elapsed.TotalMilliseconds }
        [System.Threading.Thread]::SpinWait(40)
    }
    return $null
}

# SetForegroundWindow is gated by the Windows foreground-lock: a process that is not
# itself in the foreground cannot pull another window forward, so the synthetic
# keystroke lands on whatever was focused and the run times out. AttachThreadInput
# to the current foreground thread lifts that restriction for the duration of the
# call. Without this, every other typing run loses focus (verified by _diag.ps1).
function Set-Fore([IntPtr]$h) {
    if ([XonBench.Native]::GetForegroundWindow() -eq $h) { return $true }
    $fg = [XonBench.Native]::GetForegroundWindow()
    $fgT = [uint32]0; [void][XonBench.Native]::GetWindowThreadProcessId($fg, [ref]$fgT)
    $curT = [uint32]0; [void][XonBench.Native]::GetWindowThreadProcessId($h, [ref]$curT)
    $attached = $false
    if ($fgT -ne 0 -and $curT -ne 0 -and $fgT -ne $curT) {
        [void][XonBench.Native]::AttachThreadInput($fgT, $curT, $true)
        $attached = $true
    }
    $ok = [XonBench.Native]::SetForegroundWindow($h)
    if ($attached) { [void][XonBench.Native]::AttachThreadInput($fgT, $curT, $false) }
    return $ok
}

function Ensure-Hidden([IntPtr]$h) {
    if (-not (Test-Visible $h)) { return $true }
    Send-Vk $VK_ESCAPE
    if ((Wait-Until { -not (Test-Visible $h) } 1200) -ne $null) { return $true }
    # Fallback: the accelerator is a toggle.
    Send-Summon
    if ((Wait-Until { -not (Test-Visible $h) } 1200) -ne $null) { return $true }
    return $false
}

function Get-Stats([double[]]$samples) {
    if ($samples.Count -eq 0) { return $null }
    $sorted = @($samples | Sort-Object)
    $n = $sorted.Count
    $sum = 0.0; foreach ($s in $sorted) { $sum += $s }
    $p = [Math]::Clamp([int][Math]::Ceiling(0.95 * $n) - 1, 0, $n - 1)
    return @{
        n      = $n
        min    = $sorted[0]
        median = $sorted[[int][Math]::Floor($n / 2)]
        mean   = $sum / $n
        p95    = $sorted[$p]
        max    = $sorted[$n - 1]
        all    = ($sorted | ForEach-Object { $_.ToString("F1") }) -join ", "
    }
}

# --------------------------------------------------------------------------
# Resolve the binary under test
# --------------------------------------------------------------------------
$root = Split-Path -Parent $PSScriptRoot
if (-not $ExePath) {
    $candidates = @(
        (Join-Path $root "src-tauri/target/release/xon.exe"),
        (Join-Path $root "src-tauri/target/debug/xon.exe")
    ) | Where-Object { Test-Path $_ }
    if ($candidates.Count -gt 0) { $ExePath = $candidates[0] }
}
$procName = "xon"
if ($ExePath) { $procName = [System.IO.Path]::GetFileNameWithoutExtension($ExePath) }

$appVersion = "unknown"
$versionFile = Join-Path $root "src-tauri/tauri.conf.json"
if (Test-Path $versionFile) {
    try {
        $appVersion = (Get-Content $versionFile -Raw | ConvertFrom-Json).version
    } catch { }
}

Write-Host ""
Write-Host "  xon benchmark" -ForegroundColor Green
Write-Host "  window title : $WindowTitle"
Write-Host "  binary       : $(if ($ExePath) { $ExePath } else { '(not found - measuring a running xon)' })"
Write-Host "  version      : $appVersion"
Write-Host "  runs         : hotkey x$HotkeyRuns, typing x$TypingRuns"
Write-Host ""

# --------------------------------------------------------------------------
# Locate (or start) the app
# --------------------------------------------------------------------------
$script:xonProc = $null
$hwnd = Get-XonHwnd
if (-not $hwnd -and $ExePath) {
    Write-Host "  starting $procName ..."
    $script:xonProc = Start-Process -FilePath $ExePath -PassThru
    $null = Wait-Until { (Get-XonHwnd) -ne $null } 15000
    $hwnd = Get-XonHwnd

    # A freshly launched instance does not answer the accelerator until setup
    # has registered it (the initial index scan runs first). The window exists
    # long before that, so without this probe the first runs time out and look
    # exactly like a regression. Probe until it answers, then hide again.
    Write-Host "  waiting for the accelerator ... " -NoNewline
    $ready = $false
    for ($t = 0; $t -lt 60; $t++) {
        $null = Ensure-Hidden $hwnd
        Start-Sleep -Milliseconds 150
        Send-Summon
        if ((Wait-Until { Test-Visible $hwnd } 700) -ne $null) { $ready = $true; break }
    }
    Write-Host $(if ($ready) { "live" } else { "TIMEOUT — numbers will be unreliable" })
    $null = Ensure-Hidden $hwnd
}
if (-not $hwnd) {
    throw "No window titled '$WindowTitle' found. Start xon (release build) first, or pass -ExePath."
}

# --------------------------------------------------------------------------
# Cold start -> usable (opt-in: it kills the running instance)
# --------------------------------------------------------------------------
$coldMs = $null
if ($ColdStart) {
    if (-not $ExePath) { Write-Host "  -ColdStart needs -ExePath; skipping." -ForegroundColor Yellow }
    else {
        Get-Process -Name $procName -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 800
        Write-Host "  cold start ... " -NoNewline
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $script:xonProc = Start-Process -FilePath $ExePath -PassThru
        while ($sw.Elapsed.TotalMilliseconds -lt 20000) {
            $h = Get-XonHwnd
            if ($h) {
                Send-Summon
                if ((Wait-Until { Test-Visible $h } 400) -ne $null) { break }
            }
            Start-Sleep -Milliseconds 25
        }
        $coldMs = $sw.Elapsed.TotalMilliseconds
        $hwnd = Get-XonHwnd
        Write-Host ("{0} ms" -f $coldMs.ToString("F0"))
        if (-not $hwnd) { throw "Cold start never produced a window." }
    }
}

# --------------------------------------------------------------------------
# hotkey -> visible
# --------------------------------------------------------------------------
Write-Host "  hotkey -> visible ... " -NoNewline
$hotkey = @()
for ($i = 0; $i -lt $HotkeyRuns; $i++) {
    if (-not (Ensure-Hidden $hwnd)) { Write-Host "`n  run $i : could not hide window"; break }
    Start-Sleep -Milliseconds 150

    # Time from the keystroke, not from the start of polling: the injected
    # input is queued asynchronously, so whatever happens between the two
    # calls is part of what the user waits for.
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Send-Summon
    $hit = Wait-Until { Test-Visible $hwnd } $TimeoutMs
    $sw.Stop()

    if ($hit -eq $null) { Write-Host "`n  run $i : timeout (hotkey did not summon)"; break }
    $hotkey += $sw.Elapsed.TotalMilliseconds
    Start-Sleep -Milliseconds 120
}
Write-Host ("{0} runs" -f $hotkey.Count)

# --------------------------------------------------------------------------
# keystroke -> results painted (proxy: the window grows)
# --------------------------------------------------------------------------
Write-Host "  keystroke -> results ... " -NoNewline
$typing = @()
$vkChar = [byte][int][char]::ToUpper($TypingChar)

for ($i = 0; $i -lt $TypingRuns; $i++) {
    if (-not (Ensure-Hidden $hwnd)) { Write-Host "`n  run $i : could not hide window"; break }
    Start-Sleep -Milliseconds 150

    Send-Summon
    if ((Wait-Until { Test-Visible $hwnd } $TimeoutMs) -eq $null) {
        # One retry. An Esc that races with an in-flight render can leave the
        # toggle desynced; re-establish the hidden state instead of assuming it.
        $null = Ensure-Hidden $hwnd
        Start-Sleep -Milliseconds 400
        Send-Summon
        if ((Wait-Until { Test-Visible $hwnd } $TimeoutMs) -eq $null) {
            Write-Host "`n  run $i : summon timeout"; break
        }
    }
    # The character lands wherever focus is. Wait for the panel to actually own
    # it rather than assuming a fixed sleep — a dropped character looks exactly
    # like "no results" from the outside.
    $null = Set-Fore $hwnd
    $null = Wait-Until { [XonBench.Native]::GetForegroundWindow() -eq $hwnd } 1500
    # The window being foreground is not the same as the <input> having focus:
    # xon refocuses it from the `wake` event, which arrives over IPC after the
    # show. Waiting only for the foreground test returns too early and the
    # character gets dropped — which looks identical to "no results". Empirically
    # the focus can land 200-500ms after show on a busy machine, so wait generously.
    Start-Sleep -Milliseconds 600

    $base = Get-WindowHeight $hwnd
    $target = $base + 20   # one result row is ROW_H(38) logical px, DPI-scaled

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Send-Vk $vkChar
    $hit = Wait-Until { (Get-WindowHeight $hwnd) -ge $target } 800
    $sw.Stop()

    if ($hit -eq $null) {
        # One re-sync: a stale query from the previous run ("cc") or a dropped
        # keystroke both look like this. Reset and try the same run once.
        # Print the state that distinguishes the two — silent failure here made
        # this look like "typing is just broken" once.
        $fgOk = ([XonBench.Native]::GetForegroundWindow() -eq $hwnd)
        Write-Host "`n  run $i : no growth (h=$base target=$target focus=$fgOk), re-syncing"
        Send-Vk $VK_ESCAPE
        $null = Wait-Until { -not (Test-Visible $hwnd) } 1500
        Start-Sleep -Milliseconds 200
        Send-Summon
        if ((Wait-Until { Test-Visible $hwnd } $TimeoutMs) -eq $null) {
            Write-Host "`n  run $i : re-sync summon timeout"; break
        }
        $null = Set-Fore $hwnd
        $null = Wait-Until { [XonBench.Native]::GetForegroundWindow() -eq $hwnd } 1500
        Start-Sleep -Milliseconds 600

        $base = Get-WindowHeight $hwnd
        $target = $base + 20
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        Send-Vk $vkChar
        $hit = Wait-Until { (Get-WindowHeight $hwnd) -ge $target } 800
        $sw.Stop()
        if ($hit -eq $null) {
            Write-Host "`n  run $i : no growth (typed '$TypingChar' matched nothing?)"; break
        }
    }
    $typing += $sw.Elapsed.TotalMilliseconds
    Start-Sleep -Milliseconds 120
}
Write-Host ("{0} runs" -f $typing.Count)

# --------------------------------------------------------------------------
# Idle cost
# --------------------------------------------------------------------------
$idleCpuPct = $null
$memMb = $null
$memPrivateMb = $null
$procCount = 0

if (-not $SkipIdle) {
    if (-not (Ensure-Hidden $hwnd)) { Write-Host "  (window still visible; idle numbers are polluted)" -ForegroundColor Yellow }

    function Get-Tree([int]$rootId) {
        # Recurse. WebView2 spawns its utility/gpu/renderer processes as
        # grandchildren, so a single level hides ~55% of the group's memory
        # (measured: 229 MB one level vs 518 MB working set for the full tree).
        # NB: not named $pid — that is a read-only automatic variable.
        $seen = @{}
        $out = New-Object System.Collections.Generic.List[object]
        $queue = [System.Collections.Queue]::new()
        $queue.Enqueue([int]$rootId)
        $depth = @{ }
        $depth[[int]$rootId] = 0
        while ($queue.Count -gt 0) {
            $id = [int]$queue.Dequeue()
            if ($seen.ContainsKey($id)) { continue }
            $seen[$id] = $true
            $proc = Get-Process -Id $id -ErrorAction SilentlyContinue
            if ($proc) { $out.Add($proc) }
            if ($depth[$id] -lt 4) {
                Get-CimInstance Win32_Process -Filter "ParentProcessId = $id" -ErrorAction SilentlyContinue |
                    ForEach-Object {
                        $kid = [int]$_.ProcessId
                        if (-not $seen.ContainsKey($kid)) {
                            $depth[$kid] = $depth[$id] + 1
                            $queue.Enqueue($kid)
                        }
                    }
            }
        }
        return , $out.ToArray()
    }

    Write-Host "  settling $SettleSeconds s, then sampling ${IdleSeconds}s idle ... " -NoNewline
    Start-Sleep -Seconds $SettleSeconds

    $procs = Get-Process -Name $procName -ErrorAction SilentlyContinue
    if ($procs) {
        $main = $procs | Sort-Object -Property WorkingSet64 -Descending | Select-Object -First 1
        $tree = Get-Tree $main.Id
        $procCount = $tree.Count
        $cpuBefore = ($tree | Measure-Object -Property CPU -Sum).Sum
        # Two figures on purpose. Working set double-counts pages shared between
        # processes (WebView2 maps the same DLLs everywhere), so it overstates
        # cost; private bytes is what the group actually commits. Report both,
        # lead with private.
        $memMb = [Math]::Round((($tree | Measure-Object -Property WorkingSet64 -Sum).Sum) / 1MB, 1)
        $memPrivateMb = [Math]::Round((($tree | Measure-Object -Property PrivateMemorySize64 -Sum).Sum) / 1MB, 1)
        $treeDetail = $tree | Sort-Object -Property PrivateMemorySize64 -Descending |
            ForEach-Object {
                "  - {0} (pid {1}): private {2:N1} MB, working set {3:N1} MB" -f `
                    $_.ProcessName, $_.Id, ($_.PrivateMemorySize64 / 1MB), ($_.WorkingSet64 / 1MB)
            }

        Start-Sleep -Seconds $IdleSeconds

        $tree2 = Get-Tree $main.Id
        $cpuAfter = ($tree2 | Measure-Object -Property CPU -Sum).Sum
        $elapsed = [Math]::Max(1, $IdleSeconds)
        $idleCpuPct = [Math]::Round(($cpuAfter - $cpuBefore) / $elapsed * 100, 2)
    }
    Write-Host "done"
}

# --------------------------------------------------------------------------
# Size on disk — distribution artifact only
# --------------------------------------------------------------------------
$sizeMb = $null
$sizeLabel = "n/a"
if ($ExePath -and (Test-Path $ExePath)) {
    # Only what ships. Summing every .exe/.dll under target/release also counts
    # build-script-build.exe and deps/*.rlib — 206 MB, of which ~194 MB is
    # compiler output that never leaves the machine. Prefer the newest
    # installer, fall back to the bare exe.
    $relDir = Split-Path -Parent $ExePath
    $installer = Get-ChildItem -Path (Join-Path $relDir "bundle") -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Extension -in '.exe', '.msi' } |
        Sort-Object -Property LastWriteTime -Descending |
        Select-Object -First 1
    if ($installer) {
        $sizeMb = [Math]::Round($installer.Length / 1MB, 2)
        $sizeLabel = $installer.Name
    } else {
        $sizeMb = [Math]::Round((Get-Item $ExePath).Length / 1MB, 2)
        $sizeLabel = Split-Path -Leaf $ExePath
    }
}

# --------------------------------------------------------------------------
# Report
# --------------------------------------------------------------------------
$hs = Get-Stats $hotkey
$ts = Get-Stats $typing

$os = (Get-CimInstance Win32_OperatingSystem).Caption.Trim()
$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
$ramGb = [Math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 0)
$stamp = Get-Date -Format "yyyy-MM-dd HH:mm"

$resultsDir = Join-Path $PSScriptRoot "results"
$null = New-Item -ItemType Directory -Force -Path $resultsDir

$rows = [ordered]@{
    "Hotkey -> window visible (p50)" = if ($hs) { "{0:N1} ms" -f $hs.median } else { "n/a" }
    "Hotkey -> window visible (p95)" = if ($hs) { "{0:N1} ms" -f $hs.p95 } else { "n/a" }
    "Keystroke -> results (p50)"     = if ($ts) { "{0:N1} ms" -f $ts.median } else { "n/a" }
    "Keystroke -> results (p95)"     = if ($ts) { "{0:N1} ms" -f $ts.p95 } else { "n/a" }
    "Cold start -> usable"           = if ($coldMs) { "{0:N0} ms" -f $coldMs } else { "n/a" }
    "Memory, idle (private)"         = if ($memPrivateMb) { "{0:N1} MB" -f $memPrivateMb } else { "n/a" }
    "Memory, idle (working set)"     = if ($memMb) { "{0:N1} MB" -f $memMb } else { "n/a" }
    "CPU while idle"                 = if ($idleCpuPct -ne $null) { "{0:N2} %" -f $idleCpuPct } else { "n/a" }
    "Installer size"                 = if ($sizeMb) { "{0:N2} MB" -f $sizeMb } else { "n/a" }
}

$table = @()
$table += "| Metric | xon $appVersion |"
$table += "| --- | --- |"
foreach ($k in $rows.Keys) { $table += "| $k | $($rows[$k]) |" }
$tableText = $table -join "`n"

$report = @()
$report += "# xon benchmark — $stamp"
$report += ""
$report += "## Environment"
$report += ""
$report += "- OS: $os"
$report += "- CPU: $cpu"
$report += "- RAM: $ramGb GB"
$report += "- App: xon $appVersion ($(if ($ExePath) { $ExePath } else { 'running instance' }))"
$report += "- Runs: hotkey x$($hotkey.Count), typing x$($typing.Count)"
$report += ""
$report += "## Results"
$report += ""
$report += $tableText
$report += ""
$report += "## Raw samples"
$report += ""
if ($hs) {
    $report += "- hotkey (ms, sorted): $($hs.all)"
    $report += "  - min $($hs.min.ToString('F1')) / mean $($hs.mean.ToString('F1')) / max $($hs.max.ToString('F1'))"
}
if ($ts) {
    $report += "- typing (ms, sorted): $($ts.all)"
    $report += "  - min $($ts.min.ToString('F1')) / mean $($ts.mean.ToString('F1')) / max $($ts.max.ToString('F1'))"
}
$report += "- process group: $procCount process(es) — full tree, recursive"
if ($treeDetail) {
    $report += ""
    $report += "## Process breakdown at idle"
    $report += ""
    $report += $treeDetail
}
$report += "- installer measured: $sizeLabel (distribution artifact; excludes the"
$report += "  WebView2 runtime, which Windows already provides)"
$report += ""
$report += "Reproduce with: pwsh -File benchmarks/bench.ps1"
$report += "See benchmarks/README.md for what each metric does and does not capture."

$reportText = $report -join "`n"

Set-Content -Path (Join-Path $resultsDir "latest.md") -Value $reportText -Encoding utf8
Set-Content -Path (Join-Path $resultsDir "table.md") -Value $tableText -Encoding utf8

Write-Host ""
Write-Host $reportText
Write-Host ""
Write-Host "  written to benchmarks/results/latest.md" -ForegroundColor DarkGray

if ($UpdateReadme) {
    $readme = Join-Path $root "README.md"
    if (Test-Path $readme) {
        $content = Get-Content $readme -Raw
        $pattern = '(?s)(<!--\s*benchmarks:start\s*-->).*?(<!--\s*benchmarks:end\s*-->)'
        if ($content -match $pattern) {
            $new = $Matches[1] + "`n`n" + $tableText + "`n`n" + $Matches[2]
            $content = [regex]::Replace($content, $pattern, { $new })
            Set-Content -Path $readme -Value $content -Encoding utf8
            Write-Host "  README.md benchmark table updated." -ForegroundColor Green
        } else {
            Write-Host "  README.md has no <!-- benchmarks:start/end --> markers; skipped." -ForegroundColor Yellow
        }
    }
}
