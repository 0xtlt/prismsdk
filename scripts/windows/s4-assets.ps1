# Security review S4 on Windows itself: the web server serves only portable
# asset names from inside the assets folder. Starts the counter's web server
# (built with `cargo build -p counter --no-default-features --features web
# --target-dir target\web`) on harmless marker files and asks it for files
# outside the folder in every way Windows paths allow: drive letters
# (`C:/…`, drive-relative `C:x`), backslashes, verbatim prefixes, alternate
# data streams, device names, dotfiles and trailing dots. Each request
# prints its status and whether a secret marker came back.
#
#   scripts/windows-remote.sh, or from the repository on the PC:
#   powershell -File scripts\windows\s4-assets.ps1 [-Server target\web\debug\counter.exe] [-Port 3171]
#
# Exit 1 if any secret leaks or the public file is not served.
param(
  [string]$Server = "target\web\debug\counter.exe",
  [int]$Port = 3171
)
$ProgressPreference = 'SilentlyContinue'
$root = Join-Path $env:TEMP "prism-s4"
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
$assets = Join-Path $root "assets"
$outside = Join-Path $root "outside"
$data = Join-Path $root "data"
New-Item -ItemType Directory -Force $assets, $outside, $data | Out-Null
Set-Content -NoNewline -Path (Join-Path $assets "ok.txt") -Value "PUBLIC-MARKER"
Set-Content -NoNewline -Path (Join-Path $assets ".env") -Value "SECRET-DOTFILE"
Set-Content -NoNewline -Path (Join-Path $assets "ok.txt") -Stream "hidden" -Value "SECRET-STREAM"
Set-Content -NoNewline -Path (Join-Path $outside "secret.txt") -Value "SECRET-OUTSIDE"
# The server's working directory: what a drive-relative name (`C:secret.txt`) reads.
Set-Content -NoNewline -Path (Join-Path $root "secret.txt") -Value "SECRET-CWD"

$env:PRISM_PORT = "$Port"; $env:PRISM_HOST = "127.0.0.1"
$env:PRISM_ASSETS_DIR = $assets; $env:PRISM_DATA_DIR = $data; $env:PRISM_LOCALE = "en"
$exe = (Resolve-Path $Server).Path
$srv = Start-Process -FilePath $exe -WorkingDirectory $root -PassThru -NoNewWindow `
  -RedirectStandardOutput (Join-Path $root "server.log") -RedirectStandardError (Join-Path $root "server.err")
$up = $false
for ($i = 0; $i -lt 60 -and -not $up; $i++) {
  $c = [Net.Sockets.TcpClient]::new()
  try { $up = $c.ConnectAsync("127.0.0.1", $Port).Wait(500) } catch {} finally { $c.Dispose() }
  if (-not $up) { Start-Sleep -Milliseconds 500 }
}
if (-not $up) { "s4-assets: FAIL: the server did not start"; Get-Content (Join-Path $root "server.err") -Tail 20; exit 1 }

$drive = $outside.Substring(0, 2)                 # C:
$abs = $outside.Replace('\', '/') + "/secret.txt" # C:/…/outside/secret.txt
$absBack = [uri]::EscapeDataString("$outside\secret.txt")
$verbatim = [uri]::EscapeDataString("\\?\$outside\secret.txt")
$requests = @(
  @{ path = "/assets/ok.txt"; public = $true },
  @{ path = "/assets/$abs" },
  @{ path = "/assets/$absBack" },
  @{ path = "/assets/$($drive)secret.txt" },
  @{ path = "/assets/$($drive.ToLower())secret.txt" },
  @{ path = "/assets/$verbatim" },
  @{ path = "/assets/" + [uri]::EscapeDataString("//?/$abs") },
  @{ path = "/assets/..%5C..%5Coutside%5Csecret.txt" },
  @{ path = "/assets/..\..\outside\secret.txt" },
  @{ path = "/assets/../outside/secret.txt" },
  @{ path = "/assets/%2e%2e/outside/secret.txt" },
  @{ path = "/assets/ok.txt:hidden" },
  @{ path = "/assets/ok.txt%3Ahidden" },
  @{ path = "/assets/ok.txt:hidden:`$DATA" },
  @{ path = "/assets/ok.txt::`$DATA" },
  @{ path = "/assets/.env" },
  @{ path = "/assets/ok.txt." },
  @{ path = "/assets/ok.txt%20" },
  @{ path = "/assets/NUL" },
  @{ path = "/assets/con.txt" },
  @{ path = "/assets/COM1" },
  @{ path = "/assets/aux.png" },
  @{ path = "/assets/CONIN`$" },
  @{ path = "/files/..%5C..%5Csecret.txt" },
  @{ path = "/files/$absBack" },
  @{ path = "/files/$abs" },
  @{ path = "/files/$($drive)secret.txt" },
  @{ path = "/files/00000000000000000000000000000000.txt:x" }
)
$body = Join-Path $root "body"
$failed = 0
$user = [regex]::Escape($env:USERNAME)
foreach ($r in $requests) {
  Remove-Item $body -ErrorAction SilentlyContinue
  $code = & curl.exe -s --path-as-is -m 10 -o $body -w "%{http_code}" "http://127.0.0.1:$Port$($r.path)"
  $got = if (Test-Path $body) { Get-Content -Raw $body } else { "" }
  $leak = $got -match "SECRET-"
  $ok = if ($r.public) { $code -eq "200" -and $got -match "PUBLIC-MARKER" } else { -not $leak -and $code -ne "200" }
  if (-not $ok) { $failed++ }
  $shown = $r.path -replace $user, "<user>"
  "{0} {1} {2}{3}" -f $(if ($ok) { "ok  " } else { "FAIL" }), $code, $shown, $(if ($leak) { "  LEAKED: $($got.Substring(0, [Math]::Min(40, $got.Length)))" } else { "" })
}
Stop-Process -Id $srv.Id -Force -ErrorAction SilentlyContinue
"server log:"; Get-Content (Join-Path $root "server.log"), (Join-Path $root "server.err") -Tail 8 -ErrorAction SilentlyContinue | ForEach-Object { "  " + ($_ -replace $user, "<user>") }
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
if ($failed) { "s4-assets: FAIL: $failed of $($requests.Count) requests"; exit 1 }
"s4-assets: OK: $($requests.Count) requests, only the public file served"
