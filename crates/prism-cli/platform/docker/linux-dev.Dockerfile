# Building and testing Prism's native Linux backend (GTK 4) in a container,
# with a virtual display (Xvfb) for snapshots, a session bus and a
# notification daemon (dunst) for notifications, a keyring (gnome-keyring:
# Secret Service), python3-dbus for test services (a mock fprintd), and
# WebKitGTK 6.0 for web views (prismsdk-linux's `web-view` feature).
#
#   docker build -f docker/linux-dev.Dockerfile -t prism-linux-dev docker
ARG RUST_VERSION=1.99
FROM rust:${RUST_VERSION}-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      libgtk-4-dev xvfb xauth fonts-dejavu-core fonts-noto-color-emoji adwaita-icon-theme \
      dbus dbus-x11 dunst \
      gnome-keyring libsecret-tools python3-dbus python3-gi \
      libwebkitgtk-6.0-dev \
 && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy rustfmt
# Xvfb has no GPU: draw with Cairo.
ENV GSK_RENDERER=cairo
