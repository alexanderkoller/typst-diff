#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$REPO_ROOT/target/debug/typst-diff"

cargo build --manifest-path "$REPO_ROOT/Cargo.toml"

"$BINARY" \
  "$SCRIPT_DIR/old.typ" \
  "$SCRIPT_DIR/new.typ" \
  --output "$SCRIPT_DIR/diff.pdf" \
  --log-modifications "$SCRIPT_DIR/modifications.txt"

