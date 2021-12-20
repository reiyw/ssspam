FROM rust:1.57.0

WORKDIR /app

RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    ;

COPY . .

RUN cargo build --release

CMD ["/app/target/release/surfpvparena"]
