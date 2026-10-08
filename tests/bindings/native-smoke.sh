#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -p libcram-ffi
# Set CARGO_TARGET_DIR to the same directory used by Cargo if customized.
libdir="${CARGO_TARGET_DIR:-$PWD/target}/debug"
libdir="$(cd "$libdir" && pwd)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -Iinclude tests/bindings/c_smoke.c -L"$libdir" -lcram -Wl,-rpath,"$libdir" -pthread -o "$tmp/c-smoke"
"$tmp/c-smoke"
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror -Iinclude tests/bindings/cpp_smoke.cpp -L"$libdir" -lcram -Wl,-rpath,"$libdir" -pthread -o "$tmp/cpp-smoke"
"$tmp/cpp-smoke"
(cd bindings/go && CGO_LDFLAGS="-L$libdir -Wl,-rpath,$libdir ${CGO_LDFLAGS:-}" GOEXPERIMENT=cgocheck2 go test -race -count=1 ./...)
echo "C, C++17 and Go native smokes passed"
