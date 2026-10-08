#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
[[ "$(cbindgen --version)" == "cbindgen 0.29.2" ]] || { echo "Install cbindgen 0.29.2" >&2; exit 1; }
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
cbindgen --config cbindgen.toml --crate libcram-ffi --output "$tmp"
diff -u include/libcram.h "$tmp"
