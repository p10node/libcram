ARG RUST_IMAGE=rust:latest
FROM ${RUST_IMAGE}
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 python3-dev python3-venv golang-go clang llvm pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
