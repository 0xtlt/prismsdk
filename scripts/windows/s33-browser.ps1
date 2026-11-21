# Security review S33: a fake browser for PRISM_BROWSER, given the sign-in's
# authorization URL (scripts/mock-oidc.py's). Before the user approves, it
# sends the app's loopback listener callbacks that are not the sign-in's:
# another path, another state, two states, no state. Then it approves, as
# the user would, and follows the provider's redirect to the real callback.
# Each step goes to $env:S33_LOG (the state and code redacted).
#
#   set PRISM_BROWSER=powershell -NoProfile -ExecutionPolicy Bypass -File <repo>\scripts\windows\s33-browser.ps1
#
# The sign-in must still end, signed in, with the real callback.
$ProgressPreference = 'SilentlyContinue'
$auth = [string]$args[0]
$log = if ($env:S33_LOG) { $env:S33_LOG } else { Join-Path $env:TEMP "s33-browser.log" }
$q = @{}
foreach ($kv in ([uri]$auth).Query.TrimStart('?').Split('&')) {
  $k, $v = $kv.Split('=', 2)
  $q[[uri]::UnescapeDataString($k)] = [uri]::UnescapeDataString(([string]$v).Replace('+', ' '))
}
$state = $q['state']
function Say([string]$s) {
  $s = $s.Replace($state, '<state>') -replace 'code=[^&]+', 'code=<code>'
  Add-Content -Path $log -Value $s -Encoding UTF8
}
$redirect = [uri]$q['redirect_uri']
Say "authorize: redirect_uri=$($q['redirect_uri'])"
$base = "http://127.0.0.1:$($redirect.Port)"
$wrong = @(
  "/callbackX?code=evil&state=$state",
  "/callback/x?code=evil&state=$state",
  "/callback?code=evil&state=another-state",
  "/callback?code=evil&state=$state&state=$state",
  "/callback?code=evil",
  "/?code=evil&state=$state",
  "/favicon.ico"
)
foreach ($p in $wrong) {
  $code = & curl.exe -s -m 5 -o NUL -w "%{http_code}" "$base$p"
  Say "not the sign-in's: $p -> $code"
}
Start-Sleep -Seconds 1
# The user approves: the provider redirects to the real callback.
$approve = $auth.Replace('/authorize?', '/approve?') + "&decision=allow"
$location = & curl.exe -s -m 10 -o NUL -w "%{redirect_url}" $approve
Say "approved: the provider redirects to $location"
$code = & curl.exe -s -m 10 -o NUL -w "%{http_code}" $location
Say "the real callback -> $code"
