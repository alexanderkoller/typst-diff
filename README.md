# typst-diff

typst-diff compares two versions of a Typst document and produces a PDF that
marks every addition in green and every deletion in red strikethrough — word
by word.

- **Works on evaluated content.** `#include` directives, user-defined functions,
  and show rules are fully expanded before diffing, so the output reflects what
  Typst actually typesets, not what the source looks like.
- **Multi-file projects.** Pass the top-level entry file; all included files
  are resolved automatically.
- **Git integration.** Compare the current working tree against any commit,
  branch, or tag without manually saving a copy of the old version.
- **Fine-grained diffs.** Lists, enumerations, tables, figures, footnotes, and
  other structured containers are diffed item-by-item, not as opaque blocks.

## Install

Requires [Rust 1.85 or later](https://rustup.rs).

```sh
cargo install typst-diff
```

## Quick start

**Compare two files:**
```sh
typst-diff old.typ new.typ
# writes diff.pdf in the current directory
```

**Multi-file project:**
```sh
typst-diff old/main.typ new/main.typ -o changes.pdf
```

**Compare working tree against a Git revision:**
```sh
typst-diff main.typ --revision HEAD~1
typst-diff main.typ --revision v1.0 -o since-v1.pdf
```

Run `typst-diff --help` for the full option list.

## Options

```
typst-diff <OLD> <NEW> [OPTIONS]
typst-diff <FILE> --revision <REV> [OPTIONS]

Arguments:
  <OLD>   Path to the old document entry point
  <NEW>   Path to the new document entry point
  <FILE>  Working-tree entry point when comparing against a Git revision

Options:
  -r, --revision <REV>          Compare the working-tree file against this Git revision
  -o, --output <PATH>           Output PDF path [default: diff.pdf]
  -l, --log-modifications <PATH>
                                Write a plain-text log of every detected insertion,
                                deletion, and modification
  -s, --compact-substitutions   Show substitutions as blue without red strikethrough
  -h, --help                    Print help
```

Git mode requires `git` and `tar` on your `PATH`. You can use any revision
accepted by Git: `HEAD`, `HEAD~1`, a branch name, a tag, or a commit hash.

## Colour scheme

| Change | Default | `--compact-substitutions` |
|--------|---------|--------------------------|
| Inserted word or block | green | green |
| Deleted word or block | red strikethrough | red strikethrough |
| Substitution — new text | green | **blue** |
| Substitution — old text | red strikethrough | *(hidden)* |

With `--compact-substitutions`, replaced text is hidden and the replacement is
blue. This reduces visual noise when many individual words change at once.

## Limitations

- **Math equations** are atomic. Changes inside an equation appear as a
  whole-expression delete + insert. Deleted equations are rendered with Typst's
  `math.cancel` mark.
- **Code blocks** are atomic. Changes appear as a whole-block delete + insert.
- **Moved paragraphs** show as a deletion at the old location plus an insertion
  at the new location.
- **PDF only.** No other output formats are supported.

## Further reading

- [docs/technical.md](docs/technical.md) — architecture, algorithms, and data
  structures
- [docs/contributing.md](docs/contributing.md) — building from source and
  running the test suite
