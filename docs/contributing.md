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

The corpus contains numbered test pairs that each exercise a specific
scenario. Run it with:

```sh
tests/run_corpus.sh
```

Each test compiles the document pair to a PDF, renders every page to PNG at
150 dpi, and compares the result against a committed reference image.

**Status values:**

| Status | Meaning |
|--------|---------|
| `PASS` | PDF compiled; all pages match the reference within 1% fuzz |
| `FAIL` | Compile error, invalid PDF, page-count mismatch, or pixel diff |
| `NEW`  | PDF compiled but no reference image exists yet |
| `SKIP` | No entry points found (directory has no `old.typ` / `new.typ`) |

Useful flags:

```sh
tests/run_corpus.sh --list                          # list all test names
tests/run_corpus.sh --filter 23-display-math-changed  # run one test by name
tests/run_corpus.sh --filter 23                     # match by number prefix
tests/run_corpus.sh --release                       # use the release binary
tests/run_corpus.sh --verbose                       # print modification logs
tests/run_corpus.sh --only-failures                 # suppress PASS output
tests/run_corpus.sh --dpi 200                       # higher-res rendering
tests/run_corpus.sh --threshold 2%                  # looser fuzz tolerance
```

Requires `pdftoppm` (from `poppler`) and ImageMagick `magick`. Install:

```sh
brew install poppler imagemagick
```

### Managing reference images

Reference images live in `tests/corpus/<name>/ref/` and are committed to git.
When you add a new test or intentionally change rendering output, bootstrap or
re-baseline with `--update-refs`:

```sh
# Bootstrap all references from scratch (first time, or after a mass change):
tests/run_corpus.sh --release --update-refs

# Re-baseline a single test after an intentional change:
tests/run_corpus.sh --release --filter 02-single-word-substitution --update-refs
```

After `--update-refs`, every affected test should report `NEW` (refs created)
and a plain re-run should report all `PASS`.

### Outputs

Per-test outputs land in `tests/corpus_output/<test-name>/` (git-ignored):

| Path | Contents |
|------|----------|
| `diff.pdf` | The rendered diff PDF |
| `actual/page-N.png` | Per-page PNG renders of `diff.pdf` |
| `diff/page-N.png` | Visual diff PNGs (only for mismatched pages) |
| `modifications.txt` | Plain-text modification log (`-l` output) |
| `stderr.txt` | Compiler / layout diagnostics |
| `result.txt` | Full verdict with pixel-diff counts |

## Releasing a new version

typst-diff is published to [crates.io](https://crates.io/crates/typst-diff),
which is the Rust community package registry. When someone runs
`cargo install typst-diff`, Cargo downloads the crate from there. Publishing
is automated via a GitHub Actions workflow that triggers when you push a version
tag.

### One-time setup: crates.io token

The workflow authenticates to crates.io with an API token stored as a GitHub
repository secret. This only needs to be done once per crates.io account:

1. Log in to [crates.io](https://crates.io) with your GitHub account.
2. Go to **Account Settings → API Tokens** and generate a new token with
   the *Publish new crates* and *Publish updates* scopes.
3. In the GitHub repository, go to **Settings → Secrets and variables →
   Actions** and create a secret named `CARGO_REGISTRY_TOKEN` with that token
   as its value.

### How to release

1. **Bump the version** in `Cargo.toml`:
   ```toml
   [package]
   version = "0.2.0"   # was 0.1.0
   ```

2. **Commit the bump:**
   ```sh
   git add Cargo.toml
   git commit -m "release 0.2.0"
   ```

3. **Create and push a version tag.** The tag must start with `v` followed by
   the version number:
   ```sh
   git tag v0.2.0
   git push origin main
   git push origin v0.2.0
   ```
   Pushing the tag is what triggers the workflow — pushing the commit alone does
   not.

That's it. GitHub Actions takes over from here.

### What the GitHub Actions workflow does

The workflow is defined in `.github/workflows/publish.yml`. It runs on
`ubuntu-latest` and performs three steps:

1. **Checkout** — checks out the repository at the tagged commit.
2. **Install Rust** — installs the latest stable Rust toolchain via
   `dtolnay/rust-toolchain`.
3. **Test** — runs `cargo test`. If any test fails, the workflow stops and
   nothing is published.
4. **Publish** — runs `cargo publish`, which packages the crate and uploads it
   to crates.io using the `CARGO_REGISTRY_TOKEN` secret.

`cargo publish` uses the `exclude` list in `Cargo.toml` to decide what goes
into the package. Tests, docs, and markdown files are excluded so the published
crate contains only the source code needed to build the binary.

### Things to know

- **crates.io versions are immutable.** Once `0.2.0` is published you cannot
  overwrite it. If a release has a critical bug, publish a patch version
  (`0.2.1`).
- **The tag and `Cargo.toml` version should match.** The workflow does not
  enforce this, but mismatches are confusing. Tag `v0.2.0` should correspond
  to `version = "0.2.0"` in `Cargo.toml`.
- **You can do a dry run locally** before tagging to check that the package
  looks right:
  ```sh
  cargo publish --dry-run
  ```
  This runs all the packaging steps but does not upload anything.

## Example pair

A larger real-world example lives in `tests/examples/`. Run it with:

```sh
tests/examples/run_diff.sh
```

This writes `tests/examples/diff.pdf` and `tests/examples/modifications.txt`.
