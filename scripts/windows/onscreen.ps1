# On-screen proofs of the Windows backend's system UI, in the signed-in
# desktop session, with the counter's debug build (`cargo build -p counter`),
# on a PC someone else may be using. Run it there through run-in-session.ps1:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run-in-session.ps1 -TimeoutSeconds 120 `
#     -Command "powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\onscreen.ps1 -Case share"
#
# Each case first reads how long the session has had no input
# (GetLastInputInfo): under -MinIdle seconds (300) it shows nothing and
# exits 3, so nobody at the PC is disturbed; with the lock screen up, 4.
# Its files go to target\onscreen: <case>.txt (what it saw), the counter's
# <case>.log and PNGs. A screen capture keeps only the app's window and the
# dialog: everything else in its rectangle is painted gray.
#
# - s41-none: /components/media shows a local page that calls window.open
#   1.5 s after it loads, without a user gesture. Nothing may open: no new
#   process of the default browser (or of any common browser), no new
#   window; the app logs the refusal.
# - s41-tap: the same page, and a real click (SendInput) on its
#   target=_blank link to https://example.com: the app logs that it hands
#   the link to the browser, and a new browser process (or window) shows.
#   The browser is never captured: <case>-screen.png is deleted. A PC
#   whose https handler opens no browser (a stub in place of a removed
#   Edge) shows the hand-off in the log only.
# - s41-browser: s41-tap with PRISM_BROWSER set to Chrome (or Brave) and a
#   throwaway profile in %TEMP%: a browser window shows Example Domain.
#   That browser is then closed and its profile deleted; it runs only when
#   no process of that browser ran before (it would be the PC owner's).
# - share: "Share" on /components opens Windows' Share dialog over the
#   window; a capture, then Escape closes it (UI Automation times out
#   on the Share window). No target is picked.
# - passkey -Attended: "Create a passkey" on /settings opens Windows'
#   passkey dialog; a capture, then a person at the PC presses Cancel
#   within 60 s, and the app logs `passkeys: Cancelled` and shows
#   "Cancelled.". Only a person can close that dialog: it runs above the
#   app's integrity level (UI Automation sees nothing in it, Escape and
#   clicks from a standard process never reach it, its process cannot be
#   stopped), and on Windows 11 25H2 neither the request's timeout nor
#   WebAuthNCancelCurrentOperation closes its "Choose where to save your
#   passkey" screen, even once the app has quit. Without -Attended the
#   case refuses to run.
# - idle: only prints the idle time.
#
# Exit codes: 0 done, 1 a check failed, 2 passkey without -Attended, 3 the
# PC is in use (not checked with -Attended: the person is there for it),
# 4 locked.
param(
  [Parameter(Mandatory)]
  [ValidateSet('idle', 's41-none', 's41-tap', 's41-browser', 'share', 'passkey')] [string]$Case,
  [string]$Exe = 'target\debug\counter.exe',
  # Where the click lands for s41-tap, in DIPs from the "A web page" heading.
  [string]$At = '160,150',
  [int]$MinIdle = 300,
  # passkey: a person at the PC will press Cancel.
  [switch]$Attended
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version 2
$repo = (Get-Location).ProviderPath
$out = Join-Path $repo 'target\onscreen'
New-Item -ItemType Directory -Force $out | Out-Null
$report = Join-Path $out "$Case.txt"
Remove-Item $report -ErrorAction SilentlyContinue
$failed = 0

# Printed, and kept in <case>.txt (never a function's return value).
function Say([string]$s) {
  [Console]::Out.WriteLine($s)
  Add-Content -LiteralPath $report -Value $s -Encoding UTF8
}

Add-Type -AssemblyName System.Drawing, UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class OnScreen {
  [StructLayout(LayoutKind.Sequential)] struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LASTINPUTINFO i);
  [DllImport("kernel32.dll")] static extern uint GetTickCount();
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int size);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int size);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);

  public static int IdleSeconds() {
    var i = new LASTINPUTINFO();
    i.cbSize = (uint)Marshal.SizeOf(i);
    GetLastInputInfo(ref i);
    return (int)((GetTickCount() - i.dwTime) / 1000);
  }
  public class Win { public long Handle; public int Pid; public string Class; public string Name; public int X, Y, W, H; }
  // Visible, uncloaked top-level windows, with their frame as DWM draws it.
  public static List<Win> Windows() {
    var list = new List<Win>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      if (!IsWindowVisible(h)) return true;
      int cloaked;
      if (DwmGetWindowAttribute(h, 14, out cloaked, 4) == 0 && cloaked != 0) return true;
      RECT r;
      if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) GetWindowRect(h, out r);
      if (r.Right - r.Left < 2 || r.Bottom - r.Top < 2) return true;
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      var sb = new StringBuilder(256);
      GetClassName(h, sb, 256);
      var w = new Win();
      w.Handle = h.ToInt64(); w.Pid = (int)pid; w.Class = sb.ToString();
      w.X = r.Left; w.Y = r.Top; w.W = r.Right - r.Left; w.H = r.Bottom - r.Top;
      list.Add(w);
      return true;
    }, IntPtr.Zero);
    return list;
  }
  // Shown: a window, visible, not cloaked (the Share window is cloaked, not
  // destroyed, when it closes).
  public static bool Exists(long h) {
    var w = new IntPtr(h);
    int cloaked;
    if (!IsWindow(w) || !IsWindowVisible(w)) return false;
    return !(DwmGetWindowAttribute(w, 14, out cloaked, 4) == 0 && cloaked != 0);
  }
  // Whether a window's title starts with `prefix` (titles are never printed:
  // a browser's would name the owner's tabs).
  public static bool TitleStarts(long h, string prefix) {
    var sb = new StringBuilder(512);
    GetWindowText(new IntPtr(h), sb, 512);
    return sb.ToString().StartsWith(prefix, StringComparison.OrdinalIgnoreCase);
  }
  public static void Escape() {
    keybd_event(0x1B, 0, 0, UIntPtr.Zero);
    keybd_event(0x1B, 0, 2, UIntPtr.Zero);
  }
}
'@
[void][OnScreen]::SetProcessDPIAware()

# --- Is anyone using the PC?
$idle = [OnScreen]::IdleSeconds()
$session = (Get-Process -Id $PID).SessionId
$locked = $null -ne (Get-Process -Name LogonUI -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $session })
Say ("{0:s} idle {1} s, lock screen {2}" -f (Get-Date), $idle, $(if ($locked) { 'up' } else { 'not up' }))
if ($Case -eq 'idle') { exit 0 }
if ($Case -eq 'passkey' -and -not $Attended) {
  Say 'passkey: only a person can close Windows'' passkey dialog: run it with -Attended while someone is at the PC'
  exit 2
}
if ($locked) { Say 'locked: nothing shown'; exit 4 }
if ($idle -lt $MinIdle -and -not $Attended) { Say "in use: input $idle s ago (want $MinIdle s): nothing shown"; exit 3 }

# --- Helpers
function Get-ProcName([int]$id) {
  $p = Get-Process -Id $id -ErrorAction SilentlyContinue
  if ($p) { $p.ProcessName } else { '?' }
}

function Format-Win($w) {
  $n = if ($w.Name) { $w.Name } else { Get-ProcName $w.Pid }
  '{0} (pid {1}) class {2} at {3},{4} {5}x{6}' -f $n, $w.Pid, $w.Class, $w.X, $w.Y, $w.W, $w.H
}

# The default browser for https links (read only).
function Get-DefaultBrowser {
  $prog = $null
  foreach ($k in 'UserChoiceLatest', 'UserChoice') {
    $v = Get-ItemProperty "HKCU:\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\$k" -ErrorAction SilentlyContinue
    if ($v -and ($v.PSObject.Properties.Name -contains 'ProgId')) { $prog = $v.ProgId; break }
  }
  $exe = $null
  if ($prog) {
    $c = Get-ItemProperty "Registry::HKEY_CLASSES_ROOT\$prog\shell\open\command" -ErrorAction SilentlyContinue
    if ($c) {
      $line = $c.'(default)'
      if ($line -match '^"([^"]+)"') { $exe = $Matches[1] } elseif ($line) { $exe = ($line -split ' ')[0] }
    }
  }
  $name = if ($exe) { [IO.Path]::GetFileNameWithoutExtension($exe) } else { $null }
  # Edge's handler may name a stub: the browser is msedge.
  if ($prog -like 'MSEdge*') { $name = 'msedge' }
  elseif ($prog -like 'ChromeHTML*') { $name = 'chrome' }
  elseif ($prog -like 'FirefoxURL*') { $name = 'firefox' }
  [pscustomobject]@{ ProgId = $prog; Name = $name }
}
$browserNames = @('msedge', 'chrome', 'firefox', 'brave', 'opera', 'vivaldi', 'iexplore')

function Get-Browsers {
  @(Get-Process -Name $browserNames -ErrorAction SilentlyContinue | ForEach-Object {
      [pscustomobject]@{ Name = $_.ProcessName; Id = $_.Id; Start = $_.StartTime }
    })
}

$exePath = (Resolve-Path -LiteralPath (Join-Path $repo $Exe)).ProviderPath
$data = Join-Path $env:TEMP "prism-onscreen-$Case"

# Starts the counter with `vars`, its own data folder, its output in <case>.log.
function Start-Counter([hashtable]$vars) {
  Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
  $env:LOCALAPPDATA = $data
  $env:PRISM_LOCALE = 'en'
  $env:PRISM_SNAPSHOT = Join-Path $out "$Case.png"
  foreach ($k in $vars.Keys) { Set-Item "env:$k" $vars[$k] }
  Remove-Item (Join-Path $out "$Case*.png"), (Join-Path $out "$Case.log") -ErrorAction SilentlyContinue
  $log = Join-Path $out "$Case.log"
  $cmd = $env:ComSpec
  Start-Process -FilePath $cmd -WorkingDirectory (Join-Path $repo 'examples\counter') -WindowStyle Hidden -PassThru `
    -ArgumentList ('/d /s /c ""{0}" > "{1}" 2>&1"' -f $exePath, $log)
}

function Get-Counter {
  Get-Process -Name counter -ErrorAction SilentlyContinue | Where-Object {
    try { $_.Path -eq $exePath } catch { $false }
  } | Select-Object -First 1
}

function Wait-CounterWindow([double]$seconds) {
  $until = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $until) {
    $c = Get-Counter
    if ($c) {
      $w = [OnScreen]::Windows() | Where-Object { $_.Pid -eq $c.Id } | Sort-Object { $_.W * $_.H } -Descending | Select-Object -First 1
      if ($w) { return $w }
    }
    Start-Sleep -Milliseconds 250
  }
  $null
}

# Copies only `rects` from the screen into a PNG of their bounding box; the
# rest is gray.
function Save-Masked([string]$path, $rects) {
  $x0 = ($rects | ForEach-Object { $_.X } | Measure-Object -Minimum).Minimum
  $y0 = ($rects | ForEach-Object { $_.Y } | Measure-Object -Minimum).Minimum
  $x1 = ($rects | ForEach-Object { $_.X + $_.W } | Measure-Object -Maximum).Maximum
  $y1 = ($rects | ForEach-Object { $_.Y + $_.H } | Measure-Object -Maximum).Maximum
  $bmp = New-Object Drawing.Bitmap ([int]($x1 - $x0)), ([int]($y1 - $y0))
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.Clear([Drawing.Color]::FromArgb(255, 128, 128, 128))
  foreach ($r in $rects) {
    $g.CopyFromScreen($r.X, $r.Y, $r.X - $x0, $r.Y - $y0, (New-Object Drawing.Size $r.W, $r.H))
  }
  $g.Dispose()
  $bmp.Save($path, [Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Say "captured $([IO.Path]::GetFileName($path)): $([int]($x1 - $x0))x$([int]($y1 - $y0)), only the app's window and the dialog"
}

# Closes a system dialog: with -Escape, or when UI Automation finds no
# Cancel or Close button, Escape while the dialog has the keyboard.
function Close-Dialog($win, [switch]$Escape) {
  $buttons = @()
  if (-not $Escape) {
    try {
      $root = [Windows.Automation.AutomationElement]::FromHandle([IntPtr]$win.Handle)
      $cond = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ControlTypeProperty), ([Windows.Automation.ControlType]::Button)
      $buttons = @($root.FindAll([Windows.Automation.TreeScope]::Descendants, $cond))
      Say ("dialog buttons: " + (($buttons | ForEach-Object { "'{0}' [{1}]" -f $_.Current.Name, $_.Current.AutomationId }) -join ', '))
    } catch {
      Say "UI Automation: $($_.Exception.Message)"
    }
  }
  $names = @('Cancel', 'Annuler', 'Close', 'Fermer')
  $hit = $buttons | Where-Object { $names -contains $_.Current.Name -or $_.Current.AutomationId -match 'Cancel|Close' } | Select-Object -First 1
  if ($hit) {
    Say "pressing '$($hit.Current.Name)' through UI Automation"
    $hit.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
  } else {
    [void][OnScreen]::SetForegroundWindow([IntPtr]$win.Handle)
    Start-Sleep -Milliseconds 300
    if ([OnScreen]::GetForegroundWindow().ToInt64() -eq $win.Handle) {
      Say 'Escape to the dialog'
      [OnScreen]::Escape()
    } else {
      Say 'the dialog does not have the keyboard: no Escape sent'
    }
  }
  $until = (Get-Date).AddSeconds(10)
  while ((Get-Date) -lt $until -and [OnScreen]::Exists($win.Handle)) { Start-Sleep -Milliseconds 200 }
  if ([OnScreen]::Exists($win.Handle)) {
    Say 'still open: Escape'
    [void][OnScreen]::SetForegroundWindow([IntPtr]$win.Handle)
    Start-Sleep -Milliseconds 300
    if ([OnScreen]::GetForegroundWindow().ToInt64() -eq $win.Handle) { [OnScreen]::Escape() }
    Start-Sleep -Seconds 2
  }
  $closed = -not [OnScreen]::Exists($win.Handle)
  Say "dialog closed: $closed"
  $closed
}

# Runs the counter with `vars`, waits for a window of another process to
# open over it (the dialog), captures both, closes it.
function Invoke-Dialog([hashtable]$vars, [switch]$Escape, [switch]$Person) {
  $before = @([OnScreen]::Windows() | ForEach-Object { $_.Handle })
  $p = Start-Counter $vars
  $app = Wait-CounterWindow 20
  if (-not $app) { Say 'FAIL: the counter showed no window'; return $false }
  Say ("app window: " + (Format-Win $app))
  $dialog = $null
  $until = (Get-Date).AddSeconds(25)
  while (-not $dialog -and (Get-Date) -lt $until) {
    Start-Sleep -Milliseconds 300
    # Not the taskbar's small popups: a dialog is at least 200 x 150.
    $new = @([OnScreen]::Windows() | Where-Object { $before -notcontains $_.Handle -and $_.Pid -ne $app.Pid -and $_.W -ge 200 -and $_.H -ge 150 })
    if ($new.Count) {
      Start-Sleep -Milliseconds 1500  # let it finish opening
      $new = @([OnScreen]::Windows() | Where-Object { $before -notcontains $_.Handle -and $_.Pid -ne $app.Pid -and $_.W -ge 200 -and $_.H -ge 150 })
      foreach ($w in $new) { Say ("new window: " + (Format-Win $w)) }
      $dialog = $new | Sort-Object { $_.W * $_.H } -Descending | Select-Object -First 1
    }
  }
  if (-not $dialog) {
    $fg = [OnScreen]::GetForegroundWindow().ToInt64()
    $fgw = [OnScreen]::Windows() | Where-Object { $_.Handle -eq $fg } | Select-Object -First 1
    Say ("FAIL: no dialog opened; the active window: " + $(if ($fgw) { Format-Win $fgw } else { 'none listed' }))
    $p.WaitForExit(60000) | Out-Null
    return $false
  }
  $now = Wait-CounterWindow 2
  if ($now) { $app = $now }
  Save-Masked (Join-Path $out "$Case-dialog.png") @($app, $dialog)
  if ($Person) {
    Say 'waiting up to 60 s for the person at the PC to press Cancel'
    $until = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $until -and [OnScreen]::Exists($dialog.Handle)) { Start-Sleep -Milliseconds 300 }
    $closed = -not [OnScreen]::Exists($dialog.Handle)
    Say "dialog closed: $closed"
  } else {
    $closed = Close-Dialog $dialog -Escape:$Escape
  }
  if (-not $p.WaitForExit(60000)) { Say 'the counter did not quit: stopping it'; Get-Counter | Stop-Process -Force }
  $closed
}

# --- The cases
switch ($Case) {
  { $_ -like 's41*' } {
    $page = Join-Path $out 's41.html'
    [IO.File]::WriteAllText($page, @"
<!doctype html><meta charset="utf-8">
<style>html,body{height:100%;margin:0}body{font:14px system-ui;background:#eef3fb}#s{padding:4px 8px}
#big{display:flex;align-items:center;justify-content:center;height:calc(100% - 28px);background:#4b4bd8;color:#fff;font-weight:600;text-decoration:none}</style>
<div id="s">S41 probe: window.open by script in 1.5 s</div>
<a id="big" href="https://example.com/?from=clicked-link" target="_blank">Click: example.com in a new window</a>
<script>
setTimeout(function () {
  var w = window.open("https://example.com/?from=script-popup", "_blank");
  var said = "script window.open returned " + (w ? "a window" : "null");
  document.getElementById("s").textContent = said;
  if (window.prismHost) prismHost.postMessage(JSON.stringify(said));
}, 1500);
</script>
"@, (New-Object Text.UTF8Encoding $false))
    $browser = Get-DefaultBrowser
    Say "default browser for https: $($browser.ProgId) ($($browser.Name))"
    if ($browser.Name -and $browserNames -notcontains $browser.Name) { $script:browserNames += $browser.Name }
    $t0 = Get-Date
    $procsBefore = @(Get-Browsers)
    $bpids = @($procsBefore | ForEach-Object { $_.Id })
    $winsBefore = @([OnScreen]::Windows())
    Say ("browser processes before: {0} ({1})" -f $procsBefore.Count, (($procsBefore | Group-Object Name | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', '))
    Say ("browser windows before: {0}" -f @($winsBefore | Where-Object { $bpids -contains $_.Pid }).Count)
    $vars = @{
      PRISM_PATH = '/components/media'; PRISM_WEB_URL = 'file:///' + $page.Replace('\', '/')
      PRISM_DELAY = '9'; PRISM_AFTER = '5'; PRISM_TRACE_INPUT = '1'; PRISM_TRACE = '1'
    }
    if ($Case -ne 's41-none') { $vars.PRISM_CLICK = "click:A web page@$At" }
    $profileDir = Join-Path $env:TEMP 'prism-onscreen-browser'
    $own = $null
    if ($Case -eq 's41-browser') {
      $own = @("$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
        "$env:ProgramFiles\BraveSoftware\Brave-Browser\Application\brave.exe") | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
      if (-not $own) { Say 'FAIL s41-browser: no Chrome or Brave'; exit 1 }
      $ownName = [IO.Path]::GetFileNameWithoutExtension($own)
      if (@(Get-Process -Name $ownName -ErrorAction SilentlyContinue).Count) { Say "s41-browser: $ownName is running (the owner's): not run"; exit 1 }
      Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue
      $vars.PRISM_BROWSER = "`"$own`" --user-data-dir=$profileDir --no-first-run --no-default-browser-check"
      Say "PRISM_BROWSER: $ownName with a throwaway profile"
    }
    # Every process and top-level window that appears while the app runs
    # and 10 s after (names only: titles are never read). The default
    # browser may be a stub that hands the link to a browser already
    # running, so short-lived processes count too.
    $known = @{}
    foreach ($q in Get-Process) { $known[$q.Id] = $true }
    $seen = @{}
    foreach ($w in $winsBefore) { $seen[$w.Handle] = $true }
    $newWins = @()
    $newProcs = @()
    $mine = @('counter', 'msedgewebview2', 'cmd', 'conhost', 'powershell', 'OpenConsole', 'WindowsTerminal')
    $p = Start-Counter $vars
    $end = $null
    while (((Get-Date) - $t0).TotalSeconds -lt 75) {
      foreach ($q in Get-Process) {
        if (-not $known.ContainsKey($q.Id)) {
          $known[$q.Id] = $true
          if ($mine -notcontains $q.ProcessName) { $newProcs += $q.ProcessName }
        }
      }
      foreach ($w in [OnScreen]::Windows()) {
        if (-not $seen.ContainsKey($w.Handle)) { $seen[$w.Handle] = $true; $w.Name = Get-ProcName $w.Pid; $newWins += $w }
      }
      if ($p.HasExited -and -not $end) { $end = Get-Date }
      if ($end -and ((Get-Date) - $end).TotalSeconds -gt 10) { break }
      Start-Sleep -Milliseconds 150
    }
    if (-not $p.HasExited) { Get-Counter | Stop-Process -Force; Say 'FAIL: the counter did not quit'; $failed++ }
    $procsAfter = @(Get-Browsers)
    $started = @($procsAfter | Where-Object { $_.Start -ge $t0 })
    Say ("browser processes after: {0}; started during the run: {1} ({2})" -f $procsAfter.Count, $started.Count,
      (($started | Group-Object Name | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', '))
    Say ("other processes started during the run: " + (($newProcs | Group-Object | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', '))
    foreach ($w in $newWins) {
      if ($mine -contains $w.Name) { continue }
      Say ("new window during the run: " + (Format-Win $w))
    }
    $foreign = @($newWins | Where-Object { $mine -notcontains $_.Name -and $_.W -ge 200 -and $_.H -ge 150 })
    $example = @([OnScreen]::Windows() | Where-Object { [OnScreen]::TitleStarts($_.Handle, 'Example Domain') }).Count
    Say ("windows showing Example Domain now: {0}" -f $example)
    $log = Join-Path $out "$Case.log"
    $logText = if (Test-Path $log) { Get-Content -Raw -Encoding UTF8 $log } else { '' }
    if ($Case -eq 's41-none') {
      if ($started.Count -eq 0 -and $example -eq 0 -and $newProcs.Count -eq 0 -and $foreign.Count -eq 0) { Say 'ok   s41-none: no browser process or window opened, no other process or window' } else { Say 'FAIL s41-none: something opened'; $failed++ }
      if ($logText -match 'a new window for https://example.com/\?from=script-popup not opened: no user gesture') {
        Say 'ok   s41-none: the app logged the refusal (no user gesture)'
      } else { Say 'FAIL s41-none: no refusal in the log'; $failed++ }
    } else {
      Remove-Item (Join-Path $out "$Case-screen.png") -ErrorAction SilentlyContinue  # may show the browser
      if ($logText -match 'a new window for https://example.com/\?from=clicked-link opens in the browser \(user gesture\)') {
        Say "ok   ${Case}: the app handed the clicked link to the browser"
      } else { Say "FAIL ${Case}: no hand-off in the log"; $failed++ }
      if ($started.Count -gt 0 -or $example -gt 0) {
        Say "ok   ${Case}: a browser process or window opened"
      } elseif ($Case -eq 's41-tap') {
        Say 's41-tap: no browser opened: the https handler is the one above (see the processes it started)'
      } else { Say "FAIL ${Case}: no browser process or window"; $failed++ }
      if ($own) {
        Get-Process -Name $ownName -ErrorAction SilentlyContinue | Where-Object { $_.StartTime -ge $t0 } | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
        Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue
        Say ("{0} closed: {1} left; profile deleted: {2}" -f $ownName, @(Get-Process -Name $ownName -ErrorAction SilentlyContinue).Count, -not (Test-Path $profileDir))
      }
      if ($logText -match 'script-popup opens in the browser') { Say "FAIL ${Case}: the script window reached the browser"; $failed++ }
    }
  }
  'share' {
    $ok = Invoke-Dialog @{ PRISM_PATH = '/components'; PRISM_CLICK = 'Share'; PRISM_DELAY = '3'; PRISM_AFTER = '20'; PRISM_TRACE = '1' } -Escape
    if (-not $ok) { $failed++ }
  }
  'passkey' {
    $ok = Invoke-Dialog @{ PRISM_PATH = '/settings'; PRISM_CLICK = 'Create a passkey'; PRISM_DELAY = '3'; PRISM_AFTER = '75' } -Person
    if (-not $ok) { $failed++ }
    $log = Join-Path $out "$Case.log"
    if ((Test-Path $log) -and (Select-String -LiteralPath $log -Pattern 'passkeys: Cancelled' -Quiet)) {
      Say 'ok   passkey: the app got Cancelled'
    } else { Say 'FAIL passkey: no Cancelled in the log'; $failed++ }
  }
}
if (Get-Counter) { Get-Counter | Stop-Process -Force; Say 'stopped a counter left running' }
Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
if ($failed) { Say "onscreen: FAIL ($failed)"; exit 1 }
Say 'onscreen: OK'
