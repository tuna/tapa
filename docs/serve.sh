#!/usr/bin/env bash
set -euo pipefail
# BUILD_WORKSPACE_DIRECTORY is set by `bazel run` to the workspace root.
BOOK_DIR="$BUILD_WORKSPACE_DIRECTORY/docs"
MDBOOK="$(find "$(dirname "$0")" -name mdbook -type f | head -1)"
MDBOOK_ADMONISH="$(find "$(dirname "$0")" -name mdbook-admonish -type f | head -1)"
# Copy binaries to a writable temp dir — the Bazel runfiles tree is read-only.
WORK_BIN="$(mktemp -d)"
cp "$MDBOOK" "$WORK_BIN/mdbook"
cp "$MDBOOK_ADMONISH" "$WORK_BIN/mdbook-admonish"
chmod +x "$WORK_BIN/mdbook" "$WORK_BIN/mdbook-admonish"
PATH="$WORK_BIN:$PATH" mdbook-admonish install "$BOOK_DIR"
exec PATH="$WORK_BIN:$PATH" "$WORK_BIN/mdbook" serve "$BOOK_DIR" --open
