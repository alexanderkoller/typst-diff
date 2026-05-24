#!/usr/bin/env bash
# Side-by-side comparison for a corpus test: old.typ compiled, reference diff,
# current diff output, and new.typ compiled.
#
# Usage:
#   bash tests/compare_corpus.sh <ID> [--dpi N] [--no-open]
#
# Example:
#   bash tests/compare_corpus.sh 19
#   bash tests/compare_corpus.sh 105 --dpi 200
#   bash tests/compare_corpus.sh 19 --no-open   # used by corpus_server.py
#
# Produces a single combined PNG at /tmp/compare-<NAME>.png and opens it
# (unless --no-open is given).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORPUS_DIR="$SCRIPT_DIR/corpus"
OUTPUT_DIR="$SCRIPT_DIR/corpus_output"
DPI=150
NO_OPEN=0

if [[ $# -lt 1 ]]; then
  echo "Usage: bash tests/compare_corpus.sh <ID> [--dpi N] [--no-open]" >&2
  exit 1
fi

ID="$1"
shift
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dpi) DPI="${2:-150}"; shift 2 ;;
    --no-open) NO_OPEN=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

# Find the test directory by ID prefix (e.g. 19 → 19-list-item-added).
TEST_DIR=$(find "$CORPUS_DIR" -mindepth 1 -maxdepth 1 -type d -name "$ID-*" | head -1)
if [[ -z "$TEST_DIR" ]]; then
  echo "No corpus test matches ID $ID" >&2
  exit 1
fi
NAME=$(basename "$TEST_DIR")
echo "Comparing: $NAME"

# Resolve the old.typ entry point.
if [[ -f "$TEST_DIR/old.typ" ]]; then
  OLD_TYP="$TEST_DIR/old.typ"
elif [[ -f "$TEST_DIR/old/main.typ" ]]; then
  OLD_TYP="$TEST_DIR/old/main.typ"
else
  echo "No old.typ entry point found in $TEST_DIR" >&2
  exit 1
fi

# Resolve the new.typ entry point.
if [[ -f "$TEST_DIR/new.typ" ]]; then
  NEW_TYP="$TEST_DIR/new.typ"
elif [[ -f "$TEST_DIR/new/main.typ" ]]; then
  NEW_TYP="$TEST_DIR/new/main.typ"
else
  echo "No new.typ entry point found in $TEST_DIR" >&2
  exit 1
fi

REF_PNG="$TEST_DIR/ref/page-1.png"
ACTUAL_PNG="$OUTPUT_DIR/$NAME/actual/page-1.png"

# Compile old.typ and new.typ to PDFs and convert page 1 of each to PNG.
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

if ! command -v typst &>/dev/null; then
  echo "typst CLI not found on PATH. Install with: brew install typst" >&2
  exit 1
fi

OLD_PDF="$TMP_DIR/old.pdf"
OLD_PAGE1="$TMP_DIR/old"   # pdftoppm appends "-1.png"
typst compile --root "$(dirname "$OLD_TYP")" "$OLD_TYP" "$OLD_PDF" 2>/dev/null \
  || typst compile "$OLD_TYP" "$OLD_PDF"
pdftoppm -r "$DPI" -png -f 1 -l 1 "$OLD_PDF" "$OLD_PAGE1"
OLD_PNG="$OLD_PAGE1-1.png"

NEW_PDF="$TMP_DIR/new.pdf"
NEW_PAGE1="$TMP_DIR/new"   # pdftoppm appends "-1.png"
typst compile --root "$(dirname "$NEW_TYP")" "$NEW_TYP" "$NEW_PDF" 2>/dev/null \
  || typst compile "$NEW_TYP" "$NEW_PDF"
pdftoppm -r "$DPI" -png -f 1 -l 1 "$NEW_PDF" "$NEW_PAGE1"
NEW_PNG="$NEW_PAGE1-1.png"

# Sanity check inputs.
[[ -f "$OLD_PNG"    ]] || { echo "Failed to render old.typ to PNG: $OLD_PNG" >&2; exit 1; }
[[ -f "$REF_PNG"    ]] || { echo "Missing reference PNG: $REF_PNG" >&2; }
[[ -f "$ACTUAL_PNG" ]] || { echo "Missing actual PNG:    $ACTUAL_PNG (run tests/run_corpus.sh first)" >&2; }
[[ -f "$NEW_PNG"    ]] || { echo "Failed to render new.typ to PNG: $NEW_PNG" >&2; exit 1; }

# Compose a single side-by-side PNG with labels. The Freetype "delegate library
# support not built-in" warnings are harmless — the labels still render —
# so they're redirected to /dev/null.
# Panel order: old (before) → reference diff → actual diff → new (after)
OUT="/tmp/compare-$NAME.png"
LABEL_OPTS=(-gravity North -pointsize 24 -background white -splice 0x40 -annotate +0+8)

BORDER=(-bordercolor black -border 2)

magick "$REF_PNG"    "${LABEL_OPTS[@]}" "reference (corpus/$NAME/ref)"             "${BORDER[@]}" "$TMP_DIR/a.png" 2>/dev/null
magick "$ACTUAL_PNG" "${LABEL_OPTS[@]}" "actual diff (corpus_output/$NAME/actual)" "${BORDER[@]}" "$TMP_DIR/b.png" 2>/dev/null
magick "$NEW_PNG"    "${LABEL_OPTS[@]}" "new.typ compiled directly"                "${BORDER[@]}" "$TMP_DIR/c.png" 2>/dev/null
magick "$OLD_PNG"    "${LABEL_OPTS[@]}" "old.typ compiled directly"                "${BORDER[@]}" "$TMP_DIR/d.png" 2>/dev/null

magick "$TMP_DIR/a.png" "$TMP_DIR/b.png" "$TMP_DIR/c.png" "$TMP_DIR/d.png" +append "$OUT" 2>/dev/null

echo "Wrote: $OUT"
[[ $NO_OPEN -eq 0 ]] && open "$OUT"
