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
    CheckBox "Wi-Fi" .Button [60,160 344x40] {toggle=On}

that is the control type, "name", #automation id, .class name, bounds
(physical screen pixels: x,y width x height) and, in braces, what a screen
reader reads besides: its localized type when it is not the control type's
own, states (toggle, selected, expanded, disabled), value and range, heading
level, live region, dialog, landmark, help text, position in its set.
-Json writes the same tree as JSON (name, type, automationId, className,
bounds [x, y, w, h], localizedType, the states as keys: toggle, selected,
expanded, value, range [min, max, value], disabled, focusable, heading,
live, dialog, landmark, help, position [n, of], patterns, children), which
scripts/windows/a11y-check.py checks.

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
$Ids = [System.Windows.Automation.AutomationElementIdentifiers]
$NotSupported = [System.Windows.Automation.AutomationElement]::NotSupported

# What a screen reader reads of each element, fetched in one call per
# element (a cache request) rather than one per property.
$Props = [ordered]@{
    localizedType = $Ids::LocalizedControlTypeProperty
    enabled       = $Ids::IsEnabledProperty
    focusable     = $Ids::IsKeyboardFocusableProperty
    help          = $Ids::HelpTextProperty
    heading       = $Ids::HeadingLevelProperty
    dialog        = $Ids::IsDialogProperty
    live          = $Ids::LiveSettingProperty
    positionInSet = $Ids::PositionInSetProperty
    sizeOfSet     = $Ids::SizeOfSetProperty
    toggle        = [System.Windows.Automation.TogglePattern]::ToggleStateProperty
    selected      = [System.Windows.Automation.SelectionItemPattern]::IsSelectedProperty
    expanded      = [System.Windows.Automation.ExpandCollapsePattern]::ExpandCollapseStateProperty
    value         = [System.Windows.Automation.ValuePattern]::ValueProperty
    rangeMin      = [System.Windows.Automation.RangeValuePattern]::MinimumProperty
    rangeMax      = [System.Windows.Automation.RangeValuePattern]::MaximumProperty
    rangeValue    = [System.Windows.Automation.RangeValuePattern]::ValueProperty
    invoke        = $Ids::IsInvokePatternAvailableProperty
}
# The landmark (UIA_LandmarkTypePropertyId) has no managed identifier in
# .NET Framework: registered by id when the runtime allows it.
$Landmark = $null
try {
    $register = [System.Windows.Automation.AutomationProperty].Assembly.GetType('System.Windows.Automation.AutomationIdentifier').GetMethod('Register', [Reflection.BindingFlags]'NonPublic, Static')
    if ($null -ne $register) {
        $kind = $register.GetParameters()[0].ParameterType
        $Landmark = $register.Invoke($null, @([Enum]::ToObject($kind, 0), 30157, 'LandmarkTypeProperty'))
    }
} catch { $Landmark = $null }
if ($null -ne $Landmark) { $Props['landmark'] = $Landmark }

$Cache = New-Object System.Windows.Automation.CacheRequest
foreach ($p in @($Ids::NameProperty, $Ids::ControlTypeProperty, $Ids::AutomationIdProperty, $Ids::ClassNameProperty, $Ids::BoundingRectangleProperty)) { $Cache.Add($p) }
foreach ($p in $Props.Values) { $Cache.Add($p) }
$Cache.AutomationElementMode = [System.Windows.Automation.AutomationElementMode]::Full

function Get-Cached($element, $property) {
    try {
        $v = $element.GetCachedPropertyValue($property, $true)
        if ([object]::ReferenceEquals($v, $NotSupported)) { return $null }
        $v
    } catch { $null }
}

# The states worth reading, by name; nothing for defaults.
function Get-States($element) {
    $s = [ordered]@{}
    $type = $element.Cached.ControlType
    $lt = Get-Cached $element $Props.localizedType
    if ($lt -and ($null -eq $type -or $lt -ne $type.LocalizedControlType)) { $s.localizedType = $lt }
    if ((Get-Cached $element $Props.enabled) -eq $false) { $s.disabled = $true }
    if ((Get-Cached $element $Props.focusable) -eq $true) { $s.focusable = $true }
    $help = Get-Cached $element $Props.help
    if ($help) { $s.help = $help }
    # 1 to 9 (.NET's AutomationHeadingLevel, or UI Automation's ids from 80050).
    $h = Get-Cached $element $Props.heading
    if ($null -ne $h) {
        $level = [int]$h
        if ($level -ge 80050) { $level -= 80050 }
        if ($level -gt 0) { $s.heading = $level }
    }
    if ((Get-Cached $element $Props.dialog) -eq $true) { $s.dialog = $true }
    $live = Get-Cached $element $Props.live
    if ($null -ne $live -and [int]$live -gt 0) { $s.live = @('off', 'polite', 'assertive')[[int]$live] }
    if ($Props.Contains('landmark')) {
        $l = Get-Cached $element $Props.landmark
        if ($null -ne $l -and [int]$l -ge 80000) { $s.landmark = @('custom', 'form', 'main', 'navigation', 'search')[[int]$l - 80000] }
    }
    $t = Get-Cached $element $Props.toggle
    if ($null -ne $t) { $s.toggle = "$t" }
    $sel = Get-Cached $element $Props.selected
    if ($null -ne $sel) { $s.selected = [bool]$sel }
    $e = Get-Cached $element $Props.expanded
    if ($null -ne $e -and "$e" -ne 'LeafNode') { $s.expanded = "$e" }
    $v = Get-Cached $element $Props.value
    if ($null -ne $v -and "$v" -ne '') { $s.value = "$v" }
    $rv = Get-Cached $element $Props.rangeValue
    if ($null -ne $rv) { $s.range = @([double](Get-Cached $element $Props.rangeMin), [double](Get-Cached $element $Props.rangeMax), [double]$rv) }
    $pos = Get-Cached $element $Props.positionInSet
    $of = Get-Cached $element $Props.sizeOfSet
    if ($null -ne $pos -and [int]$pos -gt 0) { $s.position = @([int]$pos, [int]$of) }
    if ((Get-Cached $element $Props.invoke) -eq $true) { $s.invoke = $true }
    $s
}

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
    $c = $element.Cached
    $r = $c.BoundingRectangle
    $bounds = $null
    if (-not $r.IsEmpty -and -not [double]::IsInfinity($r.Width)) {
        $bounds = @([int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height)
    }
    $children = New-Object System.Collections.ArrayList
    if ($level -lt $Depth) {
        $child = $Walker.GetFirstChild($element, $Cache)
        while ($null -ne $child) {
            try { [void]$children.Add((ConvertTo-Node $child ($level + 1))) }
            catch [System.Windows.Automation.ElementNotAvailableException] { }
            $child = $Walker.GetNextSibling($child, $Cache)
        }
    }
    # Windows of other frameworks inside (WebView2's) may have none.
    $type = 'Unknown'
    if ($null -ne $c.ControlType) { $type = $c.ControlType.ProgrammaticName -replace '^ControlType\.', '' }
    $node = [ordered]@{
        name         = $c.Name
        type         = $type
        automationId = $c.AutomationId
        className    = $c.ClassName
        bounds       = $bounds
    }
    $states = Get-States $element
    foreach ($k in $states.Keys) { $node[$k] = $states[$k] }
    $node.children = $children.ToArray()
    $node
}

function Write-Node($node, [string] $indent) {
    $line = $indent + $node.type
    if ($node.name) { $line += ' "' + ($node.name -replace "`r?`n", '\n') + '"' }
    if ($node.automationId) { $line += ' #' + $node.automationId }
    if ($node.className) { $line += ' .' + $node.className }
    if ($null -ne $node.bounds) { $line += ' [{0},{1} {2}x{3}]' -f $node.bounds }
    $states = @($node.Keys | Where-Object { $_ -notin @('name', 'type', 'automationId', 'className', 'bounds', 'children', 'invoke', 'focusable') } | ForEach-Object {
        $v = $node[$_]
        if ($v -is [bool] -and $v) { $_ } elseif ($v -is [array]) { '{0}={1}' -f $_, ($v -join '/') } else { '{0}={1}' -f $_, $v }
    })
    if ($states.Count -gt 0) { $line += ' {' + ($states -join ', ') + '}' }
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
$nodes = @($windows | ForEach-Object { ConvertTo-Node ($_.GetUpdatedCache($Cache)) 0 })
foreach ($node in $nodes) { Write-Node $node '' }
if ($Json) {
    $path = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Json)
    $text = ConvertTo-Json -InputObject $nodes -Depth 100
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding $false))
}
exit 0
