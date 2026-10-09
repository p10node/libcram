#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${LIBCRAM_LINUX_IMAGE:-libcram-sdk-test:local}"
PLATFORM="${LIBCRAM_LINUX_PLATFORM:-linux/amd64}"
mkdir -p "$ROOT/target/linux" "$ROOT/target/linux-cargo"
docker run --rm --platform "$PLATFORM" \
  --mount "type=bind,src=$ROOT,dst=/src,readonly" \
  --mount "type=bind,src=$ROOT/target/linux,dst=/target" \
  --mount "type=bind,src=$ROOT/target/linux-cargo,dst=/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/target -e PYTHONDONTWRITEBYTECODE=1 \
  -e LIBCRAM_TEST_TORCH="${LIBCRAM_TEST_TORCH:-0}" -w /src "$IMAGE" \
  bash -euo pipefail -c '
    bash tests/bindings/native-smoke.sh
    python3 -m venv /target/python-venv
    /target/python-venv/bin/pip install "maturin>=1.9,<2" "pytest>=8,<10"
    export VIRTUAL_ENV=/target/python-venv
    /target/python-venv/bin/maturin develop --release --manifest-path bindings/python/Cargo.toml --locked
    /target/python-venv/bin/python -m pytest -p no:cacheprovider tests/bindings/test_python.py -q
    if [[ "${LIBCRAM_TEST_TORCH:-0}" == 1 ]]; then
      /target/python-venv/bin/pip install torch numpy --index-url https://download.pytorch.org/whl/cpu
      /target/python-venv/bin/python -m pytest -p no:cacheprovider tests/bindings/test_torch.py -q
    fi
  '
