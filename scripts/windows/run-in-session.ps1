<#
.SYNOPSIS
Runs a command in this user's interactive desktop session, from SSH, and
returns its output and exit code.

.DESCRIPTION
A program started from an SSH session runs in session 0, on a desktop that
is never shown: its windows exist but stay invisible, UI Automation sees
nothing inside them and screen captures are blank. A scheduled task with an
interactive logon type runs in the user's own session instead, as when they
start it themselves. This script registers one for the current user (Task
Scheduler COM API: no password, no administrator rights), runs it, waits,
prints what the command wrote (stdout and stderr) and deletes the task.

The command is a cmd.exe command line, written to a batch file (a literal %
is %%). It starts in -WorkingDirectory (the current directory by default),
in a hidden console: console programs show no window, GUI programs show
theirs. Its output is printed once it exits.

The user must be signed in on the PC (at the console, through Remote
Desktop or switched away from with fast user switching). Nothing runs
otherwise: Task Scheduler accepts the task but never starts it. Screen
captures also need that session to be on screen and unlocked; UI Automation
works in a locked session.

Exit codes: the command's own, 124 when it timed out (it is killed with
its child processes), 125 when it could not start (no interactive session
for this user).

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run-in-session.ps1 -Command "target\debug\counter.exe"

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run-in-session.ps1 -TimeoutSeconds 60 -Command "powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\capture.ps1 -Out screen.png"
#>
[CmdletBinding(DefaultParameterSetName = 'Run')]
param(
    # The cmd.exe command line to run in the session.
    [Parameter(Mandatory, Position = 0, ParameterSetName = 'Run')]
    [string] $Command,
    # Where the command starts.
    [Parameter(ParameterSetName = 'Run')]
    [string] $WorkingDirectory = (Get-Location).ProviderPath,
    # Kill the command (and its children) after this long.
    [double] $TimeoutSeconds = 300,
    # How long to wait for the session to start the task.
    [Parameter(ParameterSetName = 'Run')]
    [double] $StartSeconds = 20,
    # Internal: the task's side, running in the session.
    [Parameter(Mandatory, ParameterSetName = 'Inside')]
    [string] $Inside
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

# Written whole or not at all: the other side polls for these files.
function Write-Atomic([string] $path, [string] $text) {
    [IO.File]::WriteAllText("$path.tmp", $text)
    Move-Item -LiteralPath "$path.tmp" -Destination $path -Force
}

# --- In the session: run the batch file, record the session and the result.
if ($PSCmdlet.ParameterSetName -eq 'Inside') {
    Add-Type -Namespace PrismSession -Name Native -MemberDefinition '
        [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
        [DllImport("kernel32.dll")] public static extern System.IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll")] public static extern System.IntPtr CreateJobObject(System.IntPtr attributes, string name);
        [DllImport("kernel32.dll")] public static extern bool AssignProcessToJobObject(System.IntPtr job, System.IntPtr process);
        [DllImport("kernel32.dll")] public static extern bool TerminateJobObject(System.IntPtr job, uint exitCode);'
    # This process and everything the command starts share a job, so a timeout
    # stops them all (taskkill /T needs WMI, which SSH logons may not reach).
    # Without a timeout, programs the command leaves running keep running.
    $job = [PrismSession.Native]::CreateJobObject([IntPtr]::Zero, $null)
    $inJob = [PrismSession.Native]::AssignProcessToJobObject($job, [PrismSession.Native]::GetCurrentProcess())

    $session = (Get-Process -Id $PID).SessionId
    $console = [PrismSession.Native]::WTSGetActiveConsoleSessionId() -eq $session
    $locked = $null -ne (Get-Process -Name LogonUI -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $session })
    $state = 'session {0}, {1}, {2}' -f $session,
        $(if ($console) { 'on the console' } else { 'not on the console (Remote Desktop or switched away)' }),
        $(if ($locked) { 'lock screen up' } else { 'unlocked' })
    Write-Atomic (Join-Path $Inside 'session.txt') $state

    $batch = Join-Path $Inside 'command.cmd'
    $log = Join-Path $Inside 'output.log'
    $process = Start-Process -FilePath $env:ComSpec -NoNewWindow -PassThru `
        -ArgumentList ('/d /s /c "call "{0}" > "{1}" 2>&1"' -f $batch, $log)
    $handle = $process.Handle  # keeps the exit code readable after it exits
    $exitFile = Join-Path $Inside 'exit.code'
    if ($process.WaitForExit([int]($TimeoutSeconds * 1000))) {
        Write-Atomic $exitFile ([string]$process.ExitCode)
        exit 0
    }
    Write-Atomic $exitFile '124'
    if ($inJob) { [void][PrismSession.Native]::TerminateJobObject($job, 124) }  # ends this script too
    $process.Kill()
    exit 0
}

# --- From SSH: register the task, run it, wait, report, clean up.
$id = [guid]::NewGuid().ToString('N').Substring(0, 12)
$dir = Join-Path ([IO.Path]::GetTempPath()) "prism-session-$id"
$name = "prism-run-in-session-$id"
New-Item -ItemType Directory -Path $dir | Out-Null

$oem = [Text.Encoding]::GetEncoding([Globalization.CultureInfo]::CurrentCulture.TextInfo.OEMCodePage)
$cwd = (Resolve-Path -LiteralPath $WorkingDirectory).ProviderPath
[IO.File]::WriteAllText((Join-Path $dir 'command.cmd'), "@echo off`r`ncd /d `"$cwd`"`r`n$Command`r`n", $oem)

$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$scheduler = New-Object -ComObject Schedule.Service
$scheduler.Connect()
$folder = $scheduler.GetFolder('\')
$definition = $scheduler.NewTask(0)
$definition.RegistrationInfo.Description = 'Prism SDK scripts\windows\run-in-session.ps1: runs one command in this session, deleted afterwards.'
$definition.Principal.UserId = $user
$definition.Principal.LogonType = 3          # TASK_LOGON_INTERACTIVE_TOKEN: the user's session, no password
$definition.Principal.RunLevel = 0           # TASK_RUNLEVEL_LUA: no elevation
$definition.Settings.DisallowStartIfOnBatteries = $false
$definition.Settings.StopIfGoingOnBatteries = $false
$definition.Settings.ExecutionTimeLimit = 'PT{0}S' -f [int]($TimeoutSeconds + 120)
$action = $definition.Actions.Create(0)      # TASK_ACTION_EXEC
$action.Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$action.Arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "{0}" -Inside "{1}" -TimeoutSeconds {2}' -f $PSCommandPath, $dir, $TimeoutSeconds

$exitCode = 125
try {
    $task = $folder.RegisterTaskDefinition($name, $definition, 6, $user, $null, 3)  # TASK_CREATE_OR_UPDATE
    [void]$task.Run($null)

    $sessionFile = Join-Path $dir 'session.txt'
    $exitFile = Join-Path $dir 'exit.code'
    $started = [DateTime]::UtcNow
    $deadline = $started.AddSeconds($StartSeconds + $TimeoutSeconds + 30)
    while (-not (Test-Path $exitFile)) {
        $now = [DateTime]::UtcNow
        if (-not (Test-Path $sessionFile) -and $now -gt $started.AddSeconds($StartSeconds)) {
            # 0x00041303 (SCHED_S_TASK_HAS_NOT_RUN): accepted, never started.
            $result = $folder.GetTask($name).LastTaskResult
            $message = ('run-in-session: the task did not start in {0} s (last result 0x{1:X8}): ' +
                '{2} has no interactive session. Sign in on the PC as this user (console or Remote Desktop) ' +
                'and leave the session open.') -f $StartSeconds, $result, $user
            [Console]::Error.WriteLine($message)
            exit 125
        }
        if ($now -gt $deadline) {
            [Console]::Error.WriteLine('run-in-session: no result after {0} s' -f [int]($now - $started).TotalSeconds)
            exit 124
        }
        Start-Sleep -Milliseconds 250
    }
    $exitCode = [int]([IO.File]::ReadAllText($exitFile).Trim())
    $log = Join-Path $dir 'output.log'
    if (Test-Path $log) {
        # Programs the command started may still hold the log open.
        $stream = [IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
        $stdout = [Console]::OpenStandardOutput()
        $stream.CopyTo($stdout)
        $stream.Dispose()
        $stdout.Flush()
    }
    [Console]::Error.WriteLine('run-in-session: ' + [IO.File]::ReadAllText($sessionFile) + ", exit code $exitCode")
    if ($exitCode -eq 124) {
        [Console]::Error.WriteLine("run-in-session: timed out after $TimeoutSeconds s")
    }
} finally {
    try { $folder.DeleteTask($name, 0) } catch { }
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
}
exit $exitCode
