set shell := ["bash", "-euo", "pipefail", "-c"]

# List development and acceptance commands.
default:
    @just --list

# Build all host SDK libraries (Linux production; macOS DRAM fallback).
build:
    cargo build --workspace --release --locked

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo test --workspace --locked

# Compile Criterion suites without claiming timing acceptance.
bench-build:
    cargo bench -p libcram-bench --no-run --locked

bench:
    cargo bench -p libcram-bench --locked

# Linux uses an isolated Docker target directory and read-only source tree.
linux:
    bash scripts/linux-check.sh

# cbindgen is isolated inside target/tools, never installed into a global bin directory.
header-tool:
    cargo install cbindgen --version 0.29.2 --locked --root target/tools

header:
    target/tools/bin/cbindgen --config cbindgen.toml --crate libcram-ffi --output include/libcram.h

header-check:
    mkdir -p target
    target/tools/bin/cbindgen --config cbindgen.toml --crate libcram-ffi --output target/libcram.h
    diff -u include/libcram.h target/libcram.h

check: fmt-check test lint bench-build
