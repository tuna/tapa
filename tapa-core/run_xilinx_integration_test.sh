#!/bin/bash
# Driver for the gated `tapa_xilinx_integration_test` sh_test.
#
# Behavior:
#   1. If `VARS.local.bzl` is present, extract its `REMOTE_*` and
#      `XILINX_*` assignments and export them to the environment so
#      the ignored integration tests can reach the remote host.
#   2. Run `cargo test -p tapa-xilinx -- --ignored`.
#   3. Skip cleanly (exit 0) when neither a local `XILINX_HLS` install
#      nor a remote host is available — the target is tagged `manual`
#      so default CI never blocks on it.

set -euo pipefail

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

# Locate VARS.local.bzl: walk up from the cargo manifest to the repo
# root.
vars_local=""
for candidate in \
  "$(dirname "$MANIFEST")/../VARS.local.bzl" \
  "$(pwd)/VARS.local.bzl" \
  "$(dirname "$MANIFEST")/VARS.local.bzl"; do
  if [[ -f "$candidate" ]]; then
    vars_local="$candidate"
    break
  fi
done

# Parse Starlark-like `NAME = "value"` assignments into the
# environment. Only REMOTE_* / XILINX_* names are imported; other
# entries are ignored.
if [[ -n "$vars_local" ]]; then
  echo "tapa_xilinx_integration_test: loading env from $vars_local" >&2
  while IFS= read -r line; do
    # Strip comments and leading whitespace.
    trimmed="${line%%#*}"
    trimmed="${trimmed## }"
    case "$trimmed" in
      REMOTE_*\=*|XILINX_*\=*)
        key="${trimmed%%=*}"
        key="${key%% }"
        # Strip surrounding quotes from the value.
        val="${trimmed#*=}"
        val="${val## }"
        val="${val%\"}"
        val="${val#\"}"
        val="${val%\'}"
        val="${val#\'}"
        # Expand leading ~/ for SSH key paths.
        if [[ "$val" == "~/"* ]]; then
          val="${HOME}/${val#~/}"
        fi
        export "$key=$val"
        ;;
      *) ;;
    esac
  done < "$vars_local"
fi

if [[ -z "${XILINX_HLS:-}" ]] && [[ -z "${REMOTE_HOST:-}" ]]; then
  echo "tapa_xilinx_integration_test: neither XILINX_HLS nor REMOTE_HOST is set; skipping" >&2
  exit 0
fi

exec cargo test --manifest-path "$MANIFEST" -p tapa-xilinx -- --ignored
