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

# Hardware acceptance: measures then fails if any 4KB distribution exceeds 1us.
entropy-budget:
    cargo bench -p libcram-bench --bench entropy --locked -- 'entropy/dispatch/.*/4096'
    python3 scripts/entropy_budget.py

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

# The kernel program needs nightly rust-src and a BPF linker; this is not kernel-load acceptance.
ebpf-tools:
    cargo install bpf-linker --version 0.11.1 --locked --root target/tools

ebpf:
    PATH="$PWD/target/tools/bin:$PATH" CARGO_TARGET_BPFEL_UNKNOWN_NONE_LINKER=bpf-linker RUSTFLAGS="--cfg bpf_target_arch=\"x86_64\"" cargo +nightly build --manifest-path crates/libcram-ebpf/Cargo.toml --release --target bpfel-unknown-none -Z build-std=core --locked

linux-image:
    docker build --platform linux/amd64 -f tools/linux-sdk.Dockerfile -t libcram-sdk-test:local .

python-tools:
    uv venv target/python-venv --python python3
    uv pip install --python target/python-venv/bin/python 'maturin>=1.9,<2' 'pytest>=8,<10'

python-test:
    VIRTUAL_ENV="$PWD/target/python-venv" target/python-venv/bin/maturin develop --release --manifest-path bindings/python/Cargo.toml --locked
    target/python-venv/bin/python -m pytest tests/bindings/test_python.py -q

torch-tools:
    uv pip install --python target/python-venv/bin/python torch numpy --index-url https://download.pytorch.org/whl/cpu

torch-test: python-test
    target/python-venv/bin/python -m pytest tests/bindings/test_torch.py -q

pebs-probe:
    cargo run -p libcram-core --example pebs_probe --features ebpf --locked

go-test:
    cargo build -p libcram-ffi --locked
    cd bindings/go && CGO_LDFLAGS="-L$PWD/../../target/debug -Wl,-rpath,$PWD/../../target/debug" go test -race -count=1 ./...

cpp-test:
    cargo build -p libcram-ffi --locked
    c++ -std=c++17 -Wall -Wextra -Werror -Iinclude tests/bindings/cpp_smoke.cpp -Ltarget/debug -lcram -Wl,-rpath,$PWD/target/debug -o target/cpp-smoke
    target/cpp-smoke

linux-bindings:
    bash scripts/linux-bindings.sh

# Native C, C++ and Go checks, including cgo's stricter pointer checker.
native-test:
    bash tests/bindings/native-smoke.sh

bindings: native-test python-test

check: fmt-check test lint bench-build
    python3 -m unittest discover -s scripts -p test_entropy_budget.py
