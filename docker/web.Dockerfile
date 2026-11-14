# A Prism web app as a small Linux image (server binary + hashed static files).
#
#   docker build -f docker/web.Dockerfile --build-arg PACKAGE=counter -t my-app .
#   docker run -p 3000:3000 -v my-app-data:/data my-app
#
# Behind a CDN: upload /app/public (or let the CDN cache /_prism/* and
# /assets/* from the server) and set PRISM_ASSET_URL.
ARG RUST_VERSION=1.99

FROM rust:${RUST_VERSION}-bookworm AS build
ARG PACKAGE
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target-docker \
    cargo build --release -p "${PACKAGE}" --no-default-features --features web --target-dir /src/target-docker \
 && cp /src/target-docker/release/"${PACKAGE}" /app-server \
 && mkdir -p /app-public \
 && cd "$(cargo metadata --format-version 1 --no-deps | python3 -c "import json,sys,os; m=json.load(sys.stdin); print(os.path.dirname(next(p['manifest_path'] for p in m['packages'] if p['name']=='${PACKAGE}')))")" \
 && PRISM_EXPORT_STATIC=/app-public /app-server \
 && ASSETS="$(PRISM_DESCRIBE=assets /app-server)" \
 && if [ -n "$ASSETS" ] && [ -d "$ASSETS" ]; then cp -R "$ASSETS" /app-assets; else mkdir /app-assets; fi

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app-server /app/server
COPY --from=build /app-public /app/public
COPY --from=build /app-assets /app/assets
WORKDIR /app
ENV PRISM_HOST=0.0.0.0 PRISM_PORT=3000 PRISM_DATA_DIR=/data PRISM_ASSETS_DIR=/app/assets
VOLUME /data
EXPOSE 3000
CMD ["/app/server"]
