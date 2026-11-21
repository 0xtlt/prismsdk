# Security review S27 and S41 on Windows, in the signed-in desktop session
# (WinUI renders nowhere else), with the counter's debug build
# (`cargo build -p counter`). Each run writes target\security-gui\<run>.png
# (the XAML), <run>-window.png (what DWM shows) and <run>.log.
#
# S27 (Windows Hello): « Verify it's you » on /settings, with PRISM_AUTH set
# (the button then asks even where the app's capabilities say no): the
# default request (PRISM_AUTH=any), and biometrics only
# (PRISM_AUTH=biometrics, `AuthRequest::biometrics_only`). Hello's consent
# check cannot promise biometrics, so biometrics only is « unavailable »
# without asking Hello; with no Hello device or PIN, the default request is
# « unavailable » after Hello's availability check, with no prompt.
#
# S41 (WebView2's user gesture): a local page in the web view of
# /components/media opens a web page by script 1.5 s after it loads (no
# tap), and has a link to another web page in a new window. PRISM_BROWSER
# is a logger instead of a browser: the script's window must never reach
# it, the clicked link must. -At is where the link is, in DIPs from the
# « A web page » heading (see the layout run's snapshot).
#
# S33 (browser sign-in): « Sign in with Prism ID » on /settings against
# scripts/mock-oidc.py at http://127.0.0.1:8765 (on the PC, or forwarded to
# it: ssh -R 8765:127.0.0.1:8765). PRISM_BROWSER is s33-browser.ps1: it
# sends the app's loopback listener callbacks that are not the sign-in's,
# then approves. The app must end signed in. The sign-in keeps its client
# in Credential Manager: the Prism/ entries the run adds are deleted after.
#
# From the repository on the PC (a param block: run with -File, not
# windows-remote.sh's ps):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\security-gui.ps1 -Case s27
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\security-gui.ps1 -Case s41 -At 160,150
#
# Exit 125 when nobody is signed in (no desktop session), 1 when a check
# fails.
param(
  [ValidateSet("s27", "s41-layout", "s41", "s33")] [string]$Case = "s27",
  [string]$Exe = "target\debug\counter.exe",
  [string]$At = "160,150"
)
$ProgressPreference = 'SilentlyContinue'
$repo = (Get-Location).ProviderPath
$out = Join-Path $repo "target\security-gui"
New-Item -ItemType Directory -Force $out | Out-Null
# The strings' apostrophe (U+2019): this file stays ASCII.
$q = [char]0x2019
$failed = 0

# Runs the app with `vars` in the desktop session until its snapshot. A
# PowerShell launcher sets them (cmd would garble the apostrophe), then cmd
# starts the app (GUI apps start from cmd: `start /wait /b`).
function Invoke-App([string]$name, [hashtable]$vars) {
  $launcher = Join-Path $out "$name.ps1"
  $vars = $vars.Clone()
  $vars.LOCALAPPDATA = Join-Path $env:TEMP "prism-sec-$name"
  $vars.PRISM_SNAPSHOT = Join-Path $out "$name.png"
  $lines = @("Remove-Item -Recurse -Force '$($vars.LOCALAPPDATA)' -ErrorAction SilentlyContinue")
  foreach ($k in $vars.Keys) {
    $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($vars[$k]))
    $lines += "`$env:$k = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$b64'))"
  }
  $lines += @(
    "Set-Location '$(Join-Path $repo "examples\counter")'",
    "& `$env:ComSpec /d /c 'start `"`" /wait /b ..\..\$Exe > `"$(Join-Path $out "$name.log")`" 2>&1'",
    "exit `$LASTEXITCODE"
  )
  [IO.File]::WriteAllText($launcher, ($lines -join "`r`n") + "`r`n", [Text.Encoding]::ASCII)
  Remove-Item (Join-Path $out "$name*.png"), (Join-Path $out "$name.log") -ErrorAction SilentlyContinue
  & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo "scripts\windows\run-in-session.ps1") -TimeoutSeconds 120 -Command "powershell -NoProfile -ExecutionPolicy Bypass -File `"$launcher`""
  $code = $LASTEXITCODE
  "${name}: exit $code"
  if ($code -eq 125) { "security-gui: nobody is signed in on the PC: no desktop session"; exit 125 }
  $log = Join-Path $out "$name.log"
  if (Test-Path $log) { Get-Content -Encoding UTF8 $log | ForEach-Object { "  | $_" } }
  if (-not (Test-Path (Join-Path $out "$name.png"))) { "security-gui: FAIL: $name wrote no snapshot"; $script:failed++ }
}

function Test-Log([string]$name, [string]$pattern, [string]$what) {
  $log = Join-Path $out "$name.log"
  $hit = (Test-Path $log) -and (Select-String -Path $log -Pattern $pattern -Quiet)
  if ($hit) { "ok   ${name}: $what" } else { "FAIL ${name}: $what"; $script:failed++ }
}

switch ($Case) {
  "s27" {
    # What Hello says here, read-only (no prompt).
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
      $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
    $null = [Windows.Security.Credentials.UI.UserConsentVerifier, Windows.Security.Credentials.UI, ContentType = WindowsRuntime]
    $task = $asTask.MakeGenericMethod([Windows.Security.Credentials.UI.UserConsentVerifierAvailability]).Invoke($null, @([Windows.Security.Credentials.UI.UserConsentVerifier]::CheckAvailabilityAsync()))
    $null = $task.Wait(15000)
    "Windows Hello: $($task.Result)"
    $verify = "Verify it$($q)s you"
    $common = @{ PRISM_PATH = "/settings"; PRISM_CLICK = $verify; PRISM_AUTH = "any"; PRISM_DELAY = "2"; PRISM_AFTER = "3"; PRISM_LOCALE = "en" }
    Invoke-App "s27-default" $common
    Test-Log "s27-default" "authenticate: (no Windows Hello device|Windows Hello is not set up)" "Hello unavailable, no prompt"
    $bio = $common.Clone(); $bio.PRISM_AUTH = "biometrics"
    Invoke-App "s27-biometrics" $bio
    $log = Join-Path $out "s27-biometrics.log"
    if ((Test-Path $log) -and -not (Select-String -Path $log -Pattern "authenticate:" -Quiet)) {
      "ok   s27-biometrics: refused before Hello is asked (no availability check, no prompt)"
    } else { "FAIL s27-biometrics: Hello was asked"; $failed++ }
  }
  { $_ -like "s41*" } {
    $page = Join-Path $out "s41.html"
    [IO.File]::WriteAllText($page, @"
<!doctype html><meta charset="utf-8">
<style>body{font:14px system-ui;margin:6px;background:#eef3fb}#big{display:block;padding:22px 9px;background:#4b4bd8;color:#fff;font-weight:600}</style>
<div id="s">S41 probe: a window opens by script in 1.5 s</div>
<a id="big" href="https://example.com/?from=tapped-link" target="_blank">Tap: https link in a new window</a>
<script>
if (window.prismHost) prismHost.postMessage(JSON.stringify("probe loaded: a window opens by script in 1.5 s"));
setTimeout(function () {
  var w = window.open("https://example.com/?from=script-popup", "_blank");
  var said = "script window.open returned " + (w ? "a window" : "null");
  document.getElementById("s").textContent = said;
  // Shown by the app under the web view (and in its XAML snapshot).
  if (window.prismHost) prismHost.postMessage(JSON.stringify(said));
}, 1500);
</script>
"@, (New-Object Text.UTF8Encoding $false))
    $logger = Join-Path $out "browser.cmd"
    $opened = Join-Path $out "browser.log"
    [IO.File]::WriteAllText($logger, "@echo %~1>> `"%~dp0browser.log`"`r`n", [Text.Encoding]::ASCII)
    Remove-Item $opened -ErrorAction SilentlyContinue
    $vars = @{
      PRISM_PATH = "/components/media"; PRISM_LOCALE = "en"; PRISM_BROWSER = $logger
      PRISM_WEB_URL = "file:///" + $page.Replace('\', '/'); PRISM_DELAY = "9"; PRISM_AFTER = "4"
    }
    if ($Case -eq "s41-layout") {
      Invoke-App "s41-layout" $vars
    } else {
      # No tap: the script's window stays in the page.
      Invoke-App "s41-no-tap" $vars
      $none = -not (Test-Path $opened)
      if ($none) { "ok   s41-no-tap: nothing opened" } else { "FAIL s41-no-tap: opened $(Get-Content $opened)"; $failed++ }
      # A real click on the link (real mouse input: the session must be on screen).
      $tap = $vars.Clone(); $tap.PRISM_CLICK = "click:A web page@$At"; $tap.PRISM_TRACE_INPUT = "1"
      Invoke-App "s41-tap" $tap
      Start-Sleep -Seconds 2
      $got = if (Test-Path $opened) { @(Get-Content $opened) } else { @() }
      "browser.log: $($got -join ' ; ')"
      if ($got -contains "https://example.com/?from=tapped-link" -and -not ($got -match "script-popup")) {
        "ok   s41-tap: the tapped link opened, the script's window did not"
      } else { "FAIL s41-tap: expected only the tapped link"; $failed++ }
    }
  }
}
# The Prism/ entries of Credential Manager, in the desktop session (an SSH
# key logon has no credential store).
function Get-PrismCredentials {
  $list = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo "scripts\windows\run-in-session.ps1") -TimeoutSeconds 60 -Command "cmdkey /list"
  @($list | Select-String -Pattern 'target=(Prism/\S+)' -AllMatches | ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value })
}

if ($Case -eq "s33") {
  $before = Get-PrismCredentials
  $browserLog = Join-Path $out "s33-browser.log"
  Remove-Item $browserLog -ErrorAction SilentlyContinue
  $vars = @{
    PRISM_PATH = "/settings"; PRISM_LOCALE = "en"; PRISM_CLICK = "Sign in with Prism ID"
    PRISM_DELAY = "2"; PRISM_AFTER = "20"; S33_LOG = $browserLog
    PRISM_BROWSER = "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File " + (Join-Path $repo "scripts\windows\s33-browser.ps1")
  }
  Invoke-App "s33-sign-in" $vars
  "fake browser:"
  if (Test-Path $browserLog) { Get-Content -Encoding UTF8 $browserLog | ForEach-Object { "  > $_" } }
  if ((Test-Path $browserLog) -and (Select-String -Path $browserLog -Pattern "the real callback -> 200" -Quiet)) {
    "ok   s33: the real callback was still awaited after the others"
  } else { "FAIL s33: the real callback did not reach the app"; $failed++ }
  $added = @(Get-PrismCredentials | Where-Object { $before -notcontains $_ })
  foreach ($t in $added) {
    $null = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo "scripts\windows\run-in-session.ps1") -TimeoutSeconds 60 -Command "cmdkey /delete:$t"
  }
  $left = @(Get-PrismCredentials | Where-Object { $before -notcontains $_ })
  "Credential Manager: $($added.Count) Prism/ entries added by the run, $($left.Count) left after cleanup"
}
if ($failed) { "security-gui: FAIL: $failed"; exit 1 }
"security-gui: OK"
