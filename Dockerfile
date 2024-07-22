FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
RUN apt-get update && apt-get install -y --no-install-recommends libopus-dev protobuf-compiler libprotobuf-dev
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS ffmpeg
RUN apt-get update && apt-get install -y --no-install-recommends wget xz-utils \
    && wget -q --no-check-certificate https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz \
    && tar xf ffmpeg-master-latest-linux64-gpl.tar.xz \
    && mv ffmpeg-master-latest-linux64-gpl/bin/ffmpeg /usr/local/bin/ \
    && mv ffmpeg-master-latest-linux64-gpl/bin/ffprobe /usr/local/bin/

FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    libopus-dev \
    ca-certificates \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

COPY --from=builder /app/target/release/ssspam-bot /usr/local/bin
COPY --from=ffmpeg /usr/local/bin/ffmpeg /usr/local/bin
COPY --from=ffmpeg /usr/local/bin/ffprobe /usr/local/bin

ENTRYPOINT ["/usr/local/bin/ssspam-bot"]
