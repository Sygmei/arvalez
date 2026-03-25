FROM rust:1.94-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY plugins ./plugins

RUN cargo build --release -p arvalez-cli

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

COPY --from=builder /app/target/release/arvalez-cli /usr/local/bin/arvalez-cli

ENTRYPOINT ["/usr/local/bin/arvalez-cli"]
CMD ["--help"]
