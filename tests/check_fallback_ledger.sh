#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DECISION_RS="$ROOT_DIR/src/decision.rs"
LEDGER="$ROOT_DIR/docs/fallback-debt-ledger.md"

if [[ ! -f "$DECISION_RS" ]]; then
  printf 'missing decision code table: %s\n' "$DECISION_RS" >&2
  exit 1
fi

if [[ ! -f "$LEDGER" ]]; then
  printf 'missing fallback debt ledger: %s\n' "$LEDGER" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

codes_file="$TMP_DIR/codes.txt"
ledger_codes_file="$TMP_DIR/ledger-codes.txt"
active_without_code_file="$TMP_DIR/active-without-code.txt"

grep -o 'FB-[0-9][0-9][0-9]-[a-z0-9-]*' "$DECISION_RS" \
  | sort -u >"$codes_file"

if [[ ! -s "$codes_file" ]]; then
  printf 'no FallbackCode labels found in %s\n' "$DECISION_RS" >&2
  exit 1
fi

missing=0
while IFS= read -r code; do
  if ! grep -q "\`$code\`" "$LEDGER"; then
    printf 'ledger is missing warning code: %s\n' "$code" >&2
    missing=1
  fi
done <"$codes_file"

awk '
  /^## FB-[0-9][0-9][0-9] / {
    if (entry != "" && status == "active" && has_code == 0) print entry
    entry = $0
    status = ""
    has_code = 0
    next
  }
  /- Status: `active`/ { status = "active" }
  /- Warning code: `FB-[0-9][0-9][0-9]-[a-z0-9-]+`/ { has_code = 1 }
  END {
    if (entry != "" && status == "active" && has_code == 0) print entry
  }
' "$LEDGER" >"$active_without_code_file"

if [[ -s "$active_without_code_file" ]]; then
  printf 'active ledger entries without warning codes:\n' >&2
  sed 's/^/  /' "$active_without_code_file" >&2
  missing=1
fi

sed -n 's/.*- Warning code: `\(FB-[0-9][0-9][0-9]-[a-z0-9-]*\)`.*/\1/p' "$LEDGER" \
  | sort -u >"$ledger_codes_file"

while IFS= read -r ledger_code; do
  if ! grep -qx "$ledger_code" "$codes_file"; then
    printf 'ledger references unknown FallbackCode: %s\n' "$ledger_code" >&2
    missing=1
  fi
done <"$ledger_codes_file"

exit "$missing"
