#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${LIBCRAM_LINUX_IMAGE:-rust:latest}"
PLATFORM="${LIBCRAM_LINUX_PLATFORM:-linux/amd64}"
mkdir -p "$ROOT/target/linux" "$ROOT/target/linux-cargo"
# Source is mounted read-only. All build/cache writes stay under owned target paths.
docker run --rm --platform "$PLATFORM" \
  --mount "type=bind,src=$ROOT,dst=/src,readonly" \
  --mount "type=bind,src=$ROOT/target/linux,dst=/target" \
  --mount "type=bind,src=$ROOT/target/linux-cargo,dst=/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/target -w /src "$IMAGE" \
  sh -eu -c '
    rustc --version
    rustup component add clippy
    cargo test --locked -p libcram-core -p libcram-ffi
    cargo test --locked -p libcram-core --features ebpf
    cargo clippy --locked -p libcram-core -p libcram-ffi --all-targets --features libcram-core/ebpf -- -D warnings
    cargo build --locked -p libcram-ffi --release
  '
