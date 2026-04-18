#!/bin/bash
# Driver for the gated `tapa_cli_golden_test` sh_test.
#
# Behavior:
#   1. If `VARS.local.bzl` is present, extract `REMOTE_*` and `XILINX_*`
#      assignments and export them so the parity suite can reach the
#      remote Vitis host.
#   2. Build the Rust `tapa-cli` binary.
#   3. Run `python3 -m pytest tapa-core/tests/parity_test.py -k cli`,
#      which exercises the help/version/analyze parity tests and
#      (when toolchains are available) the byte-equal vadd flow.
#   4. Skip cleanly when neither a local Xilinx install nor a remote
#      host is configured — this target is tagged `manual`, so default
#      CI never blocks on it.

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

repo_root="$(cd "$(dirname "$MANIFEST")/.." && pwd)"

# Same VARS.local.bzl loader as run_xilinx_integration_test.sh.
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

if [[ -n "$vars_local" ]]; then
  echo "tapa_cli_golden_test: loading env from $vars_local" >&2
  while IFS= read -r line; do
    line="${line%%#*}"
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -z "$line" ]] && continue
    [[ "$line" != *=* ]] && continue
    key="${line%%=*}"
    val="${line#*=}"
    key="${key#"${key%%[![:space:]]*}"}"
    key="${key%"${key##*[![:space:]]}"}"
    val="${val#"${val%%[![:space:]]*}"}"
    val="${val%"${val##*[![:space:]]}"}"
    case "$key" in
      REMOTE_*|XILINX_*|TAPA_*) ;;
      *) continue ;;
    esac
    case "$val" in
      \"*\") val="${val#\"}"; val="${val%\"}" ;;
      \'*\') val="${val#\'}"; val="${val%\'}" ;;
    esac
    if [[ "$val" == "~/"* ]]; then
      val="${HOME}/${val:2}"
    fi
    export "$key=$val"
  done < "$vars_local"
fi

# Build the Rust binary so the parity tests can shell out to it.
cargo build --manifest-path "$MANIFEST" -p tapa-cli

# Make the built binary discoverable.
binary_path="$(dirname "$MANIFEST")/target/debug/tapa"
if [[ -x "$binary_path" ]]; then
  export TAPA_CLI_BINARY="$binary_path"
fi

# Run the native CLI parity gate. The `-k cli` filter selects every
# `test_parity_cli_*` case in `parity_test.py`, including the new
# Phase 7 gates:
#   * test_parity_cli_help_lists_same_subcommands
#   * test_parity_cli_version_matches
#   * test_parity_cli_subcommand_help_diff[<subcommand>]   (one per sub)
#   * test_parity_cli_chained_argv_corpus[<app>]           (one per fixture)
#   * test_parity_cli_analyze_vadd
#   * test_parity_cli_vadd_flow                             (full chain)
#   * test_parity_cli_unknown_first_token_fails
#   * test_parity_cli_chained_argv_value_collision_does_not_split
#
# Cases that depend on Vitis HLS / tapacc skip cleanly when the
# toolchain is missing (developer-machine friendly), but help-diff
# and chained-argv parity always run because they only need the
# parsers.
PYTHONPATH="$repo_root" exec python3 -m pytest \
  "$repo_root/tapa-core/tests/parity_test.py" -k cli -v
