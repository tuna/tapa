#!/bin/bash
set -euo pipefail

# Locate cargo — try system PATH first, fall back to common install paths.
if ! command -v cargo > /dev/null 2>&1; then
  for candidate in "$HOME/.cargo/bin/cargo" "$HOME/.rustup/toolchains"/*/bin/cargo; do
    if [[ -x "$candidate" ]]; then
      export PATH="$(dirname "$candidate"):$PATH"
      break
    fi
  done
fi

if [[ -n "${CARGO_MANIFEST_DIR:-}" ]]; then
  WORKSPACE_DIR="$CARGO_MANIFEST_DIR"
else
  WORKSPACE_DIR="tapa-core"
fi

MANIFEST="$WORKSPACE_DIR/Cargo.toml"
if [[ ! -f "$MANIFEST" ]]; then
  echo "Cannot find $MANIFEST" >&2
  exit 1
fi

# Build the PyO3 extension.
export PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1
if [[ "$(uname)" == "Darwin" ]]; then
  export RUSTFLAGS="-C link-arg=-undefined -C link-arg=dynamic_lookup"
fi
cargo build --manifest-path "$MANIFEST" -p tapa-py-bindings

# Copy the .dylib/.so to the expected Python extension name.
TARGET_DIR="$WORKSPACE_DIR/target/debug"
if [[ -f "$TARGET_DIR/libtapa_core.dylib" ]]; then
  cp "$TARGET_DIR/libtapa_core.dylib" "$TARGET_DIR/tapa_core.so"
elif [[ -f "$TARGET_DIR/libtapa_core.so" ]]; then
  cp "$TARGET_DIR/libtapa_core.so" "$TARGET_DIR/tapa_core.so"
fi

exec python3 "$WORKSPACE_DIR/tests/python_smoke_test.py"
