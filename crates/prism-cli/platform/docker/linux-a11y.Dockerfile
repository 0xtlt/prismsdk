# Accessibility checks of the GTK backend (scripts/linux-a11y.sh): the
# AT-SPI bus (at-spi2-core) and its typelib (gir1.2-atspi-2.0) for
# scripts/linux/atspi-dump.py, which reads the tree Orca gets, and xdotool,
# which presses keys as a user would.
#
# GTK 4.8 (Debian 12, the oldest GTK Prism supports), on top of the dev
# image (docker/linux-dev.Dockerfile, built first):
#   docker build -f docker/linux-a11y.Dockerfile --target gtk48 -t prism-linux-a11y:gtk4.8 docker
# The newest GTK (Debian unstable):
#   docker build -f docker/linux-a11y.Dockerfile --target newest -t prism-linux-a11y:newest docker
ARG RUST_VERSION=1.99

FROM prism-linux-dev AS gtk48
RUN apt-get update && apt-get install -y --no-install-recommends \
      at-spi2-core gir1.2-atspi-2.0 xdotool \
 && rm -rf /var/lib/apt/lists/*

FROM debian:sid AS newest
ARG RUST_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential pkg-config \
      libgtk-4-dev xvfb xauth fonts-dejavu-core fonts-noto-color-emoji adwaita-icon-theme \
      dbus dbus-x11 at-spi2-core gir1.2-atspi-2.0 python3-gi xdotool \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain "${RUST_VERSION}" --component clippy,rustfmt
# Xvfb has no GPU: draw with Cairo.
ENV GSK_RENDERER=cairo
