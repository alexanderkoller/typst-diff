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
#   --update-refs       Copy actual PNGs to corpus/<name>/ref/ (create/update references)
#   --threshold PCT     Fuzz threshold for pixel comparison (default: 1%)
#   --dpi N             DPI for PDF-to-PNG rendering (default: 150)
#
# Each test lives in tests/corpus/<name>/ and must contain either:
#   old.typ + new.typ         (single-file test)
#   old/main.typ + new/main.typ  (multi-file test; included files beside main.typ)
#
# Output is written to tests/corpus_output/<name>/:
#   diff.pdf            Annotated PDF produced by the tool
#   actual/page-N.png   Per-page PNG renders of diff.pdf
#   diff/page-N.png     Visual diff PNGs (only for mismatched pages)
#   modifications.txt   -l log from typst-diff
#   stderr.txt          Captured stderr
#   result.txt          Full summary with all artefacts inlined
#
# References live in tests/corpus/<name>/ref/ and are committed to git.

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
UPDATE_REFS=0
THRESHOLD="1%"
DPI=150

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build)         BUILD=0 ;;
    --release)          RELEASE=1 ;;
    --only-failures|-f) ONLY_FAILURES=1 ;;
    --verbose|-v)       VERBOSE=1 ;;
    --filter|-k)        FILTER="${2:-}"; shift ;;
    --list)             LIST_ONLY=1 ;;
    --open)             OPEN_OUTPUT=1 ;;
    --update-refs)      UPDATE_REFS=1 ;;
    --threshold)        THRESHOLD="${2:-1%}"; shift ;;
    --dpi)              DPI="${2:-150}"; shift ;;
    *) printf 'unknown flag: %s\n' "$1" >&2; exit 1 ;;
  esac
  shift
done

# ── colour support ────────────────────────────────────────────────────────────
if [[ -t 1 ]]; then
  GRN='\033[0;32m' RED='\033[0;31m' YLW='\033[0;33m' BLU='\033[0;34m'
  DIM='\033[2m'    BLD='\033[1m'    RST='\033[0m'
else
  GRN='' RED='' YLW='' BLU='' DIM='' BLD='' RST=''
fi

# ── list mode ─────────────────────────────────────────────────────────────────
if [[ $LIST_ONLY -eq 1 ]]; then
  find "$CORPUS_DIR" -mindepth 1 -maxdepth 1 -type d | sort | xargs -n1 basename
  exit 0
fi

# ── tool availability ─────────────────────────────────────────────────────────
for tool in pdftoppm magick; do
  if ! command -v "$tool" &>/dev/null; then
    pkg=$([ "$tool" = "pdftoppm" ] && echo "poppler" || echo "imagemagick")
    printf "${RED}Error: %s is not installed.${RST}\n  Install with: brew install %s\n" "$tool" "$pkg" >&2
    exit 1
  fi
done

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
hr() { printf '%0.s─' {1..72}; echo; }

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
new=0
failed_names=()
new_names=()
total_elapsed=0

printf "${BLD}Corpus: %s${RST}\n" "$CORPUS_DIR"
hr
echo

while IFS= read -r test_dir; do
  name=$(basename "$test_dir")

  [[ -z "$FILTER" || "$name" == *"$FILTER"* ]] || continue

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
  actual_dir="$out_dir/actual"
  diff_dir="$out_dir/diff"
  ref_dir="$CORPUS_DIR/$name/ref"

  # run the tool
  t0=$SECONDS
  set +e
  "$BINARY" "$old_path" "$new_path" -o "$out_pdf" -l "$out_mods" 2>"$out_stderr"
  exit_code=$?
  set -e
  elapsed=$((SECONDS - t0))
  total_elapsed=$((total_elapsed + elapsed))

  # validate compile output
  ok=1
  reasons=()
  pdf_bytes=0
  pdf_kb=0

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
  fi

  mod_count=0
  if [[ -s "$out_mods" ]]; then
    mod_count=$(grep -c '^## [0-9]' "$out_mods" 2>/dev/null || echo 0)
  fi

  # ── visual comparison ──────────────────────────────────────────────────────
  visual_status=""
  actual_count=0
  ref_count=0

  if [[ $ok -eq 1 ]]; then
    mkdir -p "$actual_dir"
    pdftoppm -r "$DPI" -png "$out_pdf" "$actual_dir/page" 2>/dev/null

    actual_pages=()
    while IFS= read -r f; do actual_pages+=("$f"); done \
      < <(find "$actual_dir" -name "page-*.png" | sort)
    actual_count=${#actual_pages[@]}

    ref_pages=()
    while IFS= read -r f; do ref_pages+=("$f"); done \
      < <(find "$ref_dir" -name "page-*.png" 2>/dev/null | sort)
    ref_count=${#ref_pages[@]}

    if [[ $ref_count -eq 0 ]]; then
      visual_status="NEW"
    elif [[ $ref_count -ne $actual_count ]]; then
      visual_status="FAIL"
      ok=0
      reasons+=("page count: ref=$ref_count actual=$actual_count")
    else
      visual_status="PASS"
      mkdir -p "$diff_dir"
      for i in "${!ref_pages[@]}"; do
        page_name=$(basename "${actual_pages[$i]}")
        diff_png="$diff_dir/$page_name"
        ae=$(magick compare -metric AE -fuzz "$THRESHOLD" \
               "${ref_pages[$i]}" "${actual_pages[$i]}" "$diff_png" 2>&1 \
             | grep -oE '^[0-9]+' | head -1) || true
        ae=${ae:-error}
        if [[ "$ae" != "0" ]]; then
          visual_status="FAIL"
          ok=0
          reasons+=("page $((i+1)): ${ae} pixel(s) differ → $diff_png")
        fi
      done
    fi

    if [[ $UPDATE_REFS -eq 1 && $actual_count -gt 0 ]]; then
      mkdir -p "$ref_dir"
      cp "${actual_pages[@]}" "$ref_dir/"
    fi
  fi

  # ── write result file ──────────────────────────────────────────────────────
  {
    printf "test:              %s\n" "$name"
    printf "status:            %s\n" "$([ $ok -eq 1 ] && echo "${visual_status:-PASS}" || echo FAIL)"
    printf "exit_code:         %d\n" "$exit_code"
    printf "elapsed_s:         %d\n" "$elapsed"
    printf "pdf_bytes:         %d\n" "$pdf_bytes"
    printf "modifications:     %d\n" "$mod_count"
    if [[ -n "$visual_status" ]]; then
      printf "visual_status:     %s\n" "$visual_status"
      printf "page_count_ref:    %d\n" "$ref_count"
      printf "page_count_actual: %d\n" "$actual_count"
    fi
    if [[ ${#reasons[@]} -gt 0 ]]; then
      printf "failure_reason:    %s\n" "${reasons[*]}"
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

  # ── print result ───────────────────────────────────────────────────────────
  if [[ $ok -eq 0 ]]; then
    final_status="FAIL"
  elif [[ "$visual_status" == "NEW" ]]; then
    final_status="NEW"
  else
    final_status="PASS"
  fi

  case "$final_status" in
    PASS)
      pass=$((pass + 1))
      if [[ $ONLY_FAILURES -eq 0 ]]; then
        printf "${GRN}PASS${RST}  %-42s  ${DIM}%ds  %dKB  %d mod(s)${RST}\n" \
               "$name" "$elapsed" "$pdf_kb" "$mod_count"
      fi
      if [[ $VERBOSE -eq 1 && -s "$out_mods" ]]; then
        sed 's/^/        /' "$out_mods"
      fi
      ;;
    NEW)
      new=$((new + 1))
      new_names+=("$name")
      if [[ $ONLY_FAILURES -eq 0 ]]; then
        printf "${BLU}NEW ${RST}  %-42s  ${DIM}%ds  %dKB  %d mod(s)${RST}\n" \
               "$name" "$elapsed" "$pdf_kb" "$mod_count"
      fi
      ;;
    FAIL)
      fail=$((fail + 1))
      failed_names+=("$name")
      printf "${RED}FAIL${RST}  %-42s  ${DIM}%ds${RST}\n" "$name" "$elapsed"
      for r in "${reasons[@]}"; do
        printf "      ${RED}reason:${RST} %s\n" "$r"
      done
      if [[ -s "$out_stderr" ]]; then
        printf "      ${DIM}stderr:${RST}\n"
        sed 's/^/        /' "$out_stderr"
      fi
      if [[ $VERBOSE -eq 1 && -s "$out_mods" ]]; then
        printf "      ${DIM}modifications:${RST}\n"
        sed 's/^/        /' "$out_mods"
      fi
      ;;
  esac

done < <(find "$CORPUS_DIR" -mindepth 1 -maxdepth 1 -type d | sort)

# ── summary ───────────────────────────────────────────────────────────────────
total=$((pass + fail + new + skip))
echo
hr
printf "${BLD}Results: ${GRN}%d passed${RST}${BLD}, ${RED}%d failed${RST}${BLD}, ${BLU}%d new${RST}${BLD}, ${YLW}%d skipped${RST}${BLD} — %d tests, %ds total${RST}\n" \
       "$pass" "$fail" "$new" "$skip" "$total" "$total_elapsed"
printf "${DIM}Output: %s${RST}\n" "$OUTPUT_DIR"

if [[ $UPDATE_REFS -eq 1 ]]; then
  printf "${DIM}References updated in %s${RST}\n" "$CORPUS_DIR"
fi

if [[ ${#failed_names[@]} -gt 0 ]]; then
  echo
  printf "${BLD}Failed:${RST}\n"
  for n in "${failed_names[@]}"; do
    printf "  ${RED}✗${RST} %-42s  ${DIM}%s${RST}\n" "$n" "$OUTPUT_DIR/$n/result.txt"
  done
fi

if [[ ${#new_names[@]} -gt 0 && $UPDATE_REFS -eq 0 ]]; then
  echo
  printf "${BLD}New (no reference):${RST} run with --update-refs to create references\n"
  for n in "${new_names[@]}"; do
    printf "  ${BLU}◆${RST} %s\n" "$n"
  done
fi

if [[ $OPEN_OUTPUT -eq 1 ]]; then
  open "$OUTPUT_DIR" 2>/dev/null || true
fi

[[ $fail -eq 0 ]]
