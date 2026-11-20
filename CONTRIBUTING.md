# Contributing to Prism

Thanks for helping. Bug reports, platform gaps and pull requests are welcome
on [GitHub](https://github.com/0xtlt/prismsdk/issues).

## Setup

Development happens on macOS (the Apple backends link Swift, so Xcode is
required); the Linux backend is developed in Docker.

```sh
mise install        # Rust 1.99 and the platform targets, Java, the Android SDK, mdBook
mise run test       # unit tests
mise run check      # type-check every crate on every supported target
mise run web        # the example app, restarting on save (also: macos, ios, android, macos-app, device)
mise run docs       # build the book into docs/book
```

Linux: `docker build -f docker/linux-dev.Dockerfile -t prism-linux-dev docker`,
then run `cargo` inside it as the CI does (`.github/workflows/ci.yml`).

## Windows

There is no native Windows backend yet: there `Application::run` panics
unless the app is built with the `web` feature. The whole workspace builds,
passes clippy and its tests on Windows (the `windows` CI job), with either
Rust toolchain from [rustup](https://rustup.rs) (`rustup toolchain install 1.99`):

- MSVC (`x86_64-pc-windows-msvc`, rustup's default), with the Visual Studio
  Build Tools ("Desktop development with C++", installed by an administrator);
- GNU (`x86_64-pc-windows-gnu`), with a MinGW-w64 GCC on `PATH` (WinLibs, for
  example): no administrator needed.

**Symbolic links.** Each crate's `LICENSE-APACHE` and `LICENSE-MIT`, and the
`docker` directory at the root, are symbolic links (so `cargo package` puts
the licenses in every crate). Git for Windows creates them only when cloning
with `git clone -c core.symlinks=true …` on an account allowed to create
links: an administrator, or any account once Developer Mode is on (Settings ›
System › For developers; turning it on needs an administrator). Otherwise
Git writes each link as a small text file holding its target. Building and
testing don't read those files, so they work the same; write
`crates/prism-cli/platform/docker` where the docs say `docker`, and leave
packaging to macOS, where releases are made
([RELEASING.md](https://github.com/0xtlt/prismsdk/blob/main/RELEASING.md)).

### A Windows machine over SSH

`scripts/windows-remote.sh` copies your worktree from macOS or Linux to a
Windows machine and runs cargo there. On the Windows machine:

1. An administrator installs the OpenSSH server once: Settings › System ›
   Optional features › OpenSSH Server, then sets the `sshd` service to start
   automatically (`Set-Service sshd -StartupType Automatic; Start-Service sshd`).
2. Work in a standard account. Its public keys go in
   `%USERPROFILE%\.ssh\authorized_keys` (an administrator's go in
   `C:\ProgramData\ssh\administrators_authorized_keys` instead).
3. In that account, install Rust and a toolchain as above.

On your machine, a `~/.ssh/config` entry (`Host windows-pc`, with its
`HostName`, `User` and `IdentityFile`), then:

```sh
export PRISM_WINDOWS_HOST=windows-pc     # PRISM_WINDOWS_DIR: remote directory, default %USERPROFILE%\prism
scripts/windows-remote.sh cargo clippy --workspace --all-targets -- -D warnings
scripts/windows-remote.sh cargo test --workspace
scripts/windows-remote.sh run "set RUST_BACKTRACE=1&& cargo test -p prismsdk-core"
scripts/windows-remote.sh get target/debug/counter.exe
```

Each `cargo` call syncs first: the files git knows, links followed, and
nothing else but `target` left on the other side. The header of the script
lists every command.

### Windows GUI from SSH

Programs started over SSH run in session 0, on a desktop that is never
shown. GUI apps start and open their windows there, but every window counts
as hidden: screen captures come out blank, and UI Automation only reaches a
window through its handle. `scripts/windows/uia-dump.ps1` does that: apps
that provide UI Automation themselves (WPF, Chrome) show their whole tree and
can be pressed with `-Invoke`, while controls that are child windows (classic
Win32 controls, WinForms) do not show at all. An app can still draw its own
window into an image there (a WinForms window did), so the in-process smoke
hooks (`PRISM_CLICK`, and `PRISM_SNAPSHOT` when the backend draws the snapshot
itself) can work over SSH as they do in Linux CI.

To run in the account's real desktop, `scripts/windows/run-in-session.ps1`
starts a command there through a scheduled task with an interactive logon
(no password, no administrator rights; the task is deleted afterwards), waits
for it and returns its output and exit code:

```sh
scripts/windows-remote.sh run "powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run-in-session.ps1 \"start notepad && powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\uia-dump.ps1 -ProcessName notepad\""
```

That needs the account to be signed in on the PC (at the screen, through
Remote Desktop, or left open behind another user with fast user switching).
Otherwise Windows accepts the task but never starts it, and the script exits
with 125. `scripts/windows/capture.ps1` (a window or the whole screen, as PNG)
also needs that session on screen and unlocked, and the PC awake. Over SSH,
a standard account cannot list sessions (`qwinsta` and `query user` show
nothing), WMI is refused (`Get-ScheduledTask` fails) and so is `taskkill /T`;
the scripts do without them.

## Where things are

| Path | |
|---|---|
| `crates/prism-core` | elements, styles, entities, reconciler, platform services: no platform code |
| `crates/prism-native` | patches → native views, flexbox layout (taffy) |
| `crates/prism-web` | the web backend (server, client JS and CSS in `src/`) |
| `crates/prism-{macos,ios,watchos,linux,android}` | the native backends |
| `crates/prism-ui` | the component library |
| `crates/prism` | the facade apps depend on |
| `crates/prism-apple-build/apple` | Swift: the bridge, the widget extension, the watchOS host |
| `crates/prism-cli/platform` | the bundling scripts and the Android Java host |
| `docs/` | this book (mdBook) |
| `examples/` | the counter app and an Axum server |

Files a crate needs at build or run time must live inside that crate's
directory: crates.io packages can't reach outside it. The CLI and
`prism-apple-build` embed their directories with generated lists, so a new
Swift, Java or script file only has to be added there.

## Before a pull request

```sh
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

- `prism`, `prism-core` and `prism-ui` deny undocumented public items
  (`missing_docs`, a warning that clippy's `-D warnings` turns into an error):
  give new public items a `///` line.
- Prefer the platform's real control or API, and report what a platform lacks
  through `cx.capabilities()` instead of failing silently.
- A feature that users see gets a section in the matching chapter of `docs/`,
  and a line in `CHANGELOG.md` under `Unreleased`.

Releases are described in [RELEASING.md](https://github.com/0xtlt/prismsdk/blob/main/RELEASING.md).

## License

Prism is licensed under either of the Apache License, Version 2.0 or the MIT
license, at your option. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in Prism by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
