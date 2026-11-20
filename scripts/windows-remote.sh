#!/bin/sh
# Builds and tests on a Windows machine over SSH, from macOS or Linux: copies
# this git worktree there and runs cargo (or any command) in the copy.
#
#   scripts/windows-remote.sh sync                     copy the worktree
#   scripts/windows-remote.sh cargo <args…>            sync, then run cargo there
#   scripts/windows-remote.sh run '<command>'          run a cmd.exe command line there
#   scripts/windows-remote.sh ps <file.ps1> [args…]    run a local PowerShell script there
#   scripts/windows-remote.sh get <path> [<local-dir>] copy a file or folder back (default: .)
#   scripts/windows-remote.sh ssh [args…]              ssh to the machine
#
# `sync` copies the files git knows (tracked, and untracked but not ignored),
# with symbolic links replaced by what they point to (Windows accounts can
# rarely create links), and deletes everything else in the remote directory
# except `target`, so cargo builds incrementally. Commands run in the remote
# directory, through cmd.exe (`set RUST_BACKTRACE=1&& cargo test`); `ps`
# runs Windows PowerShell 5.1 with the execution policy bypassed. The exit
# code is the remote command's.
#
# Environment:
#   PRISM_WINDOWS_HOST      ssh destination: a Host of ~/.ssh/config or user@host (required)
#   PRISM_WINDOWS_DIR       remote directory, relative to the user's profile or absolute
#                           with forward slashes (default: prism)
#   PRISM_WINDOWS_SSH_OPTS  extra `-o` options for ssh and scp; port and key belong in
#                           ~/.ssh/config
#
# The machine needs the OpenSSH server, Rust (rustup) and tar.exe (part of
# Windows 10 and 11). See "Windows" in CONTRIBUTING.md.
set -eu

HOST=${PRISM_WINDOWS_HOST:-}
DIR=${PRISM_WINDOWS_DIR:-prism}
# One connection for every call (ControlMaster), kept 10 minutes; ssh expands
# the ~. Options from PRISM_WINDOWS_SSH_OPTS come first: ssh keeps the first
# value it is given.
SSH_OPTS="${PRISM_WINDOWS_SSH_OPTS:-} -o ControlMaster=auto -o ControlPath=~/.ssh/prism-windows-%C
  -o ControlPersist=10m -o ServerAliveInterval=30"

usage() {
  sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

[ $# -gt 0 ] || usage
[ -n "$HOST" ] || {
  echo "windows-remote: set PRISM_WINDOWS_HOST (an ssh destination, see the top of $0)" >&2
  exit 2
}

# shellcheck disable=SC2086,SC2029 # SSH_OPTS is a list of words; $@ runs remotely
ssh_() { ssh $SSH_OPTS "$HOST" "$@"; }
# shellcheck disable=SC2086
scp_() { scp -q $SSH_OPTS "$@"; }

# A PowerShell string literal.
ps_quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/''/g")"; }

# Runs PowerShell code on the machine, in the remote directory (created if
# needed). powershell.exe writes its own errors and progress to a redirected
# stderr as CLIXML: errors go through [Console]::Error instead, and progress
# is off. Programs it starts write to stdout and stderr directly.
remote_ps() {
  script="\$ProgressPreference = 'SilentlyContinue'
\$ErrorActionPreference = 'Stop'
try {
    Set-Location -LiteralPath \$env:USERPROFILE
    New-Item -ItemType Directory -Force -Path $(ps_quote "$DIR") | Out-Null
    Set-Location -LiteralPath $(ps_quote "$DIR")
    \$ErrorActionPreference = 'Continue'
    $1
} catch {
    [Console]::Error.WriteLine('windows-remote: ' + \$_)
    exit 1
}"
  # cmd.exe, the default shell, takes 8191 characters: keep $1 short.
  encoded=$(printf '%s' "$script" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')
  ssh_ "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encoded"
}

# Runs a cmd.exe command line in the remote directory. cmd /s drops the
# outer quotes PowerShell adds around the line (the trailing space makes
# sure it adds them), so the line runs as typed.
remote_cmd() {
  remote_ps "& \$env:ComSpec /d /s /c ($(ps_quote "$1") + ' ')
    exit \$LASTEXITCODE"
}

# A word for a cmd.exe command line.
cmd_quote() {
  case $1 in
    '' | *[[:space:]\&\|\<\>^\(\)]*) printf '"%s"' "$1" ;;
    *) printf '%s' "$1" ;;
  esac
}

sync() {
  root=$(git rev-parse --show-toplevel)
  archive=$(mktemp "${TMPDIR:-/tmp}/prism-windows-XXXXXX")
  trap 'rm -f "$archive"' EXIT
  # bsdtar (macOS) would add ._ files for extended attributes.
  no_mac=
  if tar --version 2>/dev/null | grep -q bsdtar; then no_mac=--no-mac-metadata; fi
  # shellcheck disable=SC2016 # the inner script expands its own $f
  (cd "$root" && git ls-files -co --exclude-standard -z |
    xargs -0 sh -c 'for f; do [ -e "$f" ] && printf "%s\n" "$f"; done' sh |
    COPYFILE_DISABLE=1 tar $no_mac -c -z -h -f - -T -) >"$archive"
  name="prism-sync-$$.tar.gz"
  scp_ "$archive" "$HOST:$name"
  remote_ps "\$archive = Join-Path \$env:USERPROFILE '$name'
    try {
        Get-ChildItem -Force | Where-Object Name -ne 'target' | Remove-Item -Recurse -Force
        & \"\$env:SystemRoot\\System32\\tar.exe\" -x -z -f \$archive
        if (\$LASTEXITCODE -ne 0) { exit \$LASTEXITCODE }
    } finally {
        Remove-Item -LiteralPath \$archive -ErrorAction SilentlyContinue
    }"
  echo "windows-remote: synced to $HOST:$DIR" >&2
}

cmd=$1
shift
case $cmd in
  sync)
    sync
    ;;
  cargo)
    sync
    line=cargo
    for arg in "$@"; do line="$line $(cmd_quote "$arg")"; done
    remote_cmd "$line"
    ;;
  run)
    [ $# -eq 1 ] || usage
    remote_cmd "$1"
    ;;
  ps)
    [ $# -ge 1 ] || usage
    file=$1
    shift
    name="prism-ps-$$.ps1"
    scp_ "$file" "$HOST:$name"
    line="powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%USERPROFILE%\\$name\""
    for arg in "$@"; do line="$line $(cmd_quote "$arg")"; done
    status=0
    remote_cmd "$line" || status=$?
    remote_ps "Remove-Item -LiteralPath (Join-Path \$env:USERPROFILE '$name')" || true
    exit "$status"
    ;;
  get)
    [ $# -ge 1 ] || usage
    dest=${2:-.}
    mkdir -p "$dest"
    scp_ -r "$HOST:$DIR/$1" "$dest/"
    ;;
  ssh)
    ssh_ "$@"
    ;;
  *)
    usage
    ;;
esac
