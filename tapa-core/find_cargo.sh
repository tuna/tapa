#!/bin/bash
# Shared helper: locate cargo from Bazel runfiles, PATH, or common install paths.
# Source this file from test scripts: source "$(dirname "$0")/find_cargo.sh"

find_cargo() {
  for base in "${RUNFILES_DIR:-}" "${TEST_SRCDIR:-}" \
              "${BASH_SOURCE[0]%/*}" "${0%.runfiles/*}.runfiles"; do
    [[ -z "$base" || ! -d "$base" ]] && continue
    while IFS= read -r -d '' candidate; do
      if [[ -x "$candidate" ]]; then
        export PATH="$(dirname "$candidate"):$PATH"
        return 0
      fi
    done < <(find "$base" -path '*/bin/cargo' -print0 2>/dev/null)
  done
  for candidate in "$HOME/.cargo/bin/cargo" "$HOME/.rustup/toolchains"/*/bin/cargo; do
    if [[ -x "$candidate" ]]; then
      export PATH="$(dirname "$candidate"):$PATH"
      return 0
    fi
  done
  return 1
}

if ! command -v cargo > /dev/null 2>&1; then
  find_cargo || true
fi
