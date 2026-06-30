
  # Container Ops Consolidation

  ## Summary

  Refactor container-specific logic so each container type owns its own slot extraction, child path mapping, replacement, insertion,
  and patch-surface behavior. The generic realized-edit pipeline should remain container-neutral, and remaining legacy flat-slot code
  should reuse the same shared operations until it is removed in a later cleanup.

  ## Key Changes

  - Add internal src/container_ops.rs with grouped implementations by container family:
      - list, enum_list, table, grid
      - terms, stack, figure, footnote, quote, wrapper for existing non-list/table slot containers
  - Move container-specific branches out of generic helpers in annotated.rs and annotate.rs.
      - Generic code may dispatch by container kind, but should not inline list/table/grid mutation logic.
      - Each container bundle should define: pre-slot extraction, realized child extraction, child path discovery, replace child,
        insert child if supported, and patch-surface fallback behavior.
  - Preserve the current realized-edit invariant:
      - AnnotatedContent.realized remains the actual Typst-realized node for matching.
      - Annotation::patch_surface is used only as the structured local edit surface when realization is opaque.
  - Keep diff.rs container-neutral.
      - It should continue to operate on resolved semantic slots, RealizedEdit, and effective slot text for LCS.
  - Do not further invest in the old flat-slot path.
      - If DiffResultFlat, ModifiedSlots, SlotDiff, build_annotated_content, or replace_slot remain after this step, they should
        delegate to container_ops where practical.
      - Do not add new APIs solely to support that old path.
      - Removing the old flat-slot API is a separate follow-up.

  ## Implementation Details

  - Introduce shared internal types:
      - SlotPart { label: SlotStep, pre_content: Content }
      - SlotMapping { patch_surface: Content, children: Vec<AnnotatedContent>, slots: Vec<SemanticSlot> }
      - ContainerKind or equivalent dispatcher enum
  - Centralize common mapping flow:
      - choose realized child paths when Typst realization exposes them
      - otherwise use the pre-realization container as patch_surface
      - build AnnotatedContent children from slot parts and selected child bodies
  - Container bundles:
      - List/enum: item bodies, direct item replacement, item insertion.
      - Table/grid: flattened cell traversal including headers/footers, replacement by cell index, insertion for ordinary cells.
      - Terms/stack/figure/footnote/quote/wrapper: preserve existing slot behavior; return None for unsupported insertion.
  - Replace current scattered logic:
      - realized_child_contents and collect_leaf_block_child_paths should delegate to container_ops.
      - replace_realized_child and insert_realized_child should delegate to container_ops.
      - legacy replace_slot should delegate to container_ops for overlapping operations.

  ## Test Plan

  - Run cargo test -q.
  - Run focused corpus checks:
      - bash tests/run_corpus.sh --filter 18
      - bash tests/run_corpus.sh --filter 19
      - bash tests/run_corpus.sh --filter 20
      - bash tests/run_corpus.sh --filter 64
      - bash tests/run_corpus.sh --filter 65
      - bash tests/run_corpus.sh --filter 69
  - Add or preserve tests for:
      - list slot paths resolving to item bodies, not wrapper nodes
      - opaque list realization using patch_surface without duplicate rendered children
      - table/grid flattened cell replacement and insertion through shared container ops
      - unsupported insertion returning None without falling back to malformed output
      - legacy flat-slot tests still passing only through shared container ops

  ## Assumptions

  - This step is a refactor with no intentional behavior changes and no reference image updates.
  - One new internal module file is acceptable.
  - The old flat-slot path remains temporarily only for compatibility; it should not shape the new design.
  - A later cleanup will remove DiffResultFlat, ModifiedSlots, SlotDiff, build_annotated_content, and replace_slot after callers/
    tests are migrated.
