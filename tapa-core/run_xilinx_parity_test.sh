#!/bin/bash
# Runs the Xilinx cross-language parity subset after refreshing the
# `tapa_core` PyO3 extension in-place.
#
# Python interpreter resolution (first match wins):
#   1. $TAPA_PARITY_PYTHON if set.
#   2. Any candidate in $PATH that has `pytest` importable.
#   3. A small built-in list of common brew / venv paths.

set -euo pipefail

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

cd "$WORKSPACE_DIR"

cargo build -p tapa-py-bindings

if [[ -f "target/debug/libtapa_core.dylib" ]]; then
  cp "target/debug/libtapa_core.dylib" "target/debug/tapa_core.so"
elif [[ -f "target/debug/libtapa_core.so" ]]; then
  cp "target/debug/libtapa_core.so" "target/debug/tapa_core.so"
fi

have_pytest() {
  [[ -x "$1" ]] && "$1" -c 'import pytest' >/dev/null 2>&1
}

pick_python() {
  if [[ -n "${TAPA_PARITY_PYTHON:-}" ]]; then
    if have_pytest "$TAPA_PARITY_PYTHON"; then
      echo "$TAPA_PARITY_PYTHON"
      return 0
    fi
    echo "TAPA_PARITY_PYTHON=$TAPA_PARITY_PYTHON lacks pytest" >&2
    return 1
  fi
  local candidates=(
    "$(command -v python3 2>/dev/null || true)"
    "/opt/homebrew/bin/python3"
    "/usr/local/bin/python3"
    "$(dirname "$WORKSPACE_DIR")/.venv-tapa/bin/python3"
    "$WORKSPACE_DIR/.venv-tapa/bin/python3"
  )
  for candidate in "${candidates[@]}"; do
    if [[ -n "$candidate" ]] && have_pytest "$candidate"; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}

if ! PY=$(pick_python); then
  echo "run_xilinx_parity_test.sh: no python3 with pytest; set TAPA_PARITY_PYTHON" >&2
  exit 1
fi

echo "run_xilinx_parity_test.sh: using $PY" >&2
exec "$PY" -m pytest tests/parity_test.py -k xilinx -q
