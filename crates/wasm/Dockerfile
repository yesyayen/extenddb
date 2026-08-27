# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0
#
# Reusable builder image for the ExtendDB browser/WASM target.
#
# Why a container: sqlite-wasm-rs compiles the SQLite C amalgamation to wasm
# with clang, and its shim needs a modern clang (>= 14). The Amazon Linux 2 dev
# host has glibc 2.26 + only clang 11 (too old), and modern prebuilt clang
# needs glibc >= 2.27, so it cannot run natively here. This image (Debian
# bookworm, glibc 2.36) provides a modern clang + the wasm target + wasm-pack.
#
# Build:  docker build -t extenddb-wasm-builder:latest -f crates/wasm/Dockerfile .
# Use:    scripts/wasm-build.sh   (mounts the repo, emits crates/wasm/pkg/)
FROM public.ecr.aws/docker/library/rust:1-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends clang \
 && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown

# Official installer resolves the right wasm-pack binary (version-agnostic).
RUN curl -sSf https://rustwasm.github.io/wasm-pack/installer/init.sh | sh \
 && wasm-pack --version \
 && clang --version | head -1
