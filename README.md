# typst-diff

Compares two Typst documents and produces a PDF showing word-level changes:
additions in green, deletions in red strikethrough.

Works on fully-evaluated content trees, so `#include` directives and macros are
expanded before diffing.

## Usage

```
typst-diff <OLD> <NEW> [-o diff.pdf]
```

```
Arguments:
  <OLD>  Path to the old document entry point
  <NEW>  Path to the new document entry point

Options:
  -o, --output <OUTPUT>  Output PDF path [default: diff.pdf]
  -h, --help             Print help
```

Both arguments are entry-point `.typ` files. If your document uses `#include`,
pass the top-level file; included files are resolved relative to it.

### Examples

Single-file diff:
```sh
typst-diff v1/main.typ v2/main.typ -o changes.pdf
```

Multi-file project:
```sh
typst-diff old/main.typ new/main.typ
# writes diff.pdf in the current directory
```

## How it works

1. Both documents are evaluated by the Typst compiler, expanding all `#include`s
   and macros into a `Content` tree.
2. The trees are split into blocks (paragraphs, headings, code blocks, display
   equations) and aligned with an LCS diff.
3. Adjacent changed blocks with ≥ 30% text similarity are paired for word-level
   diffing; dissimilar blocks are marked as whole-block additions/deletions.
4. The annotated content is rendered to PDF using the **new** document's world,
   so fonts and assets resolve correctly.

## Building

Requires Rust 1.85 or later (for the 2024 edition).

```sh
git clone <repo>
cd typst-diff
cargo build --release
```

The binary is at `target/release/typst-diff`. Copy it anywhere on your `PATH`.

### Build profiles

**Release** (optimised, ~25 MB):
```sh
cargo build --release
```

**Debug** (fast incremental rebuilds, ~500 MB with debug symbols):
```sh
cargo build
```

The `Cargo.toml` profiles are tuned:

| Profile | `lto` | `strip` | `debug` |
|---|---|---|---|
| release | thin | yes | — |
| dev | — | — | line tables only |
| dev deps | — | — | none |

## Running tests

```sh
cargo test
```

22 tests: 20 unit tests covering each module, 2 integration tests that run the
full pipeline on fixture documents and verify valid PDF output.
