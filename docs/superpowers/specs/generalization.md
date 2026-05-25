Your instinct is right. Some of the changes are the right *behavior*, but not yet the right *shape*.

The current code is carrying a few local fixes that all point at the same underlying abstraction gap: we do not yet have a single, explicit representation of “the renderable semantic body of this annotated node.” Because that concept is implicit, it leaks out as separate helpers and branchy repair logic in [annotate.rs](/Users/koller/Documents/workspace/typst-diff/src/annotate.rs), [annotated.rs](/Users/koller/Documents/workspace/typst-diff/src/annotated.rs), and [diff.rs](/Users/koller/Documents/workspace/typst-diff/src/diff.rs).

**What’s Right**
The high-level choices are sound:

- `Unchanged` as a whole-subtree guarantee is the right semantic invariant.
- Comparing effective subtree content, not just realized wrapper text, is necessary.
- Rendering from `DiffNode` statuses is the right direction.
- The new status-to-render tests are very much the right kind of guardrail.

Those are not special cases. Those are domain truths we want to preserve.

**What Feels Too Special**
These are the parts I would treat as tactical:

- `effective_content` in `diff.rs` and `effective_render_content` in `annotate.rs` are basically the same concept with two names.
- `realized_child_or_fallback` in `annotated.rs` is a narrow fix for wrapper-empty realized children.
- `apply_changed_descendants` now has ordering rules: try grid rebuild, then force list/enum semantic fallback, then try subtree patching, then fallback synthesis.
- `semantic_changed_descendants_fallback` branches on `List`, `Enum`, `Table/Grid`, then unknown sequence.

None of these are awful. They are reasonable moves during a refactor where we’re still discovering Typst realization shapes. But as senior-maintenance code, I would not want this layer to keep accumulating “if list does X, if enum does Y, if empty wrapper does Z” behavior.

The smell is not that lists/enums/tables need type-specific reconstruction. They do. The smell is that type-specific logic is spread across annotation, diffing, and rendering instead of centralized behind one semantic container interface.

**Better General Shape**
I’d introduce a single abstraction around semantic containers, something like a `SemanticAdapter` or equivalent internal helper, not necessarily a Rust trait if that feels heavy.

Conceptually each semantic kind should answer the same questions:

- `extract_children(pre, realized) -> Vec<AnnotatedContent>`
- `slot_labels(pre) -> Vec<SlotStep>`
- `effective_content(node) -> Content`
- `rebuild(rendered_children) -> Content`
- optionally `patch_realized(realized, rendered_children) -> Option<Content>` when preserving realized layout is safe and valuable

Then list, enum, table, grid, stack, quote, figure, wrapper, etc. are still type-specific, but only as implementations of the same protocol. The renderer becomes a uniform fold:

```text
render_node(diff_node):
  Unchanged -> render_effective(node)
  Inserted -> style_insert(render_effective(node))
  Deleted -> style_delete(render_effective(node))
  Modified -> replace_text(render_effective(node), word_diff)
  HasChangedDescendants -> adapter(node).rebuild(rendered_children)
```

That would absorb the current special cases into one intentional design.

**Key Design Principle**
The renderer should not primarily patch arbitrary realized subtrees by equality. That should be an optimization or last resort, not the model.

The model should be:

1. Diff statuses describe semantic change.
2. Annotated nodes expose renderable semantic children.
3. Renderer folds the status tree into a new `Content` tree.
4. Container adapters reconstruct valid Typst containers from rendered children.

That gives us a general solution for corpus 18/19/20 and the similar future cases: nested lists, enums, wrappers that realize as empty blocks, show-rule-expanded containers, table/grid cells, and probably figures/quotes/terms later.

**My Recommendation**
I would keep the current fixes temporarily because they made the failing cases honest and testable. But I would treat them as scaffolding for the next refactor step, not as the final architecture.

The next clean step should be to unify `effective_content` / `effective_render_content` and move “semantic child extraction + container reconstruction” behind one shared semantic-container layer. Once that exists, `apply_changed_descendants` should stop knowing about `List | Enum | Table | Grid` directly except through that layer.

The tests we just added are good because they preserve behavior while we simplify the implementation. They let us refactor away the special cases with confidence: if node statuses are correct and rendering violates them, the tests catch it.