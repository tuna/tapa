#!/bin/bash
# No-Python install smoke: prove `cargo install --path
# tapa-core/tapa-cli` yields a usable `tapa` binary on a host with
# no Python interpreter on `PATH`.
#
# Behavior:
#   1. `cargo install --path tapa-core/tapa-cli --root $STAGING` installs the
#      Rust binary into an isolated staging tree (no system changes).
#   2. Re-invoke the installed binary under a `PATH` that has no python*
#      entries. `tapa --version`, `tapa --help`, and `tapa bogus` must
#      all produce the expected responses without spawning Python.
#   3. Exits zero on success; non-zero with a diagnostic on failure.
#
# Tagged `manual` in Bazel — CI that runs this target must have a
# recent `cargo` in PATH. Local reproduction:
#
#   $ ./tapa-core/run_no_python_install_smoke.sh

set -euo pipefail

# shellcheck source=find_cargo.sh
source "$(dirname "$0")/find_cargo.sh"

MANIFEST_DIR="$(dirname "$0")/tapa-cli"
if [[ ! -f "$MANIFEST_DIR/Cargo.toml" ]]; then
  echo "tapa-cli manifest not found at $MANIFEST_DIR/Cargo.toml" >&2
  exit 1
fi

STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT

echo "no-python-smoke: installing tapa-cli into $STAGING"
cargo install --path "$MANIFEST_DIR" --root "$STAGING" --locked --offline 2>/dev/null ||
  cargo install --path "$MANIFEST_DIR" --root "$STAGING"

BIN="$STAGING/bin/tapa"
if [[ ! -x "$BIN" ]]; then
  echo "no-python-smoke: installed binary not executable at $BIN" >&2
  exit 1
fi

# Build a PATH that strips every `python*` binary. Preserve cargo's
# toolchain dir so rustc/rustup keeps working under cargo-run targets.
STRIPPED_PATH=""
IFS=':' read -r -a parts <<< "$PATH"
for dir in "${parts[@]}"; do
  if [[ -z "$dir" ]]; then
    continue
  fi
  has_python=0
  for bn in python python3 python3.14 python3.13 python3.12 python3.11 python3.10; do
    if [[ -x "$dir/$bn" ]]; then
      has_python=1
      break
    fi
  done
  if [[ "$has_python" -eq 0 ]]; then
    if [[ -z "$STRIPPED_PATH" ]]; then
      STRIPPED_PATH="$dir"
    else
      STRIPPED_PATH="$STRIPPED_PATH:$dir"
    fi
  fi
done

# Include the staging bin so `tapa` itself resolves.
STRIPPED_PATH="$STAGING/bin:$STRIPPED_PATH"

echo "no-python-smoke: sanity-check PATH has no python"
if PATH="$STRIPPED_PATH" command -v python >/dev/null 2>&1; then
  echo "no-python-smoke: python leaked through STRIPPED_PATH — aborting" >&2
  exit 2
fi
if PATH="$STRIPPED_PATH" command -v python3 >/dev/null 2>&1; then
  echo "no-python-smoke: python3 leaked through STRIPPED_PATH — aborting" >&2
  exit 2
fi

echo "no-python-smoke: tapa --version"
PATH="$STRIPPED_PATH" "$BIN" --version

echo "no-python-smoke: tapa version subcommand"
PATH="$STRIPPED_PATH" "$BIN" version

echo "no-python-smoke: tapa --help"
PATH="$STRIPPED_PATH" "$BIN" --help >/dev/null

echo "no-python-smoke: tapa bogus-subcommand must exit non-zero"
set +e
PATH="$STRIPPED_PATH" "$BIN" bogus-subcommand 2>/dev/null
rc=$?
set -e
if [[ "$rc" == "0" ]]; then
  echo "no-python-smoke: bogus-subcommand unexpectedly returned 0" >&2
  exit 3
fi

echo "no-python-smoke: PASS (tapa runs without python on PATH)"
