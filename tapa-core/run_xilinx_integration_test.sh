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
# environment. Accepts any amount of whitespace around `=` (the Starlark
# convention in VARS.local.bzl) and strips surrounding single or double
# quotes from the value. `REMOTE_*`, `XILINX_*`, and `TAPA_*` names
# are imported; other entries are ignored. The `TAPA_*` namespace
# carries opt-in shared-vadd flags such as `TAPA_SHARED_VADD_HLS`;
# the retired `TAPA_USE_RUST_*` flag-parity flow no longer exists.
# Docstrings/comments/blank lines skipped.
if [[ -n "$vars_local" ]]; then
  echo "tapa_xilinx_integration_test: loading env from $vars_local" >&2
  while IFS= read -r line; do
    # Drop inline comments (anything after the first `#`).
    line="${line%%#*}"
    # Trim leading + trailing whitespace (spaces and tabs).
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    # Skip blank lines and lines without an `=` (e.g. docstrings).
    [[ -z "$line" ]] && continue
    [[ "$line" != *=* ]] && continue

    key="${line%%=*}"
    val="${line#*=}"
    # Trim whitespace around key and value.
    key="${key#"${key%%[![:space:]]*}"}"
    key="${key%"${key##*[![:space:]]}"}"
    val="${val#"${val%%[![:space:]]*}"}"
    val="${val%"${val##*[![:space:]]}"}"
    # Require an uppercase REMOTE_* / XILINX_* / TAPA_* name (skip
    # fields like `foo = 3` inside a table or other unrelated
    # Starlark). `TAPA_*` covers the opt-in flags (e.g.
    # `TAPA_SHARED_VADD_HLS`) the integration target honors.
    case "$key" in
      REMOTE_*|XILINX_*|TAPA_*) ;;
      *) continue ;;
    esac
    # Strip surrounding quotes from the value.
    case "$val" in
      \"*\") val="${val#\"}"; val="${val%\"}" ;;
      \'*\') val="${val#\'}"; val="${val%\'}" ;;
    esac
    # Expand leading `~/` for SSH key paths. Use `${val:2}` rather
    # than `${val#~/}` — bash performs tilde expansion inside parameter
    # expansion patterns, which breaks the literal match.
    if [[ "$val" == "~/"* ]]; then
      val="${HOME}/${val:2}"
    fi
    export "$key=$val"
  done < "$vars_local"
fi

if [[ -z "${XILINX_HLS:-}" ]] && [[ -z "${REMOTE_HOST:-}" ]]; then
  echo "tapa_xilinx_integration_test: neither XILINX_HLS nor REMOTE_HOST is set; skipping" >&2
  exit 0
fi

# Pass the shared-vadd HLS opt-in through to the cargo test
# environment. Callers that have staged the full TAPA runtime +
# vendor include chain on the runner set `TAPA_SHARED_VADD_HLS=1`
# (either in their shell, in `VARS.local.bzl`, or via the
# `TAPA_SHARED_VADD_HLS` env var in CI). When it's not set we print
# a short rationale so the skip is visible in the gate output.
#
# Preflight — catch loader regressions: if `VARS.local.bzl` defined
# `TAPA_SHARED_VADD_HLS` but our import step failed to surface it
# into the environment, fail fast so reviewers don't silently see a
# "skip" line when the env file actually requested the run.
if [[ -n "$vars_local" ]]; then
  raw_shared_vadd=""
  # Read the literal RHS of `TAPA_SHARED_VADD_HLS = "..."` (if any).
  while IFS= read -r __line; do
    __line="${__line%%#*}"
    __line="${__line#"${__line%%[![:space:]]*}"}"
    __line="${__line%"${__line##*[![:space:]]}"}"
    case "$__line" in
      TAPA_SHARED_VADD_HLS[[:space:]]*=*|TAPA_SHARED_VADD_HLS=*)
        __val="${__line#*=}"
        __val="${__val#"${__val%%[![:space:]]*}"}"
        __val="${__val%"${__val##*[![:space:]]}"}"
        case "$__val" in
          \"*\") __val="${__val#\"}"; __val="${__val%\"}" ;;
          \'*\') __val="${__val#\'}"; __val="${__val%\'}" ;;
        esac
        raw_shared_vadd="$__val"
        ;;
    esac
  done < "$vars_local"
  if [[ -n "$raw_shared_vadd" && "${TAPA_SHARED_VADD_HLS:-}" != "$raw_shared_vadd" ]]; then
    echo "tapa_xilinx_integration_test: TAPA_SHARED_VADD_HLS defined in $vars_local as '$raw_shared_vadd' but env has '${TAPA_SHARED_VADD_HLS:-<unset>}' — loader regression" >&2
    exit 2
  fi
fi
if [[ "${TAPA_SHARED_VADD_HLS:-}" == "1" ]]; then
  export TAPA_SHARED_VADD_HLS=1
  echo "tapa_xilinx_integration_test: TAPA_SHARED_VADD_HLS=1 set — running shared-vadd parity" >&2
else
  echo "tapa_xilinx_integration_test: TAPA_SHARED_VADD_HLS unset; shared-vadd fixture test will skip" >&2
fi

cargo test --manifest-path "$MANIFEST" -p tapa-xilinx -- --ignored
