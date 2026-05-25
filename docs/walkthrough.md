# A Walkthrough of `typst-diff`

> *A pedagogical tour through the codebase, in the spirit of "Thinking in Rust".*
> *Read this if you already know Typst at the user level and want to understand
> what the crate actually does inside.*

The goal of `typst-diff` is to answer a deceptively simple question:

> Given two Typst documents, produce a *third* Typst document that shows what
> changed between them.

This document walks you through how that question is answered. The pipeline has
six modules and roughly five thousand lines of code, but the core ideas are
small and worth internalizing one at a time. We will build the model from the
outside in: first the *shape* of the problem, then the data Typst gives us to
work with, and finally the algorithms that turn one into the other.

---

## 1. Why isn't this just `diff -u`?

If you have ever used `latexdiff`, you know the source-level approach: take two
LaTeX files, run an LCS-style diff on their tokens, and stitch the result back
together as LaTeX with `\DIFadd{}` / `\DIFdel{}` markup. It works because LaTeX
source is *mostly* what the reader sees, modulo some macros.

Typst is more aggressive than LaTeX. Show rules can rewrite arbitrary parts of
the tree. `#include` pulls in other files. Functions can return content
computed from inputs. Counter values, references, and footnote numbers are
resolved by *layout*, not by the parser.

So source-level diffing would have a problem:

```typst
// old.typ
#let speaker = "Alice"
#speaker said hello.

// new.typ
#let speaker = "Bob"
#speaker said hello.
```

A line diff of these files would mark the `#let` line as changed. But the
reader doesn't see `#let`; they see "Alice said hello." → "Bob said hello." And
the change to the visible text isn't even *in* the file that contains the
visible text.

The conclusion is unavoidable: we have to diff what Typst *outputs*, not what
the user *wrote*. Concretely, this means evaluating both documents to Typst's
internal `Content` tree, diffing those trees, and synthesizing a third tree
that highlights the differences. That third tree is then handed back to Typst
to lay out and render to PDF.

The whole crate is structured around this idea. Here is the pipeline that
[`src/lib.rs`](../src/lib.rs) documents:

```
old.typ ──► SystemWorld ──► eval_to_realized_content ──► old: Content
new.typ ──► SystemWorld ──► eval_to_realized_content ──► new: Content
                                       │
                          diff::diff_content(old, new) ──► DiffResult
                                       │
              annotate::build_annotated_content(result) ──► Content
                                       │
               render::render_to_pdf(content, new_world) ──► Vec<u8>
```

Five stages, three different `Content` trees flowing through them. The middle
stage — `diff_content` → `DiffResult` — is where the real work happens; the
rest is plumbing to feed it Typst's idea of the document and to convert its
output back into something Typst can render.

---

## 2. The data we are diffing: `Content` trees

Before we can diff anything we need to understand what we're diffing. The
Typst compiler exposes a single sum type called `Content`. Every element you
can think of — a paragraph, a heading, a piece of text, a list — is some
specific element type wrapped into a `Content`.

A handful of element types do almost all of the work:

| Element type   | Role                                                  |
|----------------|-------------------------------------------------------|
| `TextElem`     | Leaf: a string of characters with shared styling      |
| `SpaceElem`    | Leaf: a single whitespace (Typst's representation of a space) |
| `SequenceElem` | Internal: an ordered list of children                 |
| `StyledElem`   | Internal: a child + a set of styles applied to it     |
| `ParElem`      | Block: a paragraph (collects inline content)          |
| `HeadingElem`  | Block: `=`, `==`, `===` etc.                          |
| `ListElem` /<br>`EnumElem` /<br>`TermsElem` | Block: lists and their cousins |
| `TableElem` / `GridElem` | Block: 2D content                           |
| `FigureElem`   | Block: figure with body and caption                   |
| `EquationElem` | Inline *or* block depending on `block:` field         |

A small Typst document gives a tree like this:

```typst
= Animals

The quick brown fox *jumps* over the lazy dog.
```

After evaluation it becomes (roughly, simplified for clarity):

```
SequenceElem
├── HeadingElem(level=1)
│   └── TextElem "Animals"
├── ParbreakElem
└── ParElem
    └── SequenceElem
        ├── TextElem  "The quick brown fox "
        ├── StrongElem
        │   └── TextElem "jumps"
        └── TextElem  " over the lazy dog."
```

Two structural observations will matter a great deal later:

1. **Trees are deeply nested but the *interesting* boundaries are block-level.**
   A heading is one logical "thing"; a paragraph is one logical "thing". Within
   a paragraph, the structure is mostly inline runs of text with the occasional
   styled wrapper. We will exploit this by running two different diffs: one
   over blocks, one over words within a block.

2. **Two trees that look identical to the reader can disagree at the node
   level.** A `StrongElem` wrapping a single `TextElem` and a `StrongElem`
   wrapping a `SequenceElem` of two `TextElem`s with the same total text are
   reader-equivalent but Rust-`Eq`-distinct. This is the central headache the
   algorithm has to negotiate.

---

## 3. Reading documents into memory: [`world.rs`](../src/world.rs)

Typst doesn't read files directly. The compiler is parameterized over a
`World` trait that supplies:

- The source text for a virtual file (`fn source(id: FileId) -> Source`)
- Bytes for binary assets like images (`fn file(id) -> Bytes`)
- Fonts (`fn font(index) -> Font`)
- "Today's" date (`fn today() -> Datetime`)

This indirection lets the compiler be embedded; in the web playground the
"world" is in-memory. For us it's the filesystem.

`SystemWorld::new(entry)` does three things worth noticing:

1. **Canonicalizes the entry path** and treats its parent directory as the
   document root. Every virtual `FileId` resolves relative to that root, so
   `/chapter.typ` in a `#include` means `<root>/chapter.typ`.
2. **Caches sources and binaries** behind a `Mutex<HashMap<FileId, _>>`. Once
   read, files are *never reread* — including the entry file, which is pre-loaded
   in the constructor. The unit test
   [`source_cache_returns_original_contents_after_disk_change`](../src/world.rs)
   verifies this: changing the file on disk after the world is constructed has
   no effect on what the compiler sees. That's deliberate; the compiler
   computes for many passes (we'll see why in §4) and needs a stable input.
3. **Returns `None` from `today()`.** Reproducible output: a document that
   includes `#datetime.today()` produces the same PDF whenever you diff it.

There's nothing else conceptually deep here — the module is a 200-line bridge.
But notice the asymmetry that drives the rest of the design: each document
gets *its own* world. The old document's world is thrown away after evaluation;
the new document's world is kept all the way through to PDF rendering, because
the rendered diff inherits the new document's fonts, images, and layout
settings.

---

## 4. From source to a stable tree: [`eval.rs`](../src/eval.rs)

This is the first place where the implementation diverges sharply from what a
naive reader might expect. There are actually two functions that turn a world
into a `Content`:

```rust
pub fn eval_to_content(world: &dyn World) -> Result<Content>;
pub fn eval_to_realized_content(world: &dyn World) -> Result<Content>;
```

The first wraps `typst_eval::eval` and returns a tree where show rules,
counters, and references are still *unevaluated*. The second is what the
production pipeline actually calls. The difference matters.

### The realization problem

Consider:

```typst
#show heading: it => [§ #it.body]
= Introduction
```

Evaluation produces a `HeadingElem` whose body is `TextElem("Introduction")`.
But the *reader* sees "§ Introduction". If we diff two documents and only one
of them has the show rule, the difference is invisible at the unrealized level.
Worse, two headings with different show rules but the same body would look
*identical* to a diff that operates pre-realization.

Realization is Typst's mechanism for resolving show rules into their concrete
expansions. `ROUTINES.realize` walks the tree, applies show rules, expands
counters, and produces a tree where nothing further is hidden behind a rule.

So `eval_to_realized_content` runs three substeps:

```
   eval_to_content  ──►  layout_introspector  ──►  realize_to_content
        │                       │                          │
   raw Content              Introspector              final Content
```

### What the realized tree looks like

It is genuinely worth seeing the before/after, because the difference is what
forces almost every other design decision in this crate.

Take a small document with a show rule, a counter, and a list:

```typst
#set heading(numbering: "1.")
#show heading: it => [§ #it.body]

= First
- alpha
- beta
```

**The `eval_to_content` (pre-realization) tree**, schematically:

```
SequenceElem
├── StyledElem(set heading.numbering = "1.")
├── StyledElem(show heading => …)
├── HeadingElem(level=1)
│   └── TextElem "First"
└── ListElem
    ├── ListItem
    │   └── TextElem "alpha"
    └── ListItem
        └── TextElem "beta"
```

Every structural element is still recognizable. A `HeadingElem` is a
`HeadingElem`; a `ListElem` is a `ListElem`. The show rule and the counter
configuration are still un-applied — they sit in `StyledElem` wrappers waiting
to be consulted.

**The `realize` tree**, schematically:

```
SequenceElem
├── ParElem                        ← was HeadingElem(level=1)
│   └── SequenceElem
│       ├── TextElem "§ "          ← from show rule
│       ├── TextElem "1"           ← from counter (it.numbering applied)
│       ├── TextElem " "
│       └── TextElem "First"
├── BlockElem                      ← was ListElem
│   └── GridElem
│       ├── GridCell { TextElem "•" }   ← marker computed by realization
│       ├── GridCell { ParElem(…"alpha"…) }
│       ├── GridCell { TextElem "•" }
│       └── GridCell { ParElem(…"beta"…) }
└── …
```

Three things happened. (1) The `HeadingElem` was *replaced* by what its show
rule produces — a plain paragraph with the section symbol, the resolved
counter value, and the heading body, all flattened to text. (2) The `ListElem`
was replaced by a `BlockElem` containing a `GridElem` — because lists are
*laid out* as two-column grids (markers in one column, body in the other),
and realization commits to that layout form. (3) The styling wrappers are
gone; their effects have been baked into the tree.

The realized tree is "what Typst would render". The unrealized tree is "what
the author wrote, semantically". For diffing we *want both*: the realized
tree is what's actually correct after counters/show rules, but the
unrealized tree is what lets us say "this is still a list."

### What "opaque" means

The CLAUDE.md and code comments use the word "opaque" repeatedly. Concretely
it means this: **after realization, the element type information that the
diff cares about is gone.** A `ListElem` becomes a `BlockElem(GridElem(…))`.
A `TableElem` becomes a `GridElem` of cells with stroke and padding baked in.
An `EquationElem` becomes a `BlockElem` containing math frames — raw
typesetting primitives.

That has two consequences for us:

1. **Type-driven traversal stops working.** Code like `content.is::<ListElem>()`
   returns `false` after realization, because the `ListElem` has been replaced.
   Anything keyed on "is this a list?" fails silently.

2. **Sub-positions become unaddressable.** Pre-realization a list has named
   children (`list.children[1]`); post-realization those children are
   anonymous `GridCell`s mixed in with separator cells, indented bodies, and
   other layout artifacts. The path-based slot addressing (§11) is impossible
   on the realized tree.

This is why we can't just "diff the realized trees and call it a day". The
realized trees are the right *visual* answer but the wrong *semantic*
substrate.

The strategy is therefore: realize for the parts where realization is the
authority (show rules, counters, footnote numbering), but for the structured
containers (lists, tables, equations, figures, headings) swap the
pre-realization version back in. That's what the span-preservation trick is
doing.

### The layout loop

Why `layout_introspector` in the middle? Because realization needs to know
*positional* information — page numbers for references, footnote counters,
where elements end up on the page. That information is generated by layout.
But layout itself needs an introspector to produce position-dependent values.
Circularity.

Typst's standard answer is fixed-point iteration:

```rust
for _ in 0..5 {
    let constraint = typst::comemo::Constraint::new();
    let laid_out = typst_layout::layout_document(&mut engine, content, styles)?;
    let next_introspector = laid_out.introspector.clone();
    let converged = constraint.validate(&next_introspector);
    introspector = next_introspector;
    if converged { break; }
}
```

[eval.rs:104-129](../src/eval.rs#L104-L129)

Each iteration takes the previous iteration's introspector as input. Positions
generally converge within one or two iterations (a page-reference can change a
counter, which can change a page-reference, but the cycle damps out). Five
iterations is Typst's standard cap.

### The span-preservation trick

Now for a piece of cleverness that took a careful read to appreciate. We need
to keep the pre-realization structured elements but use the realized tree as
the skeleton. The bridge between them is the `Span`.

#### What is a `Span`?

A `Span` is Typst's name for "where in the source this came from." Every
`Content` node carries one. For a plain text run like

```typst
Hello *world*.
```

the `StrongElem` around `world` carries the span `chars 7..14 in /main.typ`.
A node that doesn't come from any source — synthesized by realization, for
instance — carries `Span::detached()`.

The crucial property: **realization preserves spans.** When a show rule
expands a `HeadingElem` into a paragraph, the resulting paragraph carries the
same span as the heading it came from. When the list realizer expands a
`ListElem` into a `BlockElem(GridElem(…))`, the outer `BlockElem` carries the
list's original span. Spans are how we *recognize* in the realized tree where
each pre-realization node ended up.

#### The two-pass dance

1. **Pre-realization pass.** Walk the original `Content` tree and collect
   every `EquationElem`, `HeadingElem`, and slot-container node into a map
   keyed by their span:

   ```rust
   fn collect_preserved_by_span(content: &Content)
       -> HashMap<Span, VecDeque<Content>>
   ```
   [eval.rs:218-233](../src/eval.rs#L218-L233)

   For the example above, this captures the original `HeadingElem` (with its
   structure intact) and the original `ListElem` (with its two `ListItem`
   children).

2. **Post-realization pass.** Walk the *realized* tree. At every node, look
   up its span in the map; if there's a hit, replace the realized node with
   the preserved one:

   ```rust
   fn restore_preserved(content: Content,
                        preserved: &mut HashMap<Span, VecDeque<Content>>)
       -> Content
   ```
   [eval.rs:355-389](../src/eval.rs#L355-L389)

   So when the walk reaches the realized `ParElem` (that used to be a
   heading), it finds the span in the map and swaps in the original
   `HeadingElem`. When it reaches the realized `BlockElem(GridElem(…))` (that
   used to be a list), it swaps in the original `ListElem`. The other
   realized nodes — the section symbol "§", the resolved counter values —
   have spans that weren't in the map (they don't correspond to preserved
   nodes), so they pass through untouched.

The net effect: the structural framework (headings, lists, tables, equations)
keeps its semantic shape, while the parts that were *added* by realization
(counter values, show-rule wrappers, footnote markers) are also kept.

#### Why a queue: the same-span trap

A naive `HashMap<Span, Content>` works as long as each span occurs at most
once in the realized tree. But that's not guaranteed. Consider corpus
fixture 39:

```typst
#let framed(title, body) = block(
  stroke: (thickness: 0.8pt),
  [*#title* #sym.dash.em #body],
)

#framed("Definition 1")[A graph is a set of vertices …]
#framed("Definition 2")[A tree is a connected …]
#framed("Theorem")[Every finite connected graph …]
```

The `block(...)` expression inside `framed` lives at a single location in the
source — the body of the function. Every call to `framed` evaluates that same
expression, so all three resulting `BlockElem`s carry **the same span**.

With a plain hashmap, `collect_preserved_by_span` would `insert` three times
with the same key. The third `insert` overwrites the previous two. Then
`restore_preserved` looks up that span three times and returns the *last*
preserved value each time — meaning all three blocks in the rendered document
contain the body of the "Theorem" call.

You can see the symptom in the regression test
[`repeated_function_expansions_with_same_span_keep_their_own_content`](../tests/integration.rs#L457-L498):

```rust
assert!(new_plain.contains("Definition 1"));
assert!(new_plain.contains("Definition 2"));
assert!(new_plain.contains("Theorem"));
assert_eq!(new_plain.matches("Theorem").count(), 1);  // not 3!
```

Without the fix, the realized tree contained "Theorem" three times and lost
both definitions entirely.

The fix is the `VecDeque`. Storage becomes "for this span, here are *all* the
preserved values, in document order":

```rust
preserved.entry(content.span())
         .or_default()
         .push_back(content.clone());
```

[eval.rs:225-229](../src/eval.rs#L225-L229)

And `restore_preserved` consumes one per traversal hit:

```rust
if let Some(replacements) = preserved.get_mut(&content.span())
    && let Some(replacement) = replacements.pop_front()
{
    return replacement;
}
```

[eval.rs:361-365](../src/eval.rs#L361-L365)

Because the realized tree is walked in document order, the queue pops in
document order, and each `framed(…)` invocation gets the body that
*originated from its own call site* — even though the spans are identical.
The author of the bug fix described this commit message as "Fixed Corpus 39
(same-span bug)"; the issue was open for three weeks.

This is one of those bugs where the test name *is* the explanation. If you
want to internalize the gotcha, read
[`restore_preserved_consumes_same_span_values_in_order`](../src/eval.rs#L511-L526)
and convince yourself why a `HashMap<Span, Content>` would fail it.

The footnote handling is similar in spirit but a separate path: realization
replaces a `FootnoteElem` with a superscript number marker and moves the body
to the page footer. We preserve the original `FootnoteElem`s in document order
and re-insert them where their markers ended up
([eval.rs:241-318](../src/eval.rs#L241-L318)).

### Style accounting

The last twist in `realize_to_content` is about styles. Typst's `#set page(…)`
applies styles to a `StyledElem` wrapping a sequence. After realization the
top-level page styles need to be preserved separately from the per-block
styles. The two helper functions

```rust
fn page_styles(styles: &Styles) -> Styles      // PageElem-only
fn non_page_styles(styles: Styles) -> Styles   // everything else
```

split a `Styles` object into the two flavors. Each realized block is
re-wrapped with only its non-page styles, and the whole sequence then gets the
root page styles. We'll see why this matters in §12 (annotation), where page
boundaries become group boundaries.

By the time `eval_to_realized_content` returns, both the old and new documents
have been turned into clean `Content` trees: show rules expanded, counters
resolved, structured elements preserved by span, footnote bodies put back, and
styles bucketed for downstream use. The diff can now treat them as pure data.

---

## 5. Traversing the tree

Before we get to the diff itself, a brief interlude on the *idiom* every
module in this crate uses to walk a `Content` tree. If you read enough of the
source, you will see the same three or four patterns over and over. They are
worth naming.

### Pattern 1: `traverse` for read-only walks

`Content::traverse` is Typst's built-in pre-order walk. It takes a closure and
calls it on every node, returning a `ControlFlow` so the closure can stop the
walk early. The `<_, ()>` annotation says "don't care about the closure's
short-circuit value":

```rust
let mut footnotes = Vec::new();
let _ = content.traverse::<_, ()>(&mut |content| {
    if content.is::<FootnoteElem>() {
        footnotes.push(content);
    }
    std::ops::ControlFlow::Continue(())
});
```

[eval.rs:241-250](../src/eval.rs#L241-L250)

`traverse` is the right tool when you need to *collect* something from the
tree (footnotes, spans, blocks-by-some-predicate) but don't need to *modify*
it. It descends into every element it knows how to descend into — which is a
key caveat, because *Typst* defines that set, not us. Most structural
elements are traversed; some opaque ones are not. (For the latter we have
`extract_slots`; more on that below.)

### Pattern 2: `to_packed` / `to_packed_mut` for downcasting

`Content` is a single enum-like type. To work with a specific element you
downcast:

```rust
if let Some(par) = content.to_packed::<ParElem>() {
    // par is &Packed<ParElem>, has a .body field
}
```

The mutable form clones-on-write:

```rust
let mut content = content.clone();
if let Some(par) = content.to_packed_mut::<ParElem>() {
    par.body = replacement;
}
```

You'll see this all over annotate.rs and content_slots.rs. The pattern is
always: clone the content (or take it by value), downcast mutably, mutate the
field, return.

### Pattern 3: structural recursion

`traverse` is fine when you just want to find things. When you need to
*rewrite* a tree — apply a transformation to every node, preserving the
structure — you need explicit recursion. This is the shape:

```rust
fn restore_preserved(content: Content,
                     preserved: &mut HashMap<Span, VecDeque<Content>>)
    -> Content
{
    // Base case: this node matches a preserved span, return the preserved version.
    if let Some(repl) = preserved.get_mut(&content.span())
        .and_then(|q| q.pop_front()) {
        return repl;
    }

    // Recursive case: descend into the known wrappers.
    let mut content = content;
    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = seq.children.iter().cloned()
            .map(|c| restore_preserved(c, preserved)).collect();
    } else if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = restore_preserved(styled.child.clone(), preserved);
    } else if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = restore_preserved(par.body.clone(), preserved);
    } else {
        // Unknown element: try the slot machinery as a fallback.
        for slot in extract_slots(&content) {
            let restored = restore_preserved(slot.content, preserved);
            if let Some(next) = replace_slot(&content, &slot.path, restored) {
                content = next;
            }
        }
    }
    content
}
```

[eval.rs:355-389](../src/eval.rs#L355-L389)

Three things to notice.

**The three wrappers** — `SequenceElem`, `StyledElem`, `ParElem` — are
hardcoded as recursion sites. These are the most common element types you
encounter when walking a Typst tree: ordered sequences of siblings, styling
applied to a child, and paragraph bodies. Almost every tree you'll work with
in this crate is mostly these three glued together with leaves.

**The slot fallback.** When the current node is *none* of those three but is
still a structured container (a list, table, figure, …), we use
`extract_slots` to enumerate its text-bearing sub-positions, recurse into
each, and `replace_slot` to write the recursed value back. This is the
generic mechanism that lets the same recursive function handle every element
type without having to enumerate them all in code.

You'll see this exact pattern in [`normalize_list_item_runs`](../src/content_slots.rs#L108-L140)
and several other places. Once you recognize it, you can navigate the
codebase by pattern: anywhere you see "if SequenceElem / if StyledElem / if
ParElem / else extract_slots", you're looking at a structural rewrite.

### Pattern 4: slot enumeration

For container elements (lists, tables, figures), the right vocabulary isn't
"walk every descendant" but "tell me which named sub-positions hold user
content." That's `extract_slots`:

```rust
let slots: Vec<ContentSlot> = extract_slots(&content);
```

Each `ContentSlot` is a `(path, content)` pair. The path is a list of
`SlotStep`s that addresses the slot within its parent — `[ListItem(1),
ItemBody]` means "the body of the list's second item." We'll see slots in
detail in §11; for now the takeaway is that they exist as an alternative
*structural addressing scheme* for elements that don't fit the
"sequence/styled/par/leaf" model.

### Why the asymmetry?

You might reasonably ask: why does `traverse` work on some elements but not
others? Why do lists need `extract_slots` while sequences don't?

The answer is what we already saw in §4: lists and tables become *opaque*
after realization. Their type-level structure is gone in the realized tree.
We hand-code which elements have slots and what those slots are
([`content_slots.rs:78-100`](../src/content_slots.rs#L78-L100)) because that
knowledge is *not* recoverable from the realized form. `traverse` works on
whatever Typst's `Content` trait knows about; `extract_slots` works on what
*we* know about. They're complementary, and you need both.

---

## 6. The shape of the diff problem

Before we open `diff.rs`, take a moment to think about what a good diff would
*want* to do.

A document is a sequence of paragraphs, headings, lists, and so on. Within
each paragraph there is text. When the user edits a document they typically:

- Add, delete, or reorder paragraphs.
- Edit the text inside a paragraph.
- Edit the text inside one cell of a table.
- Occasionally restructure a section (paragraph → bullet list, etc.).

The corresponding diff operations should be:

1. **Block-level deletes and inserts** for whole-paragraph changes.
2. **Word-level deletes and inserts** *inside* a paragraph that was edited.
3. **Slot-level diffs** when a structured container (list, table, figure)
   has only one of its inner cells changed.

So we need two — really three — diff levels. The pseudocode for the top-level
algorithm is just this:

```
old_blocks, new_blocks = extract_blocks(old), extract_blocks(new)
ops = block_lcs(old_blocks, new_blocks)
ops = pair_adjacent_deletes_and_inserts_by_similarity(ops)
for each Replace(old_block, new_block) in ops:
    if structured_container_with_matching_shape:
        recursively diff each slot
    else:
        word_lcs over tokens
```

The clever stuff lives in the details — what "similarity" means, what counts
as a slot, what happens when whitespace changes — but the skeleton is just
LCS twice with a similarity-based matching step between.

---

## 7. Block extraction: [`diff.rs`](../src/diff.rs)

The first task is to turn a `Content` tree into a flat list of blocks. A
**block** for our purposes is a unit of content that:

- Stands on its own at the top level of a document (paragraph, heading,
  display equation, list, table, figure, raw block).
- Can be moved around as a unit without anything weird happening.

Inline-only structure (`TextElem`, `SpaceElem`, `StrongElem`, …) is *not* a
block; it gets collected into paragraphs.

The driver function is `extract_block_units`. The interesting recursion lives
in `collect_blocks_from_children`. Here's the idea, simplified:

```rust
for each child in children:
    if child is a block-level element (heading, raw, display eq, ...):
        flush the inline accumulator into a paragraph block
        push child as its own block
    else if child is inline (text, strong, link, ...):
        accumulate into the inline buffer
    else if child is a SequenceElem of inline things:
        flatten into the inline buffer
    else if child is a StyledElem:
        unwrap and push styles down onto whatever's inside
    else:
        unknown — flush, push as a block
finally: flush any remaining inline buffer
```

[diff.rs:141-219](../src/diff.rs#L141-L219)

The `StyledElem` handling is the part most likely to bite you the first time
you read it. Typst frequently produces shapes like:

```
StyledElem(fill=…)
└── SequenceElem
    ├── TextElem "before "
    ├── StyledElem(emph=true)
    │   └── TextElem "italic"
    └── TextElem " after"
```

— a styled wrapper around an inline-only sequence. If we naively treated the
outer `StyledElem` as block-level, we'd fragment the paragraph into three
separate blocks. There's a regression test pinning this down,
[`inline_styled_wrapper_does_not_fragment_paragraph_into_multiple_blocks`](../src/diff.rs#L1344-L1369),
which checks that a styled sequence of pure-inline content stays in *one*
paragraph block.

### Normalizing text runs

Inside a paragraph block, the children are flattened into one `ParElem` whose
body has gone through `normalize_text_runs`. This function coalesces
consecutive `TextElem` + `SpaceElem` nodes into single `TextElem` strings.

Why? Because we're about to hash blocks for equality. Two paragraphs that are
visually identical can have wildly different internal shapes depending on how
Typst tokenized them. By coalescing into a canonical form, structurally-equal
paragraphs hash identically and the LCS gets to treat them as `Equal`.

### Sticky page styles

The final step of `extract_block_units` is:

```rust
make_page_styles_sticky(&mut blocks);
```

[diff.rs:71-79](../src/diff.rs#L71-L79)

Imagine a document that starts with `#set page(margin: 1in)` then later
inserts a single landscape page with `#set page(flipped: true)` followed by
more content. Only the *first* paragraph after each `#set page` carries the
page-style update; subsequent siblings inherit it implicitly. We make this
explicit on every block: each `DiffBlock` knows the page styles in effect at
its position, so we can group the annotated output back into the right page
regions later without losing the boundary information.

---

## 8. Block-level LCS

We now have two `Vec<DiffBlock>` to compare. The natural tool is the longest
common subsequence — given two sequences, find the longest sequence of items
that appears in both, in order. Everything not in the LCS is either an
insertion or a deletion.

We use the `similar` crate's Myers algorithm. There's one detail to handle:
the `Content` type implements `PartialEq` and `Hash` but not `Eq + Ord`. The
`similar` API needs all of those. The fix is a newtype:

```rust
struct HashableContent(Content);
impl Eq for HashableContent {}
impl Ord for HashableContent { /* by plain_text, then by hash */ }
```

[diff.rs:464-497](../src/diff.rs#L464-L497)

The trick in `Ord`: compare on plain text first, then use the structural hash
as a tiebreaker. This is consistent with `Eq` because two `Content` values
that are structurally equal will have the same hash and therefore tie; the
tiebreaker preserves the `Ord`/`Eq` contract without requiring us to define a
"correct" semantic ordering on content (which doesn't really exist).

The output of `diff_block_units_raw` is a `Vec<BlockOp>`:

```rust
enum BlockOp {
    Equal(DiffBlock, DiffBlock),
    Delete(DiffBlock),
    Insert(DiffBlock),
    Replace(DiffBlock, DiffBlock),  // not yet produced at this stage
}
```

Notice that `Replace` is in the enum but not yet emitted; we'll produce it in
the next step. Why two `DiffBlock`s in `Equal`? Sometimes a block is the same
text but with different surrounding context, and we want to know about both.
The current pipeline always renders the new version, but the data structure
remembers both.

---

## 9. Edit-zone matching: turning `Delete + Insert` into `Replace`

Myers gives us a sequence like:

```
Equal("Introduction")
Equal("The quick brown fox jumps over the lazy dog.")
Equal("Dogs sleep a lot.")
```

That's the easy case. Now consider what happens when the user changes one
word inside the second sentence: "jumps" → "leaps". The fox sentence is no
longer hash-equal between old and new, so Myers reports:

```
Equal("Introduction")
Delete("The quick brown fox jumps over the lazy dog.")
Insert("The quick brown fox leaps over the lazy dog.")
Equal("Dogs sleep a lot.")
```

If we annotate this literally — strikethrough the whole old sentence, green
the whole new sentence — the reader sees an enormous edit when really *one
word* changed. Bad UX.

So we need a step that says: "those two adjacent ops look like a replacement;
let me match them up so the *next* phase can word-diff them". That's
`match_edit_zones`:

```rust
fn match_edit_zones(ops: Vec<BlockOp>) -> Vec<BlockOp>;
```

[diff.rs:599-635](../src/diff.rs#L599-L635)

The algorithm scans the ops, identifies contiguous runs of `Delete` and
`Insert` (a "zone"), and within each zone greedily pairs each delete with the
most-similar unused insert. If the best similarity is at least 0.3 (a tuned
constant), they pair up as a `Replace`. Anything left unpaired stays as a
plain `Delete` or `Insert`.

### Picking a similarity metric

"Similarity" here is `[0, 1]` — 0 means "completely different", 1 means
"identical". The function in [diff.rs:696-714](../src/diff.rs#L696-L714) is:

```rust
fn similarity(a: &str, b: &str) -> f64 {
    let max_len = a.chars().count().max(b.chars().count());
    if max_len > 2_000 {
        return word_overlap_similarity(a, b);
    }
    let max_distance = ((1.0 - 0.3) * max_len as f64).floor() as usize;
    let distance = edit_distance_with_limit(a, b, max_distance)?;
    1.0 - distance as f64 / max_len as f64
}
```

For short strings it's normalized Levenshtein distance. The
`edit_distance_with_limit` variant bails early if the distance ever exceeds
`max_distance` — saving a lot of work when comparing two paragraphs that are
clearly unrelated. The "row min" trick is the standard one: if every cell in
the current DP row is already over the limit, we can never get back under it,
so abort.

For long strings (over 2,000 characters), Levenshtein gets too expensive
(O(n·m)). The fallback is Sørensen–Dice on words: build a bag of words for
each string, count overlaps. That's O(n+m) and gives a reasonable estimate.

Why 0.3 as the threshold? It's a heuristic. Below 0.3 the strings are usually
genuinely different paragraphs that just happened to land adjacent in the
edit zone. Empirically this catches most "I rewrote a sentence" cases while
not over-pairing unrelated content.

---

## 10. Word-level diff inside a Replace

For each `Replace(old_block, new_block)` we now need to compute the per-word
diff. The output is `Vec<WordOp>`, with `WordOp::Equal | Delete | Insert`
each carrying a list of `Token`s.

A `Token` is a small struct:

```rust
struct Token {
    text: String,
    content: Content,
}
```

[diff.rs:340-365](../src/diff.rs#L340-L365)

Notice the trick: equality and hashing are by `text` *only*, but the
`content` field carries the original `Content` node. So Myers can find common
subsequences based on visible text (where two tokens are "the same" if their
strings match), but when we go to render unchanged tokens we still have the
original styled content available to reproduce faithfully. Style changes
within a word are absorbed by the equality-by-text rule and don't generate
spurious diff operations.

### Tokenization

`extract_words` walks a block and produces a flat token list:

- `TextElem` and `SpaceElem` are split on whitespace boundaries
  (`collect_text_tokens`).
- `EquationElem` becomes a single token whose text is the equation's `repr`.
- Slot-container nodes (lists, figures, …) recurse into their slot bodies.
- Anything else becomes a single atomic token — unless its plain text is over
  500 chars, in which case it's word-split (so a giant `StrongElem` doesn't
  become one giant unsplittable token).

The tokenizer carefully preserves styling: if a `TextElem` is wrapped in a
`StyledElem(red)`, the produced tokens each carry styled `Content` so that
when they're emitted unchanged in the output they keep being red.

### Whitespace and substitution zones

Now for a subtle bit. Run Myers on `["The", " ", "fox", " ", "jumps", "."]`
vs `["The", " ", "fox", " ", "leaps", "."]` and you get:

```
Equal("The")
Equal(" ")
Equal("fox")
Equal(" ")
Delete("jumps")
Insert("leaps")
Equal(".")
```

Fine. But if you change `"jumps."` → `"leaps over"`, you might get something
uglier:

```
Equal("The"), Equal(" "), Equal("fox"), Equal(" "),
Delete("jumps"), Delete("."), Insert("leaps"), Insert(" "), Insert("over")
```

— or with whitespace interleaved between the deletes and inserts, which is
visually ugly when annotated. The function `merge_substitution_zones`
[diff.rs:874-901](../src/diff.rs#L874-L901) absorbs whitespace-only `Equal`
ops that are adjacent to or sandwiched between deletes/inserts, distributing
them onto both sides. The result is a clean "one Delete run followed by one
Insert run" pattern that's easy to annotate.

### Style-only changes

After producing word ops, we check `has_textual_word_change`:

```rust
fn has_textual_word_change(word_ops: &[WordOp]) -> bool;
```

[diff.rs:793-800](../src/diff.rs#L793-L800)

If every `Delete`/`Insert` op contains only whitespace tokens, the change is
purely stylistic — the block was put through the LCS as different because of
some style difference, but the visible text didn't change. We downgrade the
`Modified` back to `Equal` so the user doesn't see spurious annotations.

---

## 11. Slots: diffing inside containers

The simplest case so far has been editing a paragraph. But what if the change
is *inside* a table cell, or *inside* a list item?

Consider:

```typst
- Apples
- Banannas    ← typo, fixed in new
- Cherries
```

If we treat the whole list as one block and word-diff it, the result is
correct but visually noisy — we'd strikethrough "Banannas" and underline
"Bananas" but the surrounding bullets get re-emitted, possibly with
formatting drift.

The better answer: recognize that the list has the same *shape* in both
versions (three items, in this order) and only the second item's body
changed. Recurse into that item's body and diff *just that*. Leave the rest
of the list structurally untouched.

That's what slots are. A **slot** is a named, addressable text-bearing
position inside a structured element. The module
[`content_slots.rs`](../src/content_slots.rs) defines the vocabulary:

```rust
enum SlotStep {
    SequenceChild(usize),
    StyledChild,
    ParBody,
    ItemBody,
    ListItem(usize),
    EnumItem(usize),
    Term(usize), TermDescription(usize),
    FigureBody, FigureCaption,
    FootnoteBody, QuoteBody,
    WrapperBody,
    TableCell(usize), GridCell(usize), StackChild(usize),
}
```

A path like `[ListItem(1), ItemBody]` uniquely identifies "the second item's
body within this list". `extract_slots(content)` walks a content tree and
returns every such path together with the current content at that position.
`replace_slot(template, path, replacement)` writes a new value at a path
inside a clone of the template.

### Slot-based diffing

Inside `diff_content`, when a `Replace(old, new)` pair is encountered we ask:

```rust
fn diff_slots(old: &Content, new: &Content) -> Option<Vec<SlotDiff>>;
```

[diff.rs:1148-1171](../src/diff.rs#L1148-L1171)

Returns `Some(slot_diffs)` iff:

1. Both blocks have the same number of slots, *and*
2. Each slot's path is the same in both (the shape matches), *and*
3. At least one slot's contents differ.

When the shapes match, we recursively call `diff_content` on each
old-slot/new-slot pair. Recursion is real — a list item that contains a
nested list will diff down through both levels, producing a `SlotDiff`
whose `ops` themselves contain `ModifiedSlots`. The regression test
[`diff_slots_recurses_into_nested_list_item_body`](../src/diff.rs#L1848-L1893)
exercises this exact case.

When the shapes don't match (a list with 3 items vs one with 2), we fall back
to flat word-level diffing on the block as a whole.

The final `DiffResultOp` enum encodes all five outcomes:

```rust
enum DiffResultOp {
    Equal(DiffBlock),
    Deleted(DiffBlock),
    Inserted(DiffBlock),
    Modified(DiffBlock, Vec<WordOp>),
    ModifiedSlots(DiffBlock, Vec<SlotDiff>),
}
```

---

## 12. Annotation: from `DiffResult` to renderable `Content`

We now have a `DiffResult`. We need to turn it back into a `Content` tree
that Typst can render to a PDF — only colored.

[`annotate.rs`](../src/annotate.rs) does this. The color conventions:

| Change type        | Visual                                        |
|--------------------|-----------------------------------------------|
| Inserted block     | green text fill                               |
| Deleted block      | red strikethrough on plain text               |
| Deleted equation   | red `CancelElem` instead of strikethrough     |
| Modified — insert  | green (or blue, in `--compact-substitutions`) |
| Modified — delete  | red strikethrough                             |

Most of the code is small Content-manipulation functions, but two ideas are
worth explaining.

### Grafting into the innermost text container

When we have a `Modified(new_block, word_ops)`, we want to:

1. Build a flat inline `Content` sequence from the word ops, with colors.
2. Put that inline sequence back where the original block's text was, but
   keep the *outer* block structure (heading level, paragraph alignment,
   etc.).

The function that does (2) is `replace_text_container`
([annotate.rs:258-296](../src/annotate.rs#L258-L296)):

```rust
fn replace_text_container(template: &Content, replacement: &Content)
    -> Option<Content>
```

It descends through `ParElem`, `HeadingElem`, `StyledElem`, and inline-only
`SequenceElem` wrappers until it finds the innermost text-bearing position,
then writes the replacement there. The result preserves every layer of outer
styling.

This is important for headings in particular. If a `=== Subsection` is
modified, we want to keep it rendering as `=== Subsection`, not as a plain
red+green annotated paragraph. The regression test
[`modified_heading_preserves_heading_element`](../src/annotate.rs#L489-L509)
pins this behavior down.

### Why deleted blocks are flattened

Insertions get green text fills applied "inside" the structure: a list with a
green text fill applied to each item's body. The list element itself stays
intact.

But deletions are different. A deleted heading is *flattened*:

```rust
DiffResultOp::Deleted(c) => {
    let colored = plain_content(&c.content).styled(TextElem::fill.set(red().into()));
    let struck = Content::new(StrikeElem::new(colored));
    DiffBlock {
        content: replace_text_container(&c.content, &struck).unwrap_or(struck),
        ...
    }
}
```

[annotate.rs:89-96](../src/annotate.rs#L89-L96)

Why flatten? Because if we kept a deleted `HeadingElem` *as a heading*, it
would re-trigger heading counters, get a TOC entry, etc. — phantom side
effects that the reader of the diff didn't ask for. By collapsing to plain
text we preserve the *appearance* (still inside the heading container if it
had one) without the document-level side effects.

### Equations: cancel instead of strikethrough

A deleted equation can't easily be wrapped in a strikethrough — math layout
is different from text layout. So for deleted equations we use Typst's
`CancelElem`, which draws a diagonal line through the math:

```rust
let cancelled = Content::new(
    CancelElem::new(body).with_stroke(Stroke::from_pair(red(), Abs::pt(0.6).into())),
);
```

[annotate.rs:236-241](../src/annotate.rs#L236-L241)

### Page-style grouping

Recall that `extract_block_units` made page styles sticky on every block.
Now we use that. `build_annotated_content` walks the annotated blocks and
groups consecutive blocks that share the same `page_styles` into a single
sequence wrapped in a single `styled_with_map(styles)`:

```rust
for block in build_annotated_blocks(&result.block_ops, compact_substitutions) {
    if current_page_styles.as_ref().is_some_and(|s| s != &block.page_styles) {
        flush_group(&mut groups, &mut current_blocks, current_page_styles.take());
    }
    current_page_styles.get_or_insert_with(|| block.page_styles.clone());
    current_blocks.push(block.content);
}
flush_group(...);
```

[annotate.rs:46-64](../src/annotate.rs#L46-L64)

This preserves `#set page(...)` regions across section breaks — different
margins, landscape pages, custom headers/footers, etc. — even after the
diff has restructured the content.

### Compact substitution mode

The CLI flag `-s` / `--compact-substitutions` changes the colors slightly:
when a delete is *immediately followed by* an insert (the common "I changed
this word to that word" case), the deletion is omitted entirely and the
insertion is colored blue instead of green. The reader sees the new text but
not the old. Less noisy when the user just wants to know *what it says now*.

---

## 13. Rendering: [`render.rs`](../src/render.rs)

The last stage is small. `render_to_pdf` takes the annotated content and the
new world, and produces PDF bytes.

The only non-trivial bit is the same convergence loop we saw in `eval.rs`:
layout, check the introspector against its constraint, re-layout until
stable, up to 5 iterations. Then `typst_pdf::pdf` to encode.

```rust
for _ in 0..5 {
    let constraint = typst::comemo::Constraint::new();
    let laid_out = typst_layout::layout_document(&mut engine, content, styles)?;
    let next_introspector = laid_out.introspector.clone();
    let converged = constraint.validate(&next_introspector);
    document = Some(laid_out);
    introspector = next_introspector;
    if converged { break; }
}
```

[render.rs:32-58](../src/render.rs#L32-L58)

Tagged PDF is disabled because the annotation markup doesn't carry
accessible metadata — there's no good way to express "this word is a diff
deletion" in the tagged-PDF tree, so we don't pretend.

---

## 14. A worked example, end to end

Let's trace what happens for the simplest non-trivial diff. The fixture
[`tests/corpus/02-single-word-substitution`](../tests/corpus/02-single-word-substitution)
contains:

```typst
// old.typ
The quick brown fox jumps over the lazy dog.

// new.typ
The quick brown fox leaps over the lazy dog.
```

**Stage 1: world + eval.** Two `SystemWorld`s, two evaluations. Each
produces a `Content` tree of approximately the shape:

```
SequenceElem
└── ParElem
    └── TextElem "The quick brown fox jumps over the lazy dog."
```

(Plus some styling wrappers we'll ignore.)

**Stage 2: block extraction.** Each tree extracts to a single
`DiffBlock` containing the `ParElem`. So `old_blocks.len() == 1` and
`new_blocks.len() == 1`.

**Stage 3: block-level LCS.** The two paragraphs are not structurally equal
(different `TextElem` content), so Myers reports:

```
Delete(old_block)
Insert(new_block)
```

**Stage 4: edit-zone matching.** A contiguous Delete+Insert zone with one of
each. `similarity("The quick brown fox jumps...", "The quick brown fox leaps...")`
is well above 0.3 (most of the text is identical). They pair up:

```
Replace(old_block, new_block)
```

**Stage 5: slot diff?** The blocks are bare paragraphs, no slot containers.
`diff_slots` returns `None`. Fall through to word diff.

**Stage 6: word diff.** Tokens:

```
old: ["The", " ", "quick", " ", "brown", " ", "fox", " ", "jumps", " ", "over", ...]
new: ["The", " ", "quick", " ", "brown", " ", "fox", " ", "leaps", " ", "over", ...]
```

Myers produces:

```
Equal(["The", " ", "quick", " ", "brown", " ", "fox", " "])
Delete(["jumps"])
Insert(["leaps"])
Equal([" ", "over", " ", "the", " ", "lazy", " ", "dog."])
```

This becomes a `Modified(new_block, word_ops)`.

**Stage 7: annotation.** `annotated_inline_content` walks the word ops:

```
TextElem("The quick brown fox ")
StrikeElem(TextElem("jumps", fill=red))
TextElem("leaps", fill=green)
TextElem(" over the lazy dog.")
```

This is the new inline body. `replace_text_container` grafts it into the
original paragraph structure. The result is one `ParElem` whose body shows
the strike + green substitution.

**Stage 8: render.** Layout + PDF export. The result is a one-page PDF
showing the sentence with "jumps" struck out in red and "leaps" in green.

Total turnaround: two evaluations, one tree comparison, one layout pass.

---

## 15. Anatomy of content-loss bugs

If you spend any time hacking on this codebase, you will eventually break it
in a particularly disturbing way: text that *should* appear in the diff
output silently disappears. The annotated PDF compiles fine. It just doesn't
contain a paragraph that the source clearly contained. No error, no warning,
no log message — the words are simply gone.

This class of bug is unusually common here, for a structural reason. The
crate is essentially three coupled tree rewrites — eval, diff, annotate — and
every rewrite has the form "take a tree, produce a (mostly) similar tree".
Whenever the rewrite has a case it doesn't recognize, the *easiest* way to
fail is by returning an empty tree, an incorrectly-flattened tree, or a tree
where one branch was overwritten by another. None of those failures are
visible until you compare against the source.

The bugs below are all real; each has a regression test, and most have a
matching commit message. Read them as a guided tour of the failure modes.

### Bug 1: the same-span collapse

We already met this one in §4: when a function body is expanded N times,
every realized invocation shares the same `Span`. If the preservation map is
`HashMap<Span, Content>` instead of `HashMap<Span, VecDeque<Content>>`, the
last preserved value wins, and N−1 invocations get *all of their content
replaced by the body of the last one*.

```typst
#let framed(title, body) = block[*#title* — #body]

#framed("Definition 1")[A graph is a set of vertices.]
#framed("Definition 2")[A tree is a connected acyclic graph.]
#framed("Theorem")[Every finite connected graph has a spanning tree.]
```

Symptoms: the rendered document contained "Theorem" three times and none of
the definitions. The diff log was nonsense ("vertices → spanning tree").

Fix: queue per span, consumed in document order
([`eval.rs:218-233`](../src/eval.rs#L218-L233)).

Test: [`repeated_function_expansions_with_same_span_keep_their_own_content`](../tests/integration.rs#L457-L498).

### Bug 2: the inline-styled fragmentation

This one comes from `extract_block_units`. Typst's realization often produces
shapes like:

```
StyledElem(some-inline-style)
└── SequenceElem
    ├── TextElem "The species is known as "
    ├── TextElem "Felis domesticus"
    │       (wrapped in StyledElem with emphasis style)
    └── TextElem " in older literature."
```

The outer `StyledElem` wraps a `SequenceElem` whose children are all inline
elements. To the reader this is just one paragraph; to a naive block
extractor it could *look like* a block-level wrapper that should be peeled
open and have its children processed individually.

If we peel it open and call `extract_block_units` on each child, each child
flushes the inline accumulator into its own paragraph. The result: three
separate paragraph blocks instead of one. The diff then sees three blocks on
the new side and one paragraph on the old side, can't match them up, and
either fragments the diff into nonsense or loses chunks of text when the
slot machinery later recurses into the wrong `ParBody`.

Symptom (from the regression test): one paragraph of text gets split into
three blocks, the diff hallucinates structure that isn't there, and pieces
of text end up missing from the annotated output.

Fix: detect the special case "styled wrapper around an inline-only sequence"
and treat the whole thing as a single paragraph block. The check is
`is_inline_sequence` ([`diff.rs:321-323`](../src/diff.rs#L321-L323)):

```rust
if let Some(seq) = styled.child.to_packed::<SequenceElem>() {
    if is_inline_sequence(seq) {
        return vec![DiffBlock { content: paragraph_block(...), ... }];
    }
    // otherwise fall through to block-level processing
}
```

Tests:
[`inline_styled_wrapper_does_not_fragment_paragraph_into_multiple_blocks`](../src/diff.rs#L1344-L1369)
and
[`diff_content_on_paragraph_with_inline_styling_produces_single_modified_op`](../src/diff.rs#L1371-L1425).
The second test's panic message reads: *"expected Modified, got ModifiedSlots
— fragmentation bug regression"*. It exists because someone *did* regress it.

### Bug 3: the ParBody infinite recursion (and its less-bad alternative)

When `extract_slots` was first written, it pushed a slot for *every*
paragraph body. The slot path was `[ParBody]`, the slot content was the
paragraph's body content.

This caused a problem with `diff_slots`. Recall its logic: if both old and
new blocks have matching slot shapes, recurse into each slot via
`diff_content`. So for a plain paragraph:

```
ParElem
└── body: TextElem "The fox jumps."
```

`extract_slots` returns one slot `[ParBody → TextElem("The fox jumps.")]`.
`diff_slots` then calls `diff_content` on the slot body. `diff_content` calls
`extract_block_units` on the body, which wraps the inline text in a fresh
`ParElem`, which has its own `[ParBody]` slot, which calls `diff_content`,
which… you see where this is going. Infinite recursion.

The naive fix is to suppress the `ParBody` slot when the body is inline-only.
But the test
[`mixed_body_inline_change_detected_and_nested_structure_preserved`](../src/annotate.rs#L628-L662)
shows the case that survives: a paragraph body that contains *both* inline
text *and* a nested list. We still want to recurse to find the nested list,
even though we don't want to recurse infinitely on the inline part.

The actual fix in [`content_slots.rs:200-214`](../src/content_slots.rs#L200-L214)
is subtler: when we see a `ParElem`, descend into its body *looking for
sub-slots* (lists, tables, footnotes nested inside the paragraph), but do
*not* push a fallback `ParBody` slot that points at the inline body itself.
If the body has no sub-slots, `extract_slots` returns nothing for the
paragraph and `diff_slots` correctly returns `None`, falling through to flat
word-level diffing.

The comment in the code is unusually long for this codebase, which tells you
how easy it is to get wrong:

> Only descend into the body to surface sub-slots (e.g. a nested list inside
> the paragraph). Do NOT push a fallback "ParBody" slot for inline-only
> bodies — if we did, `diff_slots` would recurse via `diff_content` into the
> body, which `extract_block_units` would wrap in a fresh `ParElem` whose
> own `ParBody` slot is the same body content, causing infinite recursion.

When this was wrong, the symptom wasn't always a stack overflow — sometimes
the recursion bottomed out somewhere unexpected and returned an empty diff,
quietly losing the entire paragraph.

### Bug 4: the tight-list spacing trap

A different kind of content "loss" — the *visual* kind. The fix is in
[`annotate.rs:311-324`](../src/annotate.rs#L311-L324) (`apply_fill_inside`).

Naive insertion handling looked like this:

```rust
DiffResultOp::Inserted(c) => {
    let styled = c.content.clone()
        .styled(TextElem::fill.set(green().into()));
    DiffBlock { content: styled, ... }
}
```

Wrap the inserted content in a `StyledElem` with a green fill. Simple,
correct… for plain text. But Typst's layout engine has a special case for
*consecutive bare list blocks*: it tightens the spacing between them. Two
adjacent `ListElem`s render as one continuous list, with bullet-tight
spacing. A `StyledElem(ListElem)` next to a `ListElem` does *not* trigger
the tight-spacing case, because the outer element is now a generic styled
block, not a bare list.

So inserting a new list item between two existing items would render with
paragraph-sized gaps above and below the inserted item. The text was all
there, but it looked like an entirely different document structure had been
introduced.

The fix: for slot-bearing elements (lists, tables, figures), don't wrap the
outer element — apply the fill to each slot's content individually:

```rust
fn apply_fill_inside(content: &Content, fill: Color) -> Content {
    let slots = extract_slots(content);
    if slots.is_empty() {
        return content.clone().styled(TextElem::fill.set(fill.into()));
    }
    let mut result = content.clone();
    for slot in slots {
        let colored = slot.content.styled(TextElem::fill.set(fill.into()));
        if let Some(next) = replace_slot(&result, &slot.path, colored) {
            result = next;
        }
    }
    result
}
```

The outer `ListElem` stays bare; the green coloring goes onto each item's
body. Adjacent inserted lists collapse together visually, as expected.

Tests:
[`inserted_list_block_stays_bare_list_not_styled_wrapper`](../src/annotate.rs#L387-L407)
and
[`inserted_parbreak_is_not_wrapped_in_styled_elem`](../src/annotate.rs#L679-L698).
The second test's name and comment reference "corpus #19 bug" — the spacing
fix needed to extend to parbreaks too, because wrapping an empty-text
`ParbreakElem` in a `StyledElem` also broke list spacing.

### Common shape

All four bugs have the same shape:

1. We are *rewriting* a tree.
2. There is a case the rewriter doesn't recognize (a specific shape, a
   repeated span, a slot type it doesn't know about, an element whose
   block/inline status is ambiguous).
3. The rewriter produces a tree that's *valid* (compiles, renders, looks
   like a document) but *wrong* (missing content, fragmented structure,
   wrong spacing).
4. There's no error path. The tree just doesn't contain what it should.

The defense is regression tests with concrete `plain_text().contains(…)`
assertions. If you change anything in `eval.rs`, `diff.rs`, or `annotate.rs`,
run the full test suite and *eyeball the corpus output* (`tests/run_corpus.sh
--verbose`). A typecheck-passing PR is not enough; a tests-passing PR is
*usually* enough; a corpus-visually-inspected PR is what actually ships.

---

## 16. Limitations worth knowing about

The architecture has a few principled limitations:

- **Moves look like delete + insert.** If you move a paragraph from page 3 to
  page 7, the diff shows it deleted at page 3 and inserted at page 7. There's
  no notion of "this content moved." Detecting moves would require pairing
  *non-adjacent* deletes and inserts by similarity, which is a different
  algorithm than what `match_edit_zones` does today.

- **Equations are atomic at the expression level.** If an equation changes
  internally, the whole old equation gets cancelled and the whole new
  equation gets emitted in green. There's no sub-expression diff. This is
  partly a design choice (equations are tiny anyway) and partly an
  architectural one (`EquationElem` is preserved as a single token).

- **Heuristic similarity threshold (0.3).** Paragraphs whose similarity drops
  below 0.3 — say, a paragraph that was entirely rewritten — appear as a
  separate delete + insert pair rather than a single replace. That's
  intentional but does mean very heavy edits look noisier than light ones.

- **Reordering inside containers.** If a list is reordered (item 1 and item 2
  swap), the slot-shape check fails (the shapes don't match position-wise),
  and the algorithm falls back to flat word-diffing the list. The visible
  result is correct but loses the "this whole item moved" framing.

These are not bugs; they're consequences of the design. A future version
could in principle add a non-adjacent matching pass to detect moves, or a
sub-expression equation diff. Both are nontrivial because they interact with
the rest of the pipeline.

---

## Further reading

If you want to dig deeper, in roughly this order:

- [`docs/technical.md`](technical.md) — the precise per-module reference (less
  narrative, more API surface).
- [`docs/container-diff-regions.md`](container-diff-regions.md) — a design
  note on the next planned step beyond slots.
- The unit tests at the bottom of each `src/*.rs` file — they document
  invariants by example, with names that read like specifications.
- The corpus in [`tests/corpus/`](../tests/corpus) — 48 pairs of small
  documents that exercise specific diff scenarios. Running
  `tests/run_corpus.sh --verbose 02` is a fast way to see what the
  modification log looks like in practice.

The crate is small enough that the most efficient way to understand any
particular detail is still to read the code itself. This document exists to
give you the mental scaffolding to make that reading productive.
