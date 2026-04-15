#!/bin/bash
set -euo pipefail

# Locate cargo — try system PATH first, fall back to Bazel toolchain.
if ! command -v cargo > /dev/null 2>&1; then
  for candidate in "$HOME/.cargo/bin/cargo" "$HOME/.rustup/toolchains"/*/bin/cargo; do
    if [[ -x "$candidate" ]]; then
      export PATH="$(dirname "$candidate"):$PATH"
      break
    fi
  done
fi

if [[ -n "${CARGO_MANIFEST_DIR:-}" ]]; then
  MANIFEST="$CARGO_MANIFEST_DIR/Cargo.toml"
else
  MANIFEST="tapa-core/Cargo.toml"
fi

if [[ ! -f "$MANIFEST" ]]; then
  echo "Cannot find $MANIFEST" >&2
  exit 1
fi

exec cargo test --manifest-path "$MANIFEST" \
  -p tapa-protocol -p tapa-task-graph -p tapa-graphir
