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
    ffmpeg \
    ;
RUN cargo chef cook --release --recipe-path recipe.json

COPY src src
COPY Cargo.toml .
RUN cargo build --release && cargo build --release --bin preload

FROM chef AS donwload_sound
RUN apt-get update && apt-get install -y --no-install-recommends \
    wget=1.21-1+deb11u1 \
    unzip=6.0-26 \
    ;
ARG SOUNDS_FILE=2022-07-16.zip
RUN mkdir sound; \
    wget -q https://storage.googleapis.com/surfpvparena/${SOUNDS_FILE}; \
    unzip ${SOUNDS_FILE} -d sound

FROM debian:bullseye-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    libopus-dev=1.3.1-0.1 \
    ffmpeg \
    ca-certificates=20210119 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

COPY --from=donwload_sound /app/sound /app/sound

COPY --from=builder /app/target/release/ssspambot /usr/local/bin

CMD ["/usr/local/bin/ssspambot", "--sound-dir", "/app/sound"]
