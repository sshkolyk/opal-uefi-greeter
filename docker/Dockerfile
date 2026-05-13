FROM rust:slim-bookworm

ARG TOOLCHAIN_VERSION=nightly-2022-11-22

RUN rustup toolchain install ${TOOLCHAIN_VERSION} --component rust-src \
    && rustup default ${TOOLCHAIN_VERSION} \
    && rustup target add --toolchain ${TOOLCHAIN_VERSION} x86_64-unknown-uefi \
    && apt update \
    && apt -y install gdisk dosfstools sudo mtools \
    && apt clean \
    && rm -rf /var/cache/apt/archives /var/lib/apt/lists/*

WORKDIR /build
