<#
.SYNOPSIS
Dumps the UI Automation tree of a process's top-level windows, or invokes one
of their elements by name.

.DESCRIPTION
Windows PowerShell 5.1 (part of Windows), no modules to install. It sees the
windows of the desktop it runs on, so it runs in the same session as the
app: the user's own desktop (through scripts\windows\run-in-session.ps1 from
SSH), or the hidden session-0 desktop of an SSH logon, for apps started by
the same SSH command. On that hidden desktop, apps that provide UI
Automation themselves (WPF, for example) show their whole tree, but controls
that are child windows (classic Win32, WinForms) do not show at all.

Each element prints as one line, indented by depth:

    Button "+1" #plus .Button [60,122 344x31]

that is the control type, "name", #automation id, .class name and bounds
(physical screen pixels: x,y width x height). -Json writes the same tree as
JSON (name, type, automationId, className, bounds [x, y, w, h], children).

-Invoke finds the first element whose name is exactly the given text and
presses it: Invoke (buttons, links, menu items), else Toggle (check boxes),
else Select (tabs, list items), else Expand/Collapse. It prints "nothing
labelled <name>" and exits 1 when no element has that name, like the
PRISM_CLICK hook of the backends.

Exit codes: 0 done, 1 no window, no element or an element that cannot be
pressed, 2 bad arguments.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\uia-dump.ps1 -ProcessName charmap

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\uia-dump.ps1 -Id 4242 -Json tree.json

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\uia-dump.ps1 -ProcessName counter -Invoke "+1"
#>
[CmdletBinding(DefaultParameterSetName = 'ByName')]
param(
    # Process name without ".exe" (every process of that name counts).
    [Parameter(Mandatory, ParameterSetName = 'ByName', Position = 0)]
    [string] $ProcessName,
    # Process id.
    [Parameter(Mandatory, ParameterSetName = 'ById')]
    [int] $Id,
    # Name of an element to press instead of printing the tree.
    [string] $Invoke,
    # Also write the tree as JSON to this file.
    [string] $Json,
    # How deep to walk below each window.
    [int] $Depth = 40,
    # How long to wait for the process and its window.
    [double] $TimeoutSeconds = 10
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs')
$Native = [PrismWindows.Native]
$Native::UsePhysicalPixels()

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$Automation = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]
$Walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker

function Get-TargetIds {
    if ($PSCmdlet.ParameterSetName -eq 'ById') { return @($Id) }
    @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
}

# Top-level windows of the target processes on this desktop.
function Get-TargetWindows {
    [int[]] $ids = @(Get-TargetIds)
    if ($ids.Count -eq 0) { return @() }
    # A window may close in between: skip it.
    @($Native::ShownWindows($ids) | ForEach-Object { try { $Automation::FromHandle($_) } catch { } })
}

function ConvertTo-Node($element, [int] $level) {
    $c = $element.Current
    $r = $c.BoundingRectangle
    $bounds = $null
    if (-not $r.IsEmpty -and -not [double]::IsInfinity($r.Width)) {
        $bounds = @([int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height)
    }
    $children = New-Object System.Collections.ArrayList
    if ($level -lt $Depth) {
        $child = $Walker.GetFirstChild($element)
        while ($null -ne $child) {
            try { [void]$children.Add((ConvertTo-Node $child ($level + 1))) }
            catch [System.Windows.Automation.ElementNotAvailableException] { }
            $child = $Walker.GetNextSibling($child)
        }
    }
    [ordered]@{
        name         = $c.Name
        type         = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''
        automationId = $c.AutomationId
        className    = $c.ClassName
        bounds       = $bounds
        children     = $children.ToArray()
    }
}

function Write-Node($node, [string] $indent) {
    $line = $indent + $node.type
    if ($node.name) { $line += ' "' + ($node.name -replace "`r?`n", '\n') + '"' }
    if ($node.automationId) { $line += ' #' + $node.automationId }
    if ($node.className) { $line += ' .' + $node.className }
    if ($null -ne $node.bounds) { $line += ' [{0},{1} {2}x{3}]' -f $node.bounds }
    $line
    foreach ($child in $node.children) { Write-Node $child ($indent + '  ') }
}

function Invoke-Element($element) {
    $pattern = $null
    if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        $pattern.Invoke(); return 'invoked'
    }
    if ($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        $pattern.Toggle(); return 'toggled to ' + $pattern.Current.ToggleState
    }
    if ($element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
        $pattern.Select(); return 'selected'
    }
    if ($element.TryGetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$pattern)) {
        if ($pattern.Current.ExpandCollapseState -eq 'Collapsed') { $pattern.Expand(); return 'expanded' }
        $pattern.Collapse(); return 'collapsed'
    }
    $null
}

if ($Depth -lt 0 -or $TimeoutSeconds -lt 0) {
    [Console]::Error.WriteLine('uia-dump: -Depth and -TimeoutSeconds cannot be negative')
    exit 2
}

$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
$windows = @(Get-TargetWindows)
while ($windows.Count -eq 0 -and [DateTime]::UtcNow -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $windows = @(Get-TargetWindows)
}
if ($windows.Count -eq 0) {
    $target = if ($PSCmdlet.ParameterSetName -eq 'ById') { "process $Id" } else { "process '$ProcessName'" }
    [Console]::Error.WriteLine("uia-dump: no window for $target on this desktop after $TimeoutSeconds s")
    if (-not $Native::DesktopShown()) {
        [Console]::Error.WriteLine('uia-dump: this is a hidden session-0 desktop (SSH): only apps started from this session show here')
    }
    exit 1
}

if ($Invoke) {
    $condition = New-Object System.Windows.Automation.PropertyCondition($Automation::NameProperty, $Invoke)
    foreach ($window in $windows) {
        $element = if ($window.Current.Name -eq $Invoke) { $window } else { $window.FindFirst($Scope::Descendants, $condition) }
        if ($null -ne $element) {
            $type = $element.Current.ControlType.ProgrammaticName -replace '^ControlType\.', ''
            $what = Invoke-Element $element
            if ($null -eq $what) {
                [Console]::Error.WriteLine("uia-dump: $type ""$Invoke"" supports none of Invoke, Toggle, SelectionItem, ExpandCollapse")
                exit 1
            }
            '{0} "{1}": {2}' -f $type, $Invoke, $what
            exit 0
        }
    }
    [Console]::Error.WriteLine("uia-dump: nothing labelled $Invoke")
    exit 1
}

if (-not $Native::DesktopShown()) {
    [Console]::Error.WriteLine('uia-dump: hidden session-0 desktop (SSH): controls that are child windows (Win32, WinForms) do not show')
}
$nodes = @($windows | ForEach-Object { ConvertTo-Node $_ 0 })
foreach ($node in $nodes) { Write-Node $node '' }
if ($Json) {
    $path = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Json)
    $text = ConvertTo-Json -InputObject $nodes -Depth 100
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding $false))
}
exit 0
