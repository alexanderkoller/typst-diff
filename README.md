# typst-diff

Compares two Typst documents and produces a PDF showing word-level changes:
additions in green, deletions in red strikethrough.

Works on fully-evaluated content trees, so `#include` directives and macros are
expanded before diffing.

## Usage

```
typst-diff <OLD> <NEW> [-o diff.pdf] [-l changes.log]
typst-diff <FILE> --revision <REV> [-o diff.pdf] [-l changes.log]
```

```
Arguments:
  <OLD>   Path to the old document entry point
  <NEW>   Path to the new document entry point
  <FILE>  Working-tree entry point when comparing against Git

Options:
  -r, --revision <REV>  Compare the working-tree file against this Git revision
  -o, --output <OUTPUT>  Output PDF path [default: diff.pdf]
  -l, --log-modifications <PATH>
                          Write a text log of detected insertions, deletions,
                          and modified blocks
  -s, --compact-substitutions
                          Show substitutions as blue without red strikethrough;
                          insertions remain green, pure deletions remain red
  -h, --help             Print help
```

Both arguments are entry-point `.typ` files. If your document uses `#include`,
pass the top-level file; included files are resolved relative to it.

The Git form compares the working-tree version of `<FILE>` against `<REV>`.
Run it from anywhere inside the Git working tree. The file path must point to
the current working-tree entry point.

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

Working tree against Git:
```sh
typst-diff main.typ --revision HEAD~1 -o changes.pdf
```

Write a debugging log of the detected edits:
```sh
typst-diff main.typ --revision HEAD -o changes.pdf -l changes.log
```

Git mode assumes the command runs inside a Git working tree. It snapshots the
full tree at `<REV>` with `git archive`, so included files and assets are read
from that revision while the new document is read from the current working tree.

You can use any revision accepted by Git, such as `HEAD`, `HEAD~1`, a branch
name, a tag, or a commit hash.

## How it works

1. Both documents are evaluated by the Typst compiler, expanding all `#include`s
   and macros into a `Content` tree.
2. Show rules and counter-dependent content are realized with a layout
   introspector so the diff sees the content Typst actually typesets.
3. The trees are split into blocks (paragraphs, headings, code blocks, display
   equations, tables) and aligned with an LCS diff.
4. Adjacent changed blocks with ≥ 30% text similarity are paired for word-level
   diffing. Structured containers (lists, tables, figures, etc.) are diffed
   slot-by-slot rather than as a whole. Dissimilar blocks are marked as
   whole-block additions/deletions.
5. Consecutive deleted and inserted words that are separated only by whitespace
   are merged into a single red run and a single green run, avoiding alternating
   red–green noise when a whole sentence is replaced.
6. The annotated content is rendered to PDF using the **new** document's world,
   so fonts and assets resolve correctly.

### Colour scheme

| Change | Default | `--compact-substitutions` |
|---|---|---|
| Inserted word or block | green | green |
| Deleted word or block | red strikethrough | red strikethrough |
| Substitution — new text | green | **blue** |
| Substitution — old text | red strikethrough | *(hidden)* |

A substitution is a word deletion immediately adjacent to a word insertion within
the same block. With `--compact-substitutions` the replaced text is hidden and
the replacement is coloured blue, making diffs with many small word changes
easier to read at a glance.

## Limitations

- **Math equations** are treated as atomic tokens. Changes inside a single
  equation are shown as a whole-equation delete + insert. Deleted equations use
  Typst's `math.cancel` element instead of strikethrough.
- **Code blocks** (`raw`) are atomic blocks. Changes are shown as whole-block
  delete + insert.
- **Moved paragraphs** appear as a deletion at the original site plus an
  insertion at the new site. Move detection is not implemented.
- **Only PDF output** is supported.
- **Colours are hardcoded**: green `#00b400` and red `#dc0000`.

## Building

Requires Rust 1.85 or later (for the 2024 edition).

```sh
git clone <repo>
cd typst-diff
cargo build --release
```

The binary is at `target/release/typst-diff`. Copy it anywhere on your `PATH`.

## Installing

To install the binary into your PATH, run

```sh
cargo install --path .
```

This installs the `typst-diff` binary into Cargo's bin directory, usually
`~/.cargo/bin`. Make sure that directory is on your `PATH`.

To install from a local checkout with release optimizations and overwrite an
older local install:

```sh
cargo install --path . --force
```

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

This runs the Rust unit tests and integration tests, including fixture documents,
Git revision mode, table cell diffs, math diffs, and PDF validation.

Run the corpus suite:

```sh
tests/run_corpus.sh
```

Useful corpus flags:

```sh
tests/run_corpus.sh --list
tests/run_corpus.sh --filter 23-display-math-changed
tests/run_corpus.sh --release
tests/run_corpus.sh --verbose
```

Corpus outputs are written to `tests/corpus_output/<test-name>/`, including
`diff.pdf`, `modifications.txt`, `stderr.txt`, and `result.txt`.

Run the larger example pair:

```sh
tests/examples/run_diff.sh
```

This writes `tests/examples/diff.pdf` and
`tests/examples/modifications.txt`.
