<#
.SYNOPSIS
Starts a Prism app, waits for its first frame and its PRISM_CLICK steps,
dumps its UI Automation tree as JSON (uia-dump.ps1 -Json) and stops it.

.DESCRIPTION
Runs in the desktop session (through run-in-session.ps1 from SSH): UI
Automation sees a WinUI window only there. The app gets fresh storage
(LOCALAPPDATA in %TEMP%), PRISM_PATH, PRISM_CLICK and the variables of -Env;
its log goes next to the JSON (<out>.log). Text that cmd.exe would mangle
(the click labels, "Delete…") comes base64-encoded (UTF-8).

Exit codes: 0 dumped, 1 the app never showed its first frame or the dump
failed, 2 bad arguments.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\a11y-dump.ps1 -Exe target\debug\counter.exe -WorkingDirectory examples\counter -Path /components -Out target\windows-a11y\components.json
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out,
    [string] $WorkingDirectory = (Get-Location).ProviderPath,
    [string] $Path = '/',
    # PRISM_CLICK, base64 of its UTF-8 text.
    [string] $ClickBase64 = '',
    # More variables, base64 of UTF-8 "NAME=value" lines.
    [string] $EnvBase64 = '',
    # Seconds to wait after the first frame (the clicks run 1 s after it).
    [double] $Settle = 3,
    [double] $TimeoutSeconds = 90
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

function Decode([string] $b64) {
    if (-not $b64) { return '' }
    [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64))
}

$Exe = (Resolve-Path -LiteralPath $Exe).ProviderPath
$WorkingDirectory = (Resolve-Path -LiteralPath $WorkingDirectory).ProviderPath
$Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
$log = [IO.Path]::ChangeExtension($Out, '.log')
$err = [IO.Path]::ChangeExtension($Out, '.err')
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
Remove-Item -LiteralPath $Out, $log, $err -ErrorAction SilentlyContinue

# The app inherits this process's environment.
$name = [IO.Path]::GetFileNameWithoutExtension($Out)
$env:LOCALAPPDATA = Join-Path $env:TEMP "prism-a11y-$name"
if (Test-Path -LiteralPath $env:LOCALAPPDATA) { Remove-Item -Recurse -Force -LiteralPath $env:LOCALAPPDATA }
$env:PRISM_PATH = $Path
$env:PRISM_CLICK = Decode $ClickBase64
foreach ($line in ((Decode $EnvBase64) -split "`r?`n")) {
    if ($line -match '^([A-Za-z_][A-Za-z0-9_]*)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}

$app = Start-Process -FilePath $Exe -WorkingDirectory $WorkingDirectory -PassThru `
    -RedirectStandardOutput $log -RedirectStandardError $err
try {
    # prism-windows logs its first frame.
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $shown = $false
    while (-not $shown -and -not $app.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 250
        $shown = (Test-Path -LiteralPath $err) -and ((Get-Content -LiteralPath $err -Raw -ErrorAction SilentlyContinue) -match 'first frame')
    }
    if (-not $shown) {
        [Console]::Error.WriteLine("a11y-dump: no first frame from $Exe (see $err)")
        exit 1
    }
    Start-Sleep -Milliseconds ([int]($Settle * 1000))
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'uia-dump.ps1') -Id $app.Id -Json $Out | Out-Null
    if (-not (Test-Path -LiteralPath $Out)) {
        [Console]::Error.WriteLine("a11y-dump: uia-dump wrote nothing for $Exe")
        exit 1
    }
    exit 0
} finally {
    if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
}
