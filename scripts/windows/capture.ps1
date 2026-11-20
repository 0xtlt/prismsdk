<#
.SYNOPSIS
Saves a PNG of a process's window, or of the whole screen.

.DESCRIPTION
Windows PowerShell 5.1, no modules. With -ProcessName or -Id it draws the
process's first top-level window (console windows left out) with
PrintWindow, which works when other windows cover it but not when it is
minimized. Without them it copies the whole screen.

Either way it needs the user's signed-in session, on screen and unlocked:
from SSH, run it there through scripts\windows\run-in-session.ps1. The
session-0 desktop of an SSH logon is never drawn, and a locked or
disconnected session gives black or lock-screen pictures.

It prints the PNG's path and size. Exit codes: 0 done, 1 no window, 2 bad
arguments, 3 nothing to capture (hidden desktop, minimized window, or every
pixel the same colour: locked session, or nothing drawn yet).

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\capture.ps1 -ProcessName counter -Out counter.png

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\capture.ps1 -Out screen.png
#>
[CmdletBinding(DefaultParameterSetName = 'Screen')]
param(
    # Where to write the PNG.
    [Parameter(Mandatory)]
    [string] $Out,
    # Process name without ".exe" (every process of that name counts).
    [Parameter(Mandatory, ParameterSetName = 'ByName')]
    [string] $ProcessName,
    # Process id.
    [Parameter(Mandatory, ParameterSetName = 'ById')]
    [int] $Id,
    # How long to wait for the process's window.
    [double] $TimeoutSeconds = 10
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs')
$Native = [PrismWindows.Native]
$Native::UsePhysicalPixels()

function Fail([int] $code, [string] $message) {
    [Console]::Error.WriteLine("capture: $message")
    exit $code
}

if (-not $Native::DesktopShown()) {
    Fail 3 ('this desktop is never shown (session 0, as from SSH): nothing to capture. ' +
        'Run the app and this script in the user''s session: scripts\windows\run-in-session.ps1')
}

function Get-Window {
    [int[]] $ids = @(if ($PSCmdlet.ParameterSetName -eq 'ById') {
            $Id
        } else {
            Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | ForEach-Object { $_.Id }
        })
    if ($ids.Count -eq 0) { return [IntPtr]::Zero }
    $windows = @($Native::ShownWindows($ids))
    if ($windows.Count -eq 0) { return [IntPtr]::Zero }
    $windows[0]
}

$path = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)

if ($PSCmdlet.ParameterSetName -eq 'Screen') {
    $area = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bitmap = New-Object System.Drawing.Bitmap $area.Width, $area.Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($area.Location, [System.Drawing.Point]::Empty, $area.Size)
    } catch {
        # A locked or disconnected session ("The handle is invalid").
        Fail 3 "cannot read the screen: $($_.Exception.Message)"
    } finally {
        $graphics.Dispose()
    }
} else {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $hwnd = Get-Window
    while ($hwnd -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 250
        $hwnd = Get-Window
    }
    if ($hwnd -eq [IntPtr]::Zero) { Fail 1 "no window after $TimeoutSeconds s" }
    if ($Native::IsMinimized($hwnd)) { Fail 3 'the window is minimized' }
    $bounds = $Native::Bounds($hwnd)
    if ($bounds[2] -le 0 -or $bounds[3] -le 0) { Fail 3 'the window has no size' }
    $bitmap = New-Object System.Drawing.Bitmap $bounds[2], $bounds[3]
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    $drawn = $Native::Draw($hwnd, $hdc)
    $graphics.ReleaseHdc($hdc)
    $graphics.Dispose()
    if (-not $drawn) { Fail 3 'PrintWindow failed' }
}

$bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
'{0} {1}x{2}' -f $path, $bitmap.Width, $bitmap.Height

# One colour everywhere (sampled on a 32 x 32 grid): nothing was drawn.
$first = $bitmap.GetPixel(0, 0).ToArgb()
$uniform = $true
for ($i = 0; $i -lt 32 -and $uniform; $i++) {
    for ($j = 0; $j -lt 32; $j++) {
        $x = [int](($bitmap.Width - 1) * $i / 31)
        $y = [int](($bitmap.Height - 1) * $j / 31)
        if ($bitmap.GetPixel($x, $y).ToArgb() -ne $first) { $uniform = $false; break }
    }
}
$bitmap.Dispose()
if ($uniform) { Fail 3 'every pixel has the same colour (locked session, or nothing drawn yet)' }
exit 0
