# Building and testing Prism's native Linux backend (GTK 4) in a container,
# with a virtual display (Xvfb) for snapshots.
#
#   docker build -f docker/linux-dev.Dockerfile -t prism-linux-dev docker
ARG RUST_VERSION=1.99
FROM rust:${RUST_VERSION}-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      libgtk-4-dev xvfb xauth fonts-dejavu-core fonts-noto-color-emoji adwaita-icon-theme \
 && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy rustfmt
# Xvfb has no GPU: draw with Cairo.
ENV GSK_RENDERER=cairo
