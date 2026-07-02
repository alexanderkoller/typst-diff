#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RUN_CORPUS="$SCRIPT_DIR/run_corpus.sh"
RUN_PASSING="$SCRIPT_DIR/run_passing_corpus.sh"
BASELINE="$SCRIPT_DIR/corpus-passing-baseline.txt"
BINARY="$ROOT_DIR/target/debug/typst-diff"

if [[ ! -x "$BINARY" ]]; then
  cargo build --manifest-path "$ROOT_DIR/Cargo.toml"
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

mini_corpus="$TMP_DIR/corpus"
mini_output="$TMP_DIR/output"
passing_list="$TMP_DIR/passing.txt"
mkdir -p "$mini_corpus" "$mini_output"

cp -R "$SCRIPT_DIR/corpus/01-no-change" "$mini_corpus/pass-case"
cp -R "$SCRIPT_DIR/corpus/02-single-word-substitution" "$mini_corpus/fail-case"
rm -rf "$mini_corpus/fail-case/ref"
cp -R "$SCRIPT_DIR/corpus/01-no-change/ref" "$mini_corpus/fail-case/ref"
cp -R "$SCRIPT_DIR/corpus/01-no-change" "$mini_corpus/new-case"
rm -rf "$mini_corpus/new-case/ref"
mkdir -p "$mini_corpus/skip-case"

list_output="$TMP_DIR/list.txt"
TYPST_DIFF_CORPUS_DIR="$mini_corpus" \
  bash "$RUN_CORPUS" --list >"$list_output"

grep -qx 'fail-case' "$list_output"
grep -qx 'new-case' "$list_output"
grep -qx 'pass-case' "$list_output"
grep -qx 'skip-case' "$list_output"

exact_output="$TMP_DIR/exact.txt"
TYPST_DIFF_CORPUS_DIR="$mini_corpus" \
TYPST_DIFF_CORPUS_OUTPUT_DIR="$mini_output" \
TYPST_DIFF_BINARY="$BINARY" \
  bash "$RUN_CORPUS" --exact pass-case --no-build >"$exact_output"

grep -q '^PASS  pass-case' "$exact_output"
if grep -q 'fail-case\|new-case\|skip-case' "$exact_output"; then
  printf 'exact mode ran more than the requested corpus case\n' >&2
  exit 1
fi

set +e
TYPST_DIFF_CORPUS_DIR="$mini_corpus" \
TYPST_DIFF_CORPUS_OUTPUT_DIR="$mini_output" \
TYPST_DIFF_BINARY="$BINARY" \
  bash "$RUN_CORPUS" --write-passing-list "$passing_list" --no-build >"$TMP_DIR/write.txt"
write_status=$?
set -e

if [[ $write_status -eq 0 ]]; then
  printf 'write-passing-list run unexpectedly succeeded despite FAIL/NEW/SKIP cases\n' >&2
  exit 1
fi

expected_list="$TMP_DIR/expected-passing.txt"
printf 'pass-case\n' >"$expected_list"
diff -u "$expected_list" "$passing_list"

baseline_backup="$TMP_DIR/corpus-passing-baseline.backup"
cp "$BASELINE" "$baseline_backup"
restore_baseline() {
  cp "$baseline_backup" "$BASELINE"
}
trap 'restore_baseline; rm -rf "$TMP_DIR"' EXIT

printf 'does-not-exist\n' >"$BASELINE"
set +e
TYPST_DIFF_CORPUS_DIR="$mini_corpus" \
TYPST_DIFF_CORPUS_OUTPUT_DIR="$mini_output" \
TYPST_DIFF_BINARY="$BINARY" \
  bash "$RUN_PASSING" --no-build >"$TMP_DIR/missing.txt"
missing_status=$?
set -e

if [[ $missing_status -eq 0 ]]; then
  printf 'run_passing_corpus unexpectedly accepted a nonexistent baseline entry\n' >&2
  exit 1
fi
grep -q 'does-not-exist' "$TMP_DIR/missing.txt"

restore_baseline
trap 'rm -rf "$TMP_DIR"' EXIT
