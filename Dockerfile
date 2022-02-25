FROM lukemathwalker/cargo-chef:latest-rust-1.57.0 AS chef
WORKDIR /app

FROM chef AS planner
COPY src .
COPY Cargo.toml .
COPY Cargo.lock .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json

RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    ;
RUN cargo chef cook --release --recipe-path recipe.json

COPY src .
COPY Cargo.toml .
RUN cargo build --release
RUN cargo build --release --bins

FROM debian:buster-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    wget \
    unzip \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir sound; \
    cd sound; \
    echo foo; \
    wget https://storage.googleapis.com/surfpvparena/2022-02-12.zip; \
    unzip 2022-02-12.zip; \
    rm -f 2022-02-12.zip;

COPY --from=builder /app/target/release/preload /usr/local/bin
RUN /usr/local/bin/preload --sound-dir /app/sound

COPY --from=builder /app/target/release/ssspambot /usr/local/bin
CMD ["/usr/local/bin/ssspambot"]
