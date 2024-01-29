# must be run from the root directory
FROM rust:1.75-slim-bullseye as builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev libasound2-dev portaudio19-dev build-essential libpulse-dev libdbus-1-dev libudev-dev && rm -rf /var/lib/apt/lists/*
COPY examples/simple_box /app/examples/simple_box
COPY lightyear /app/lightyear
COPY macros /app/macros
COPY vendor /app/vendor
RUN echo $(ls -1 /app)
WORKDIR /app/examples/simple_box
# COPY Cargo.toml Cargo.lock ./
# RUN mkdir src && \
#     echo "fn main() {}" > src/main.rs && \
#     cargo build --release
# COPY src ./src
# RUN touch src/main.rs && cargo build --release && cargo build --features lightyear/rivet --release --bin rivet_backend
RUN cargo build --release

# expose udp port for server
EXPOSE 5000/udp
# expose tcp port for backend
EXPOSE 4000/tcp

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y libssl-dev ca-certificates
COPY --from=builder /app/target/release/simple_box /usr/local/bin/simple_box
CMD ["simple_box -- server --connection=rivet"]