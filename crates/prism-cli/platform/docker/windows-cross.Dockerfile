# Cross-compiling Prism apps for Windows (x86_64-pc-windows-gnu) with
# MinGW-w64, for `prism build windows` on macOS and Linux when MinGW-w64 is
# not installed. `windres` (binutils) embeds the app's resources; `curl` and
# `unzip` fetch the Windows App SDK packages from nuget.org.
#
#   docker build -f docker/windows-cross.Dockerfile -t prism-windows-cross docker
ARG RUST_VERSION=1.99
FROM rust:${RUST_VERSION}-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64 unzip \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu
