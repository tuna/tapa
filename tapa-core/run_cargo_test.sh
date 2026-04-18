#!/bin/bash
set -euo pipefail

# Locate cargo via shared helper.
# shellcheck source=find_cargo.sh
source "$(dirname "$0")/find_cargo.sh"

if [[ -n "${CARGO_MANIFEST_DIR:-}" ]]; then
  MANIFEST="$CARGO_MANIFEST_DIR/Cargo.toml"
elif [[ -f "Cargo.toml" ]]; then
  MANIFEST="Cargo.toml"
elif [[ -f "tapa-core/Cargo.toml" ]]; then
  MANIFEST="tapa-core/Cargo.toml"
else
  echo "Cannot find Cargo.toml — run from tapa-core/ or repo root" >&2
  exit 1
fi

exec cargo test --manifest-path "$MANIFEST" \
  -p tapa-protocol \
  -p tapa-task-graph \
  -p tapa-graphir \
  -p tapa-rtl \
  -p tapa-topology \
  -p tapa-slotting \
  -p tapa-codegen \
  -p tapa-floorplan \
  -p tapa-lowering \
  -p tapa-graphir-export \
  -p tapa-xilinx \
  -p tapa-py-bindings
