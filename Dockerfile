# Two stages: the builder carries the Rust toolchain, the runtime carries neither it nor cargo.
FROM rust:1-slim-bookworm AS builder
WORKDIR /src
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config \
 && rm -rf /var/lib/apt/lists/*
# rust-toolchain.toml is deliberately not copied. It pins rustfmt and clippy, which a release
# build never runs, and copying it makes every image build download a second toolchain on top
# of the one this image already ships.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p hermes-cli

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 10001 hermes \
 && mkdir -p /data && chown hermes:hermes /data
WORKDIR /app
COPY --from=builder /src/target/release/hermes /usr/local/bin/hermes
COPY static ./static
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# /data is where the volume mounts. Without one the database lives in the container
# filesystem and every redeploy silently starts from an empty scan.
ENV HERMES_DB=sqlite:///data/hermes.db \
    HERMES_STATIC_DIR=/app/static \
    HERMES_RPC_URL=https://mainnet.base.org \
    HERMES_CONCURRENCY=3 \
    PORT=8080

USER hermes
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
