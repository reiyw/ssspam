FROM lukemathwalker/cargo-chef:latest-rust-1.57.0 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json

RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    ;
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release
RUN cargo build --release --bins

FROM debian:buster-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    wget \
    unzip \
    ;

RUN mkdir sound; \
    cd sound; \
    wget https://storage.googleapis.com/surfpvparena/2021-12-25_2.zip; \
    unzip 2021-12-25_2.zip; \
    rm -f 2021-12-25_2.zip;

COPY --from=builder /app/target/release/preload /usr/local/bin
RUN /usr/local/bin/preload --sound-dir /app/sound

COPY --from=builder /app/target/release/ssspambot /usr/local/bin
CMD ["/usr/local/bin/ssspambot"]
