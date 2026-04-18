#!/bin/bash
set -euo pipefail

# Locate cargo via shared helper.
# shellcheck source=find_cargo.sh
source "$(dirname "$0")/find_cargo.sh"

if [[ -n "${CARGO_MANIFEST_DIR:-}" ]]; then
  WORKSPACE_DIR="$CARGO_MANIFEST_DIR"
elif [[ -f "Cargo.toml" ]]; then
  WORKSPACE_DIR="."
elif [[ -f "tapa-core/Cargo.toml" ]]; then
  WORKSPACE_DIR="tapa-core"
else
  echo "Cannot find Cargo.toml — run from tapa-core/ or repo root" >&2
  exit 1
fi

# Build from WORKSPACE_DIR so Cargo discovers .cargo/config.toml
cd "$WORKSPACE_DIR"

# Build the PyO3 extension (.cargo/config.toml handles platform flags).
cargo build -p tapa-py-bindings

# Copy the .dylib/.so to the expected Python extension name.
if [[ -f "target/debug/libtapa_core.dylib" ]]; then
  cp "target/debug/libtapa_core.dylib" "target/debug/tapa_core.so"
elif [[ -f "target/debug/libtapa_core.so" ]]; then
  cp "target/debug/libtapa_core.so" "target/debug/tapa_core.so"
fi

exec python3 "tests/python_smoke_test.py"
