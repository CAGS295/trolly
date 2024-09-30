FROM alpine:latest AS openssl

RUN apk update && apk add --no-cache openssl-dev

# Rust
FROM alpine:latest AS rust

RUN apk add --update --no-cache \
    perl \
    make \
    gcc \
    libc-dev \
    protobuf-dev

# Install Rust
RUN apk update && apk add --no-cache curl \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    && export PATH="$HOME/.cargo/bin:$PATH" \
    && rustup default stable

# Set up environment variables
ENV USER=root \
    CARGO_HOME=/root/.cargo \
    RUSTUP_HOME=/root/.rustup \
    PATH=$PATH:/root/.cargo/bin

COPY --from=openssl /usr/lib/libssl* /usr/lib/
COPY --from=openssl /usr/lib/libcrypto* /usr/lib/

WORKDIR /trolly

# Copy in the rest of the source code and build the application
COPY Cargo.toml .
COPY src ./src
COPY examples ./examples
COPY benches ./benches

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=target cargo build --release && mv ./target/release/depth_monitor ./depth_monitor

FROM alpine:latest

COPY --from=openssl /usr/lib/libssl* /usr/lib/
COPY --from=openssl /usr/lib/libcrypto* /usr/lib/
COPY --from=rust /trolly/depth_monitor /usr/bin

# Set LD_LIBRARY_PATH environment variable
ENV LD_LIBRARY_PATH=/usr/lib/