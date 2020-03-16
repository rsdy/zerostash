FROM ubuntu:18.04


LABEL name="rust-musl"
LABEL version="0.1.0"
LABEL repository="https://github.com/rsdy/zerostash"
LABEL homepage="https://github.com/rsdy/zerostash"
LABEL maintainer="Peter Parkanyi"

LABEL com.github.actions.name="Rust MUSL Builder"
LABEL com.github.actions.description="Provides a Rust MUSL environment"
LABEL com.github.actions.icon="settings"
LABEL com.github.actions.color="orange"

ENV BUILD_DIR=/build \
    OUTPUT_DIR=/output \
    RUST_BACKTRACE=1 \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    PREFIX=/toolchain \
    BUILD_TARGET=x86_64-unknown-linux-musl

RUN apt-get update \
    && apt-get install -y curl build-essential pkg-config musl musl-tools musl-dev \
    && apt-get clean -y

RUN curl https://sh.rustup.rs -sSf > rustup.sh \
    && sh rustup.sh -y \
          --default-toolchain nightly \
          --no-modify-path \
    && rustup target add $BUILD_TARGET \
    && rustup --version \
    && cargo --version \
    && rustc --version 

COPY entrypoint.sh /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]