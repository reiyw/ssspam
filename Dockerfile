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

FROM debian:buster-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    ;
COPY --from=builder /app/target/release/ssspambot /usr/local/bin
CMD ["/usr/local/bin/ssspambot"]
