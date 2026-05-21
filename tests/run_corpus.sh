#!/usr/bin/env bash
# Run all corpus tests against the typst-diff binary and report results.
#
# Usage:
#   bash tests/run_corpus.sh [FLAGS]
#
# Flags:
#   --no-build          Skip cargo build (use existing binary)
#   --release           Build/use release binary
#   --only-failures     Print only failed tests
#   --verbose, -v       Print modification log for every test
#   --filter PATTERN    Run only tests whose name contains PATTERN
#   --list              Print test names and exit
#   --open              Open output directory after run (macOS)
#
# Each test lives in tests/corpus/<name>/ and must contain either:
#   old.typ + new.typ         (single-file test)
#   old/main.typ + new/main.typ  (multi-file test; included files beside main.typ)
#
# Output is written to tests/corpus_output/<name>/:
#   diff.pdf            Annotated PDF produced by the tool
#   modifications.txt   -l log from typst-diff
#   stderr.txt          Captured stderr
#   result.txt          Full summary with all artefacts inlined

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORPUS_DIR="$SCRIPT_DIR/corpus"
OUTPUT_DIR="$SCRIPT_DIR/corpus_output"
MANIFEST="$SCRIPT_DIR/../Cargo.toml"

# ── parse flags ───────────────────────────────────────────────────────────────
BUILD=1
RELEASE=0
ONLY_FAILURES=0
VERBOSE=0
FILTER=""
LIST_ONLY=0
OPEN_OUTPUT=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build)         BUILD=0 ;;
    --release)          RELEASE=1 ;;
    --only-failures|-f) ONLY_FAILURES=1 ;;
    --verbose|-v)       VERBOSE=1 ;;
    --filter|-k)        FILTER="${2:-}"; shift ;;
    --list)             LIST_ONLY=1 ;;
    --open)             OPEN_OUTPUT=1 ;;
    *) printf 'unknown flag: %s\n' "$1" >&2; exit 1 ;;
  esac
  shift
done

# ── colour support ────────────────────────────────────────────────────────────
if [[ -t 1 ]]; then
  GRN='\033[0;32m' RED='\033[0;31m' YLW='\033[0;33m'
  DIM='\033[2m'    BLD='\033[1m'    RST='\033[0m'
else
  GRN='' RED='' YLW='' DIM='' BLD='' RST=''
fi

# ── list mode ─────────────────────────────────────────────────────────────────
if [[ $LIST_ONLY -eq 1 ]]; then
  find "$CORPUS_DIR" -mindepth 1 -maxdepth 1 -type d | sort | xargs -n1 basename
  exit 0
fi

# ── build ─────────────────────────────────────────────────────────────────────
PROFILE_FLAG=""
BINARY_SUBDIR="debug"
if [[ $RELEASE -eq 1 ]]; then
  PROFILE_FLAG="--release"
  BINARY_SUBDIR="release"
fi

if [[ $BUILD -eq 1 ]]; then
  printf "${BLD}Building typst-diff${RST} (%s)...\n" "$BINARY_SUBDIR"
  if ! cargo build $PROFILE_FLAG --manifest-path "$MANIFEST" 2>&1; then
    printf "${RED}Build failed.${RST}\n" >&2
    exit 1
  fi
  echo
fi

BINARY="$SCRIPT_DIR/../target/$BINARY_SUBDIR/typst-diff"
if [[ ! -x "$BINARY" ]]; then
  printf "${RED}Binary not found: %s${RST}\n" "$BINARY" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

# ── helpers ───────────────────────────────────────────────────────────────────
# Write a horizontal rule to stdout
hr() { printf '%0.s─' {1..72}; echo; }

# Validate a PDF file: exits 0 if the file is a plausible PDF
check_pdf() {
  local f="$1"
  [[ -f "$f" ]] || { echo "no output file"; return 1; }
  local sz
  sz=$(wc -c < "$f" | tr -d ' ')
  [[ $sz -ge 1000 ]] || { echo "PDF too small (${sz} bytes)"; return 1; }
  LC_ALL=C head -c 4 "$f" | grep -q '%PDF' || { echo "output is not a PDF"; return 1; }
  return 0
}

# ── test loop ─────────────────────────────────────────────────────────────────
pass=0
fail=0
skip=0
failed_names=()
total_elapsed=0

printf "${BLD}Corpus: %s${RST}\n" "$CORPUS_DIR"
hr
echo

while IFS= read -r test_dir; do
  name=$(basename "$test_dir")

  # apply name filter
  [[ -z "$FILTER" || "$name" == *"$FILTER"* ]] || continue

  # locate entry points (single-file or multi-file)
  if [[ -f "$test_dir/old.typ" && -f "$test_dir/new.typ" ]]; then
    old_path="$test_dir/old.typ"
    new_path="$test_dir/new.typ"
  elif [[ -f "$test_dir/old/main.typ" && -f "$test_dir/new/main.typ" ]]; then
    old_path="$test_dir/old/main.typ"
    new_path="$test_dir/new/main.typ"
  else
    printf "${YLW}SKIP${RST}  %-42s  (no entry points)\n" "$name"
    skip=$((skip + 1))
    continue
  fi

  out_dir="$OUTPUT_DIR/$name"
  mkdir -p "$out_dir"
  out_pdf="$out_dir/diff.pdf"
  out_mods="$out_dir/modifications.txt"
  out_stderr="$out_dir/stderr.txt"
  out_result="$out_dir/result.txt"

  # run the tool
  t0=$SECONDS
  set +e
  "$BINARY" "$old_path" "$new_path" -o "$out_pdf" -l "$out_mods" 2>"$out_stderr"
  exit_code=$?
  set -e
  elapsed=$((SECONDS - t0))
  total_elapsed=$((total_elapsed + elapsed))

  # validate
  ok=1
  reasons=()
  pdf_validation=""

  if [[ $exit_code -ne 0 ]]; then
    ok=0
    reasons+=("exit code $exit_code")
  fi

  if pdf_validation=$(check_pdf "$out_pdf" 2>&1); then
    pdf_bytes=$(wc -c < "$out_pdf" | tr -d ' ')
    pdf_kb=$((pdf_bytes / 1024))
  else
    ok=0
    reasons+=("$pdf_validation")
    pdf_bytes=0
    pdf_kb=0
  fi

  # count modifications detected
  mod_count=0
  if [[ -s "$out_mods" ]]; then
    mod_count=$(grep -c '^## [0-9]' "$out_mods" 2>/dev/null || echo 0)
  fi

  # ── write result file ───────────────────────────────────────────────────
  {
    printf "test:           %s\n" "$name"
    printf "status:         %s\n" "$([ $ok -eq 1 ] && echo PASS || echo FAIL)"
    printf "exit_code:      %d\n" "$exit_code"
    printf "elapsed_s:      %d\n" "$elapsed"
    printf "pdf_bytes:      %d\n" "$pdf_bytes"
    printf "modifications:  %d\n" "$mod_count"
    if [[ ${#reasons[@]} -gt 0 ]]; then
      printf "failure_reason: %s\n" "${reasons[*]}"
    fi
    printf "\nold: %s\nnew: %s\n" "$old_path" "$new_path"

    if [[ -s "$out_stderr" ]]; then
      printf "\n%s\n" "$(hr)"
      printf "STDERR\n"
      printf "%s\n" "$(hr)"
      cat "$out_stderr"
    fi

    if [[ -s "$out_mods" ]]; then
      printf "\n%s\n" "$(hr)"
      printf "MODIFICATIONS LOG (%d detected)\n" "$mod_count"
      printf "%s\n" "$(hr)"
      cat "$out_mods"
    fi
  } >"$out_result"

  # ── print result ─────────────────────────────────────────────────────────
  if [[ $ok -eq 1 ]]; then
    pass=$((pass + 1))
    if [[ $ONLY_FAILURES -eq 0 ]]; then
      printf "${GRN}PASS${RST}  %-42s  ${DIM}%ds  %dKB  %d mod(s)${RST}\n" \
             "$name" "$elapsed" "$pdf_kb" "$mod_count"
    fi
    if [[ $VERBOSE -eq 1 && -s "$out_mods" ]]; then
      sed 's/^/        /' "$out_mods"
    fi
  else
    fail=$((fail + 1))
    failed_names+=("$name")
    printf "${RED}FAIL${RST}  %-42s  ${DIM}%ds${RST}\n" "$name" "$elapsed"
    for r in "${reasons[@]}"; do
      printf "      ${RED}reason:${RST} %s\n" "$r"
    done
    if [[ -s "$out_stderr" ]]; then
      printf "      ${DIM}stderr:${RST}\n"
      # show full stderr, indented
      sed 's/^/        /' "$out_stderr"
    fi
    if [[ $VERBOSE -eq 1 && -s "$out_mods" ]]; then
      printf "      ${DIM}modifications:${RST}\n"
      sed 's/^/        /' "$out_mods"
    fi
  fi

done < <(find "$CORPUS_DIR" -mindepth 1 -maxdepth 1 -type d | sort)

# ── summary ───────────────────────────────────────────────────────────────────
total=$((pass + fail + skip))
echo
hr
printf "${BLD}Results: ${GRN}%d passed${RST}${BLD}, ${RED}%d failed${RST}${BLD}, ${YLW}%d skipped${RST}${BLD} — %d tests, %ds total${RST}\n" \
       "$pass" "$fail" "$skip" "$total" "$total_elapsed"
printf "${DIM}Output: %s${RST}\n" "$OUTPUT_DIR"

if [[ ${#failed_names[@]} -gt 0 ]]; then
  echo
  printf "${BLD}Failed:${RST}\n"
  for n in "${failed_names[@]}"; do
    printf "  ${RED}✗${RST} %-42s  ${DIM}%s${RST}\n" "$n" "$OUTPUT_DIR/$n/result.txt"
  done
fi

if [[ $OPEN_OUTPUT -eq 1 ]]; then
  open "$OUTPUT_DIR" 2>/dev/null || true
fi

[[ $fail -eq 0 ]]
