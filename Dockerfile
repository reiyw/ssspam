FROM lukemathwalker/cargo-chef:latest-rust-1.59-bullseye AS chef
WORKDIR /app

FROM chef AS planner
COPY src src
COPY Cargo.toml .
COPY Cargo.lock .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json

RUN apt-get update && apt-get install -y --no-install-recommends \
    libopus-dev=1.3.1-0.1 \
    ffmpeg=7:4.3.3-0+deb11u1 \
    ;
RUN cargo chef cook --release --recipe-path recipe.json

COPY src src
COPY Cargo.toml .
RUN cargo build --release && cargo build --release --bin preload

FROM debian:bullseye-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    libopus-dev=1.3.1-0.1 \
    ffmpeg=7:4.3.3-0+deb11u1 \
    wget=1.21-1+deb11u1 \
    unzip=6.0-26 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir sound; \
    wget -q https://storage.googleapis.com/surfpvparena/2022-02-12.zip; \
    unzip 2022-02-12.zip -d sound; \
    rm -f 2022-02-12.zip;

COPY --from=builder /app/target/release/preload /usr/local/bin
RUN /usr/local/bin/preload --sound-dir /app/sound

COPY --from=builder /app/target/release/ssspambot /usr/local/bin
CMD ["/usr/local/bin/ssspambot", "--sound-dir", "/app/sound"]
