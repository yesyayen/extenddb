#!/usr/bin/env bash
# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0
#
# Build the Stage B one-module integration (aws-sdk-dynamodb + extenddb engine)
# inside the modern-clang wasm container, same as scripts/wasm-build.sh.
#   scripts/wasm-dynein-build.sh [wasm-pack args...]   (default: --target nodejs --dev)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="extenddb-wasm-builder:latest"
ARGS=("$@")
if [ ${#ARGS[@]} -eq 0 ]; then
  ARGS=(--target nodejs --dev)
fi

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "Building $IMAGE ..." >&2
  docker build -t "$IMAGE" -f "$REPO/crates/wasm/Dockerfile" "$REPO"
fi

exec docker run --rm \
  --user "$(id -u):$(id -g)" \
  -v "$REPO":/work -w /work \
  -e CARGO_HOME=/work/.cargo-wasm-container \
  -e CARGO_TARGET_DIR=/work/target/wasm-container \
  -e HOME=/work/.home-wasm-container \
  -e CFLAGS_wasm32_unknown_unknown=-std=gnu2x \
  "$IMAGE" \
  bash -c "unset CC CXX CFLAGS; wasm-pack build crates/dynein-wasm ${ARGS[*]}"
