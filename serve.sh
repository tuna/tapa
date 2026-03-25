#!/usr/bin/env bash
set -euo pipefail
# BUILD_WORKSPACE_DIRECTORY is set by `bazel run` to the workspace root.
BOOK_DIR="$BUILD_WORKSPACE_DIRECTORY"
MDBOOK="$(find "$(dirname "$0")" -name mdbook -type f | head -1)"
MDBOOK_ADMONISH="$(find "$(dirname "$0")" -name mdbook-admonish -type f | head -1)"
MDBOOK_DIR="$(dirname "$MDBOOK")"
cp "$MDBOOK_ADMONISH" "$MDBOOK_DIR/mdbook-admonish"
chmod +x "$MDBOOK_DIR/mdbook-admonish"
PATH="$MDBOOK_DIR:$PATH" "$MDBOOK" admonish install "$BOOK_DIR"
exec PATH="$MDBOOK_DIR:$PATH" "$MDBOOK" serve "$BOOK_DIR" --open
