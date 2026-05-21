# Contributing to typst-diff

## Prerequisites

Rust 1.85 or later (2024 edition). Install from [rustup.rs](https://rustup.rs)
if needed.

## Building from source

```sh
git clone <repo>
cd typst-diff
cargo build --release
```

The binary is at `target/release/typst-diff`. Copy it anywhere on your `PATH`,
or use `cargo install --path .` to put it in Cargo's bin directory
(`~/.cargo/bin`).

### Build profiles

| Profile | `lto` | `strip` | `debug` | Binary size |
|---------|-------|---------|---------|-------------|
| release | thin | yes | — | ~25 MB |
| dev | — | — | line tables only | ~500 MB |

## Running tests

```sh
cargo test
```

Runs all Rust unit tests and integration tests, including fixture documents,
Git revision mode, table and slot diffs, math diffs, and PDF output validation.

## Corpus test suite

The corpus contains 48 numbered test pairs that each exercise a specific
scenario. Run it with:

```sh
tests/run_corpus.sh
```

Useful flags:

```sh
tests/run_corpus.sh --list                          # list all test names
tests/run_corpus.sh --filter 23-display-math-changed  # run one test by name
tests/run_corpus.sh --filter 23                     # match by number prefix
tests/run_corpus.sh --release                       # use the release binary
tests/run_corpus.sh --verbose                       # print modification logs
```

Outputs land in `tests/corpus_output/<test-name>/`:

| File | Contents |
|------|----------|
| `diff.pdf` | The rendered diff |
| `modifications.txt` | Plain-text modification log |
| `stderr.txt` | Compiler/layout diagnostics |
| `result.txt` | Pass / fail verdict |

## Example pair

A larger real-world example lives in `tests/examples/`. Run it with:

```sh
tests/examples/run_diff.sh
```

This writes `tests/examples/diff.pdf` and `tests/examples/modifications.txt`.
