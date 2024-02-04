# must be run from the root directory
FROM rust:1.75-slim-bullseye as builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev libasound2-dev portaudio19-dev build-essential libpulse-dev libdbus-1-dev libudev-dev && rm -rf /var/lib/apt/lists/*
COPY examples/certificates /app/examples/certificates
COPY examples/simple_box /app/examples/simple_box
COPY lightyear /app/lightyear
COPY macros /app/macros
COPY vendor /app/vendor
WORKDIR /app/examples/simple_box
RUN cargo build --release

EXPOSE 4006/udp

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y libssl-dev ca-certificates libasound2-dev
COPY --from=builder /app/examples/simple_box/target/release/simple_box /app/examples/simple_box/simple_box
COPY --from=builder /app/examples/certificates /app/examples/certificates
WORKDIR /app/examples/simple_box
RUN chmod +x simple_box
ENTRYPOINT ["./simple_box", "server", "--port=4006", "--headless"]