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

### Prebuilt binaries

Prebuilt binaries are attached to each
[GitHub release](https://github.com/alexanderkoller/typst-diff/releases) for
Linux, macOS, and Windows.

### With cargo-binstall

If you have Rust installed, the fastest install path is
[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall). It downloads
the matching release binary when one is available:

```sh
cargo binstall typst-diff
```

### From source

You can also build from source with Cargo. This requires
[Rust 1.85 or later](https://rustup.rs).

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
      --debug                   Write structured diagnostics next to the output PDF
      --debug-trace             Write debug diagnostics plus detailed JSONL traces
  -h, --help                    Print help
```

Git mode requires `git` and `tar` on your `PATH`. You can use any revision
accepted by Git: `HEAD`, `HEAD~1`, a branch name, a tag, or a commit hash.

## Debugging and tracing

`--debug` writes a diagnostic bundle next to the output PDF. For
`typst-diff old.typ new.typ -o changes.pdf --debug`, diagnostics are written to
`changes.debug/`.

Debug mode uses the same diff pipeline as normal mode. It only keeps
intermediate data structures long enough to serialize them after the run.

The debug bundle contains YAML snapshots:

| Path | Meaning |
|------|---------|
| `manifest.yml` | Build identity, resolved inputs, CLI flags, output paths, and trace-file metadata |
| `old/raw-eval.yml`, `new/raw-eval.yml` | Raw evaluated Typst content before normalization |
| `old/normalized.yml`, `new/normalized.yml` | Content after list/enum/terms normalization |
| `old/realized-tree.yml`, `new/realized-tree.yml` | Realized content with semantic annotations |
| `old/blocks.yml`, `new/blocks.yml` | Extracted semantic block units |
| `diff/block-raw.yml` | Raw Myers block operations |
| `diff/block-matched.yml` | Block operations after similarity pairing |
| `diff/final-edits.yml` | Final block, page-region, and rendered-region edit script |
| `diff/rendered-regions.yml` | Per-page rendered header/footer/background/foreground edits |
| `output/annotated-content.yml` | Typst content tree that will be rendered to the output PDF |
| `output/modification-log.txt` | Same text as `--log-modifications` would write |

Use `--debug-trace` when the snapshots are too coarse and you need to see
decisions. It implies `--debug` and additionally writes JSONL files:

| Path | Created when | Meaning |
|------|--------------|---------|
| `diff/pipeline-events.jsonl` | Always with `--debug-trace` | Whole-pipeline decision events: block extraction, block matching, similarity scores, owner claims, slot recursion, opaque fallback, page-region decisions, annotation grouping, and render boundaries |
| `diff/rendered-region-frame-traces.jsonl` | Only when rendered page-region frame walking runs | Low-level frame-walk events for contextual headers, footers, backgrounds, and foregrounds |

The rendered-region frame trace is intentionally scoped. It explains how text
was extracted from Typst layout frames for contextual page regions; it is not a
whole-document trace. For normal text, figures, captions, lists, equations, and
opaque content, start with `diff/pipeline-events.jsonl` and the YAML snapshots.

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
- **Code blocks** are line-diffed, not word-diffed. Changed lines appear as
  deleted old lines plus inserted new lines; typst-diff does not currently
  highlight individual token changes inside a code line.
- **Opaque visuals** such as raw graphics, SVGs, and text-empty shapes are not
  diffed word-by-word. Structural visual changes are shown as an old visual
  replacement framed in red plus a new visual replacement framed in green.
- **Document styles can override edit colours.** typst-diff represents
  insertions and deletions by adding Typst styles to the annotated content, then
  renders the result as a normal Typst document. Page styles and show rules in
  the document can still win in the final cascade, so an edit may be detected
  but not appear with the expected red/green fill. Page background content,
  styled headings, styled strong text, and styled figure captions can recolor or
  otherwise restyle the annotation output. Check `diff/final-edits.yml` or
  `output/annotated-content.yml` with `--debug` to distinguish a missed edit from
  an edit whose visual colour was overridden.
- **Context-opaque package or function output** can hide changed text from the
  semantic diff. typst-diff works on Typst's evaluated and realized `Content`
  tree; if a package macro or user-defined function returns `context`-dependent
  content whose visible body is not exposed as ordinary realized `Content`, the
  diff may see only empty `context`, `tag`, or layout artifacts and produce no
  edit. This is known to affect packages such as `@preview/showybox`, where box
  body text changes can be hidden behind `context` expansion; see the
  [technical example](docs/technical.md#context-opaque-body-content) for the
  general shape. This does not mean
  all `context` usage is unsupported: direct body `context` for state, counters,
  or references can still diff correctly when the realized text appears in the
  content tree. To diagnose this class of issue, run with `--debug` or
  `--debug-trace` and inspect `normalized.yml`, `realized-tree.yml`, and
  `final-edits.yml` for empty `plain_text`, missing slots, or
  `changed_block_count: 0`.
- **Some show-rule-generated body text is not yet diffed as authored text.**
  typst-diff expands show rules through Typst's evaluated and realized content
  pipeline, and many show-rule changes are visible there. However, some text
  created only during final layout can be absent from the realized `Content`
  tree used for semantic diffing. For example, a custom
  `#show figure.caption` rule that changes a visible caption prefix from
  `Figure:` to `Exhibit:` may typeset the new prefix, while the semantic diff
  can still see only the underlying caption body and Typst's caption object
  metadata. In those cases typst-diff does not currently have a general,
  provenance-tracked way to attribute the rendered layout text back to the
  owning semantic slot, so it may omit the show-rule-generated prefix edit or
  fall back to less precise caption-object text. Use `--debug` or
  `--debug-trace` and compare `realized-tree.yml` with the rendered output when
  diagnosing this class of issue.
- **Ambiguous footnote identity** is not guessed. If one version has a different
  number of authored footnotes in the same paragraph, typst-diff treats the
  bodies as separate inserted and deleted footnotes instead of matching them by
  textual similarity. For example, if
  `Existing note mentions baseline settings.` is the only old footnote body and
  the new paragraph contains both `New note explains calibration.` and
  `Existing note mentions revised settings.`, the footer shows the two new
  bodies as insertions and the old body as a deleted synthetic footnote. Inline
  text converted to a footnote is likewise shown as deleted inline text plus an
  inserted footnote body; the reverse is shown as inserted inline text plus a
  deleted footnote body.
- **Moved paragraphs** show as a deletion at the old location plus an insertion
  at the new location.
- **PDF only.** No other output formats are supported.

## Further reading

- [docs/technical.md](docs/technical.md) — architecture, algorithms, and data
  structures
- [docs/figure-and-opaque-diffs.md](docs/figure-and-opaque-diffs.md) —
  figure/caption and opaque visual diff behavior
- [docs/contributing.md](docs/contributing.md) — building from source and
  running the test suite
