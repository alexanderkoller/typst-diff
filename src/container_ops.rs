//! Container-specific slot mapping and mutation operations.
//!
//! This module is the single owner for structured containers whose semantic
//! children do not always line up with Typst's realized tree shape. Callers
//! provide generic annotated paths or slot labels; the container bundle decides
//! how to extract, replace, insert, and fall back to a patch surface.

use crate::annotated::{
    AnnotatedContent, Annotation, SemanticKind, SemanticSlot, SlotStep, WrapperKind,
    annotate_realized,
};
use crate::normalize::normalize_list_item_runs;
use typst::foundations::{Content, Packed, SequenceElem, StyleChain, StyledElem};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, GridCell, GridChild, GridElem, GridItem,
    PadElem, PlaceElem, StackChild, StackElem,
};
use typst::model::{
    EnumElem, EnumItem, FigureElem, FootnoteBody, FootnoteElem, ListElem, ListItem, ParElem,
    ParbreakElem, QuoteElem, TableCell, TableChild, TableElem, TableItem, TermsElem,
};
use typst::visualize::{CircleElem, EllipseElem, RectElem};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContainerKind {
    List,
    Enum,
    Terms,
    Table,
    Grid,
    Stack,
    Figure,
    Footnote,
    Quote,
    Wrapper(WrapperKind),
}

impl ContainerKind {
    pub(crate) fn of(content: &Content) -> Option<Self> {
        container_kind_of(content)
    }

    pub(crate) fn semantic_kind(&self) -> SemanticKind {
        match self {
            Self::List => SemanticKind::List,
            Self::Enum => SemanticKind::Enum,
            Self::Terms => SemanticKind::Terms,
            Self::Table => SemanticKind::Table,
            Self::Grid => SemanticKind::Grid,
            Self::Stack => SemanticKind::Stack,
            Self::Figure => SemanticKind::Figure,
            Self::Footnote => SemanticKind::Footnote,
            Self::Quote => SemanticKind::Quote,
            Self::Wrapper(kind) => SemanticKind::Wrapper(kind.clone()),
        }
    }
}

pub(crate) struct SlotPart {
    pub(crate) label: SlotStep,
    pub(crate) pre_content: Content,
}

pub(crate) struct SlotMapping {
    pub(crate) patch_surface: Content,
    pub(crate) children: Vec<AnnotatedContent>,
    pub(crate) slots: Vec<SemanticSlot>,
}

trait ContainerOps: Sync {
    fn kind(&self, content: &Content) -> Option<ContainerKind>;
    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>>;

    fn child_contents(&self, content: &Content) -> Option<Vec<Content>> {
        self.slot_parts(content).map(|parts| {
            parts
                .into_iter()
                .map(|part| part.pre_content)
                .collect::<Vec<_>>()
        })
    }

    fn replace_child(
        &self,
        _content: &mut Content,
        _index: usize,
        _replacement: Content,
    ) -> Option<()> {
        None
    }

    fn insert_child(
        &self,
        _content: &mut Content,
        _index: usize,
        _insertion: Content,
        _before: bool,
    ) -> Option<()> {
        None
    }
}

struct ListOps;
struct EnumOps;
struct TermsOps;
struct TableOps;
struct GridOps;
struct StackOps;
struct FigureOps;
struct FootnoteOps;
struct QuoteOps;
struct WrapperOps;

static CONTAINER_OPS: &[&dyn ContainerOps] = &[
    &ListOps,
    &EnumOps,
    &TermsOps,
    &TableOps,
    &GridOps,
    &StackOps,
    &FigureOps,
    &FootnoteOps,
    &QuoteOps,
    &WrapperOps,
];

fn ops_for(content: &Content) -> Option<&'static dyn ContainerOps> {
    CONTAINER_OPS
        .iter()
        .copied()
        .find(|ops| ops.kind(content).is_some())
}

fn container_kind_of(content: &Content) -> Option<ContainerKind> {
    ops_for(content).and_then(|ops| ops.kind(content))
}

impl ContainerOps for ListOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<ListElem>().then_some(ContainerKind::List)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let list = content.to_packed::<ListElem>()?;
        Some(
            list.children
                .iter()
                .enumerate()
                .map(|(idx, item)| SlotPart {
                    label: SlotStep::ListItem(idx),
                    pre_content: item.body.clone(),
                })
                .collect(),
        )
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        content
            .to_packed_mut::<ListElem>()?
            .children
            .get_mut(index)?
            .body = replacement;
        Some(())
    }

    fn insert_child(
        &self,
        content: &mut Content,
        index: usize,
        insertion: Content,
        before: bool,
    ) -> Option<()> {
        let list = content.to_packed_mut::<ListElem>()?;
        let insert_at = if before { index } else { index + 1 };
        if insert_at <= list.children.len() {
            list.children
                .insert(insert_at, Packed::new(ListItem::new(insertion)));
            return Some(());
        }
        None
    }
}

impl ContainerOps for EnumOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<EnumElem>().then_some(ContainerKind::Enum)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let enm = content.to_packed::<EnumElem>()?;
        Some(
            enm.children
                .iter()
                .enumerate()
                .map(|(idx, item)| SlotPart {
                    label: SlotStep::EnumItem(idx),
                    pre_content: item.body.clone(),
                })
                .collect(),
        )
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        content
            .to_packed_mut::<EnumElem>()?
            .children
            .get_mut(index)?
            .body = replacement;
        Some(())
    }

    fn insert_child(
        &self,
        content: &mut Content,
        index: usize,
        insertion: Content,
        before: bool,
    ) -> Option<()> {
        let enm = content.to_packed_mut::<EnumElem>()?;
        let insert_at = if before { index } else { index + 1 };
        if insert_at <= enm.children.len() {
            enm.children
                .insert(insert_at, Packed::new(EnumItem::new(insertion)));
            return Some(());
        }
        None
    }
}

impl ContainerOps for TermsOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<TermsElem>().then_some(ContainerKind::Terms)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let terms = content.to_packed::<TermsElem>()?;
        Some(
            terms
                .children
                .iter()
                .enumerate()
                .flat_map(|(idx, item)| {
                    [
                        SlotPart {
                            label: SlotStep::Term(idx),
                            pre_content: item.term.clone(),
                        },
                        SlotPart {
                            label: SlotStep::TermDescription(idx),
                            pre_content: item.description.clone(),
                        },
                    ]
                })
                .collect(),
        )
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        let item = content
            .to_packed_mut::<TermsElem>()?
            .children
            .get_mut(index / 2)?;
        if index & 1 == 0 {
            item.term = replacement;
        } else {
            item.description = replacement;
        }
        Some(())
    }
}

impl ContainerOps for TableOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<TableElem>().then_some(ContainerKind::Table)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let table = content.to_packed::<TableElem>()?;
        Some(
            table_cell_bodies(&table.children)
                .into_iter()
                .enumerate()
                .map(|(idx, pre_content)| SlotPart {
                    label: SlotStep::TableCell(idx),
                    pre_content,
                })
                .collect(),
        )
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        let table = content.to_packed_mut::<TableElem>()?;
        replace_table_child_cell(&mut table.children, index, replacement, &mut 0)
    }

    fn insert_child(
        &self,
        content: &mut Content,
        index: usize,
        insertion: Content,
        before: bool,
    ) -> Option<()> {
        let table = content.to_packed_mut::<TableElem>()?;
        let insert_at = ordinary_table_insert_index(&table.children, index, before)?;
        table.children.insert(
            insert_at,
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(insertion)))),
        );
        Some(())
    }
}

impl ContainerOps for GridOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<GridElem>().then_some(ContainerKind::Grid)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let grid = content.to_packed::<GridElem>()?;
        Some(
            grid_cell_bodies(&grid.children)
                .into_iter()
                .enumerate()
                .map(|(idx, pre_content)| SlotPart {
                    label: SlotStep::GridCell(idx),
                    pre_content,
                })
                .collect(),
        )
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        let grid = content.to_packed_mut::<GridElem>()?;
        replace_grid_child_cell(&mut grid.children, index, replacement, &mut 0)
    }

    fn insert_child(
        &self,
        content: &mut Content,
        index: usize,
        insertion: Content,
        before: bool,
    ) -> Option<()> {
        let grid = content.to_packed_mut::<GridElem>()?;
        let insert_at = ordinary_grid_insert_index(&grid.children, index, before)?;
        grid.children.insert(
            insert_at,
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(insertion)))),
        );
        Some(())
    }
}

impl ContainerOps for StackOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<StackElem>().then_some(ContainerKind::Stack)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let stack = content.to_packed::<StackElem>()?;
        Some(
            stack
                .children
                .iter()
                .enumerate()
                .filter_map(|(idx, child)| {
                    if let StackChild::Block(body) = child {
                        Some(SlotPart {
                            label: SlotStep::StackChild(idx),
                            pre_content: body.clone(),
                        })
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        let child = content
            .to_packed_mut::<StackElem>()?
            .children
            .get_mut(index)?;
        if let StackChild::Block(body) = child {
            *body = replacement;
            return Some(());
        }
        None
    }
}

impl ContainerOps for FigureOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<FigureElem>().then_some(ContainerKind::Figure)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let figure = content.to_packed::<FigureElem>()?;
        let mut parts = vec![SlotPart {
            label: SlotStep::FigureBody,
            pre_content: figure.body.clone(),
        }];
        if let Some(cap) = figure.caption.get_cloned(StyleChain::default()) {
            parts.push(SlotPart {
                label: SlotStep::FigureCaption,
                pre_content: cap.body.clone(),
            });
        }
        Some(parts)
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        let figure = content.to_packed_mut::<FigureElem>()?;
        if index == 0 {
            figure.body = replacement;
            return Some(());
        }
        if index == 1 {
            figure.caption.as_option_mut().as_mut()?.as_mut()?.body = replacement;
            return Some(());
        }
        None
    }
}

impl ContainerOps for FootnoteOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content
            .is::<FootnoteElem>()
            .then_some(ContainerKind::Footnote)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let footnote = content.to_packed::<FootnoteElem>()?;
        let FootnoteBody::Content(body) = &footnote.body else {
            return None;
        };
        Some(vec![SlotPart {
            label: SlotStep::FootnoteBody,
            pre_content: body.clone(),
        }])
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        if index == 0 {
            content.to_packed_mut::<FootnoteElem>()?.body = FootnoteBody::Content(replacement);
            return Some(());
        }
        None
    }
}

impl ContainerOps for QuoteOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        content.is::<QuoteElem>().then_some(ContainerKind::Quote)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        let quote = content.to_packed::<QuoteElem>()?;
        Some(vec![SlotPart {
            label: SlotStep::QuoteBody,
            pre_content: quote.body.clone(),
        }])
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        if index == 0 {
            content.to_packed_mut::<QuoteElem>()?.body = replacement;
            return Some(());
        }
        None
    }
}

impl ContainerOps for WrapperOps {
    fn kind(&self, content: &Content) -> Option<ContainerKind> {
        wrapper_kind_of(content).map(ContainerKind::Wrapper)
    }

    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>> {
        Some(vec![SlotPart {
            label: SlotStep::WrapperBody,
            pre_content: wrapper_body_of(content)?,
        }])
    }

    fn replace_child(
        &self,
        content: &mut Content,
        index: usize,
        replacement: Content,
    ) -> Option<()> {
        if index == 0 {
            *content = replace_wrapper_body(content.clone(), replacement)?;
            return Some(());
        }
        None
    }
}

pub(crate) fn map_container(
    pre: &Content,
    realized: &Content,
    _kind: ContainerKind,
) -> SlotMapping {
    let Some(ops) = ops_for(pre) else {
        return empty_mapping(realized);
    };
    let Some(parts) = ops.slot_parts(pre) else {
        return empty_mapping(realized);
    };
    map_slot_parts(pre, realized, parts)
}

pub(crate) fn realized_child_contents(content: &Content) -> Vec<Content> {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.clone();
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return vec![styled.child.clone()];
    }
    if let Some(par) = content.to_packed::<ParElem>() {
        return vec![par.body.clone()];
    }
    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return vec![body];
    }
    if let Some(children) = ops_for(content).and_then(|ops| ops.child_contents(content)) {
        return children;
    }
    vec![]
}

pub(crate) fn semantic_diff_child_contents(content: &Content) -> Vec<Content> {
    realized_child_contents(content)
}

pub(crate) fn collect_leaf_block_child_paths(content: &Content) -> Vec<Vec<usize>> {
    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return prepend_path_index(0, collect_leaf_block_child_paths(&body));
    }

    if content.to_packed::<GridElem>().is_some() || content.to_packed::<TableElem>().is_some() {
        let cell_count = realized_child_contents(content).len();
        if cell_count > 0 {
            return (0..cell_count).map(|i| vec![i]).collect();
        }
    }

    if let Some(styled) = content.to_packed::<StyledElem>() {
        let inner = collect_leaf_block_child_paths(&styled.child);
        if !inner.is_empty() {
            return prepend_path_index(0, inner);
        }
    }

    if let Some(list) = content.to_packed::<ListElem>() {
        return (0..list.children.len()).map(|i| vec![i]).collect();
    }

    if let Some(enm) = content.to_packed::<EnumElem>() {
        return (0..enm.children.len()).map(|i| vec![i]).collect();
    }

    if ContainerKind::of(content).is_some() {
        let child_count = realized_child_contents(content).len();
        if child_count > 0 {
            return (0..child_count).map(|i| vec![i]).collect();
        }
    }

    if let Some(seq) = content.to_packed::<SequenceElem>() {
        let mut paths = Vec::new();
        for (index, child) in seq.children.iter().enumerate() {
            let child_paths = collect_leaf_block_child_paths(child);
            if child_paths.is_empty() {
                if !child.is::<ParbreakElem>() {
                    paths.push(vec![index]);
                }
            } else {
                paths.extend(prepend_path_index(index, child_paths));
            }
        }
        return paths;
    }

    vec![]
}

pub(crate) fn replace_realized_child(
    content: &Content,
    index: usize,
    replacement: Content,
) -> Option<Content> {
    let mut result = content.clone();

    if let Some(seq) = result.to_packed_mut::<SequenceElem>() {
        *seq.children.get_mut(index)? = replacement;
        return Some(result);
    }

    if let Some(styled) = result.to_packed_mut::<StyledElem>() {
        if index == 0 {
            styled.child = replacement;
            return Some(result);
        }
        return None;
    }

    if let Some(par) = result.to_packed_mut::<ParElem>() {
        if index == 0 {
            par.body = replacement;
            return Some(result);
        }
        return None;
    }

    if let Some(block) = result.to_packed_mut::<BlockElem>() {
        if index == 0 {
            block.body.set(Some(BlockBody::Content(replacement)));
            return Some(result);
        }
        return None;
    }

    if let Some(ops) = ops_for(&result) {
        ops.replace_child(&mut result, index, replacement)?;
        return Some(result);
    }

    None
}

pub(crate) fn insert_realized_child(
    content: &Content,
    index: usize,
    insertion: Content,
    before: bool,
) -> Option<Content> {
    let mut result = content.clone();

    if let Some(seq) = result.to_packed_mut::<SequenceElem>() {
        let insert_at = if before { index } else { index + 1 };
        if insert_at <= seq.children.len() {
            seq.children.insert(insert_at, insertion);
            return Some(result);
        }
        return None;
    }

    if let Some(ops) = ops_for(&result) {
        ops.insert_child(&mut result, index, insertion, before)?;
        return Some(result);
    }

    None
}

fn map_slot_parts(
    pre_container: &Content,
    realized: &Content,
    mut parts: Vec<SlotPart>,
) -> SlotMapping {
    let realized_paths = collect_leaf_block_child_paths(realized);
    if let Some(mapping) = map_unique_partial_item_container(pre_container, realized, &mut parts) {
        return mapping;
    }
    let use_pre_as_patch_surface = realized_paths.len() < parts.len();
    let patch_surface = if use_pre_as_patch_surface {
        patch_surface_for_opaque_realization(realized, pre_container)
    } else {
        realized.clone()
    };
    let mut tree = anonymous_realized_tree(&patch_surface);
    let paths = collect_leaf_block_child_paths(&patch_surface);
    let mut slots = Vec::new();

    for (idx, part) in parts.into_iter().enumerate() {
        let Some(path) = paths.get(idx).cloned() else {
            continue;
        };
        let mut realized_child = tree
            .get_path(&path)
            .map(|child| child.realized.clone())
            .unwrap_or_else(|| part.pre_content.clone());
        if realized_child.plain_text().is_empty() && !part.pre_content.plain_text().is_empty() {
            realized_child = part.pre_content.clone();
        }
        let pre_content = normalize_list_item_runs(part.pre_content);
        let replacement = annotate_realized(&pre_content, &realized_child);
        if replace_annotated_at_path(&mut tree, &path, replacement) {
            slots.push(SemanticSlot {
                label: part.label,
                path,
            });
        }
    }

    SlotMapping {
        patch_surface: tree.realized,
        children: tree.children,
        slots,
    }
}

fn map_unique_partial_item_container(
    pre_container: &Content,
    realized: &Content,
    parts: &mut Vec<SlotPart>,
) -> Option<SlotMapping> {
    let realized_text = realized.plain_text();
    if realized_text.is_empty() {
        return None;
    }
    let matches: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| {
            (part.pre_content.plain_text() == realized_text).then_some(index)
        })
        .collect();
    if matches.len() != 1 {
        return None;
    }

    let index = matches[0];
    let patch = single_item_patch_surface(pre_container, &parts[index])?;
    let part = parts.remove(index);
    let mut tree = anonymous_realized_tree(&patch.surface);
    let path = patch.path;
    let pre_content = normalize_list_item_runs(part.pre_content);
    let replacement = annotate_realized(&pre_content, realized);
    replace_annotated_at_path(&mut tree, &path, replacement).then(|| SlotMapping {
        patch_surface: tree.realized,
        children: tree.children,
        slots: vec![SemanticSlot {
            label: part.label,
            path,
        }],
    })
}

struct SingleItemPatch {
    surface: Content,
    path: Vec<usize>,
}

fn single_item_patch_surface(pre_container: &Content, part: &SlotPart) -> Option<SingleItemPatch> {
    match &part.label {
        SlotStep::ListItem(index) => {
            let mut list = pre_container.to_packed::<ListElem>()?.clone();
            let mut item = list.children.get(*index)?.clone();
            item.body = part.pre_content.clone();
            list.children = vec![item];
            Some(SingleItemPatch {
                surface: list.pack(),
                path: vec![0],
            })
        }
        SlotStep::EnumItem(index) => {
            let mut enm = pre_container.to_packed::<EnumElem>()?.clone();
            let mut item = enm.children.get(*index)?.clone();
            item.body = part.pre_content.clone();
            enm.children = vec![item];
            Some(SingleItemPatch {
                surface: enm.pack(),
                path: vec![0],
            })
        }
        SlotStep::Term(index) => {
            let mut terms = pre_container.to_packed::<TermsElem>()?.clone();
            let mut item = terms.children.get(*index)?.clone();
            item.term = part.pre_content.clone();
            terms.children = vec![item];
            Some(SingleItemPatch {
                surface: terms.pack(),
                path: vec![0],
            })
        }
        SlotStep::TermDescription(index) => {
            let mut terms = pre_container.to_packed::<TermsElem>()?.clone();
            let mut item = terms.children.get(*index)?.clone();
            item.description = part.pre_content.clone();
            terms.children = vec![item];
            Some(SingleItemPatch {
                surface: terms.pack(),
                path: vec![1],
            })
        }
        _ => None,
    }
}

fn empty_mapping(realized: &Content) -> SlotMapping {
    SlotMapping {
        patch_surface: realized.clone(),
        children: vec![],
        slots: vec![],
    }
}

fn patch_surface_for_opaque_realization(realized: &Content, pre_container: &Content) -> Content {
    graft_opaque_patch_surface(realized, pre_container)
        .unwrap_or_else(|| opaque_pre_surface(pre_container))
}

fn graft_opaque_patch_surface(realized: &Content, pre_container: &Content) -> Option<Content> {
    let mut content = realized.clone();

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = graft_opaque_patch_surface(&styled.child, pre_container)
            .unwrap_or_else(|| opaque_pre_surface(pre_container));
        return Some(content);
    }

    if let Some(block) = content.to_packed_mut::<BlockElem>()
        && matches!(
            block.body.get_cloned(StyleChain::default()),
            Some(BlockBody::Content(_))
        )
    {
        block
            .body
            .set(Some(BlockBody::Content(pre_container.clone())));
        return Some(content);
    }

    None
}

fn opaque_pre_surface(pre_container: &Content) -> Content {
    if has_nested_list_container(pre_container) {
        Content::sequence([Content::new(ParbreakElem::new()), pre_container.clone()])
    } else {
        pre_container.clone()
    }
}

fn has_nested_list_container(content: &Content) -> bool {
    let mut seen_root = content.is::<ListElem>() || content.is::<EnumElem>();
    let mut nested = false;
    let _ = content.traverse::<_, ()>(&mut |child| {
        if child.is::<ListElem>() || child.is::<EnumElem>() {
            if seen_root {
                nested = true;
                return std::ops::ControlFlow::Break(());
            }
            seen_root = true;
        }
        std::ops::ControlFlow::Continue(())
    });
    nested
}

fn anonymous_realized_tree(realized: &Content) -> AnnotatedContent {
    AnnotatedContent {
        realized: realized.clone(),
        annotation: Annotation {
            span: realized.span(),
            ..Annotation::default()
        },
        children: realized_child_contents(realized)
            .into_iter()
            .map(|child| anonymous_realized_tree(&child))
            .collect(),
    }
}

fn replace_annotated_at_path(
    node: &mut AnnotatedContent,
    path: &[usize],
    replacement: AnnotatedContent,
) -> bool {
    let Some((first, rest)) = path.split_first() else {
        *node = replacement;
        return true;
    };
    let Some(child) = node.children.get_mut(*first) else {
        return false;
    };
    replace_annotated_at_path(child, rest, replacement)
}

fn table_cell_bodies(children: &[TableChild]) -> Vec<Content> {
    let mut cells = Vec::new();
    for child in children {
        match child {
            TableChild::Item(TableItem::Cell(cell)) => cells.push(cell.body.clone()),
            TableChild::Header(header) => {
                for item in &header.children {
                    if let TableItem::Cell(cell) = item {
                        cells.push(cell.body.clone());
                    }
                }
            }
            TableChild::Footer(footer) => {
                for item in &footer.children {
                    if let TableItem::Cell(cell) = item {
                        cells.push(cell.body.clone());
                    }
                }
            }
            _ => {}
        }
    }
    cells
}

fn grid_cell_bodies(children: &[GridChild]) -> Vec<Content> {
    let mut cells = Vec::new();
    for child in children {
        match child {
            GridChild::Item(GridItem::Cell(cell)) => cells.push(cell.body.clone()),
            GridChild::Header(header) => {
                for item in &header.children {
                    if let GridItem::Cell(cell) = item {
                        cells.push(cell.body.clone());
                    }
                }
            }
            GridChild::Footer(footer) => {
                for item in &footer.children {
                    if let GridItem::Cell(cell) = item {
                        cells.push(cell.body.clone());
                    }
                }
            }
            _ => {}
        }
    }
    cells
}

fn prepend_path_index(index: usize, mut paths: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    for path in &mut paths {
        path.insert(0, index);
    }
    paths
}

fn replace_wrapper_body(mut content: Content, replacement: Content) -> Option<Content> {
    if let Some(elem) = content.to_packed_mut::<AlignElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<PadElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<PlaceElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<ColumnsElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<BoxElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<BlockElem>() {
        elem.body.set(Some(BlockBody::Content(replacement)));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<RectElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<CircleElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<EllipseElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    None
}

fn wrapper_kind_of(content: &Content) -> Option<WrapperKind> {
    if content.is::<AlignElem>() {
        return Some(WrapperKind::Align);
    }
    if content.is::<PadElem>() {
        return Some(WrapperKind::Pad);
    }
    if content.is::<PlaceElem>() {
        return Some(WrapperKind::Place);
    }
    if content.is::<ColumnsElem>() {
        return Some(WrapperKind::Columns);
    }
    if content.is::<BoxElem>() {
        return Some(WrapperKind::Box);
    }
    if content.is::<BlockElem>() {
        return Some(WrapperKind::Block);
    }
    if content.is::<RectElem>() {
        return Some(WrapperKind::Rect);
    }
    if content.is::<CircleElem>() {
        return Some(WrapperKind::Circle);
    }
    if content.is::<EllipseElem>() {
        return Some(WrapperKind::Ellipse);
    }
    None
}

fn wrapper_body_of(content: &Content) -> Option<Content> {
    if let Some(e) = content.to_packed::<AlignElem>() {
        return Some(e.body.clone());
    }
    if let Some(e) = content.to_packed::<PadElem>() {
        return Some(e.body.clone());
    }
    if let Some(e) = content.to_packed::<PlaceElem>() {
        return Some(e.body.clone());
    }
    if let Some(e) = content.to_packed::<ColumnsElem>() {
        return Some(e.body.clone());
    }
    if let Some(e) = content.to_packed::<BoxElem>() {
        return e.body.get_cloned(StyleChain::default());
    }
    if let Some(e) = content.to_packed::<BlockElem>() {
        return match e.body.get_cloned(StyleChain::default()) {
            Some(BlockBody::Content(b)) => Some(b),
            _ => None,
        };
    }
    if let Some(e) = content.to_packed::<RectElem>() {
        return e.body.get_cloned(StyleChain::default());
    }
    if let Some(e) = content.to_packed::<CircleElem>() {
        return e.body.get_cloned(StyleChain::default());
    }
    if let Some(e) = content.to_packed::<EllipseElem>() {
        return e.body.get_cloned(StyleChain::default());
    }
    None
}

fn replace_table_child_cell(
    children: &mut [TableChild],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for child in children {
        let found = match child {
            TableChild::Header(header) => {
                replace_table_item_cell(&mut header.children, target, replacement.clone(), index)
            }
            TableChild::Footer(footer) => {
                replace_table_item_cell(&mut footer.children, target, replacement.clone(), index)
            }
            TableChild::Item(item) => {
                replace_one_table_item_cell(item, target, replacement.clone(), index)
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn replace_table_item_cell(
    items: &mut [TableItem],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for item in items {
        if replace_one_table_item_cell(item, target, replacement.clone(), index).is_some() {
            return Some(());
        }
    }
    None
}

fn replace_one_table_item_cell(
    item: &mut TableItem,
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    if let TableItem::Cell(cell) = item {
        if *index == target {
            cell.body = replacement;
            return Some(());
        }
        *index += 1;
    }
    None
}

fn replace_grid_child_cell(
    children: &mut [GridChild],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for child in children {
        let found = match child {
            GridChild::Header(header) => {
                replace_grid_item_cell(&mut header.children, target, replacement.clone(), index)
            }
            GridChild::Footer(footer) => {
                replace_grid_item_cell(&mut footer.children, target, replacement.clone(), index)
            }
            GridChild::Item(item) => {
                replace_one_grid_item_cell(item, target, replacement.clone(), index)
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn replace_grid_item_cell(
    items: &mut [GridItem],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for item in items {
        if replace_one_grid_item_cell(item, target, replacement.clone(), index).is_some() {
            return Some(());
        }
    }
    None
}

fn replace_one_grid_item_cell(
    item: &mut GridItem,
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    if let GridItem::Cell(cell) = item {
        if *index == target {
            cell.body = replacement;
            return Some(());
        }
        *index += 1;
    }
    None
}

fn ordinary_table_insert_index(
    children: &[TableChild],
    target: usize,
    before: bool,
) -> Option<usize> {
    let mut seen = 0usize;
    for (child_index, child) in children.iter().enumerate() {
        match child {
            TableChild::Item(TableItem::Cell(_)) => {
                if seen == target {
                    return Some(if before { child_index } else { child_index + 1 });
                }
                seen += 1;
            }
            TableChild::Header(header) => {
                for item in &header.children {
                    if matches!(item, TableItem::Cell(_)) {
                        if seen == target {
                            return None;
                        }
                        seen += 1;
                    }
                }
            }
            TableChild::Footer(footer) => {
                for item in &footer.children {
                    if matches!(item, TableItem::Cell(_)) {
                        if seen == target {
                            return None;
                        }
                        seen += 1;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn ordinary_grid_insert_index(
    children: &[GridChild],
    target: usize,
    before: bool,
) -> Option<usize> {
    let mut seen = 0usize;
    for (child_index, child) in children.iter().enumerate() {
        match child {
            GridChild::Item(GridItem::Cell(_)) => {
                if seen == target {
                    return Some(if before { child_index } else { child_index + 1 });
                }
                seen += 1;
            }
            GridChild::Header(header) => {
                for item in &header.children {
                    if matches!(item, GridItem::Cell(_)) {
                        if seen == target {
                            return None;
                        }
                        seen += 1;
                    }
                }
            }
            GridChild::Footer(footer) => {
                for item in &footer.children {
                    if matches!(item, GridItem::Cell(_)) {
                        if seen == target {
                            return None;
                        }
                        seen += 1;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Packed, StyleChain};
    use typst::model::TermItem;
    use typst::text::TextElem;

    fn text(value: &str) -> Content {
        TextElem::packed(value)
    }

    #[test]
    fn single_item_patch_surface_preserves_list_tightness() {
        let pre = Content::new(
            ListElem::new(vec![
                Packed::new(ListItem::new(text("Alpha"))),
                Packed::new(ListItem::new(text("Beta"))),
            ])
            .with_tight(false),
        );
        let part = SlotPart {
            label: SlotStep::ListItem(1),
            pre_content: text("Beta"),
        };

        let patch = single_item_patch_surface(&pre, &part).unwrap();
        let list = patch.surface.to_packed::<ListElem>().unwrap();

        assert_eq!(patch.path, vec![0]);
        assert_eq!(list.children.len(), 1);
        assert_eq!(list.children[0].body.plain_text(), "Beta");
        assert!(!list.tight.get(StyleChain::default()));
    }

    #[test]
    fn single_item_patch_surface_preserves_enum_tightness() {
        let pre = Content::new(
            EnumElem::new(vec![
                Packed::new(EnumItem::new(text("One"))),
                Packed::new(EnumItem::new(text("Two"))),
            ])
            .with_tight(false),
        );
        let part = SlotPart {
            label: SlotStep::EnumItem(0),
            pre_content: text("One"),
        };

        let patch = single_item_patch_surface(&pre, &part).unwrap();
        let enm = patch.surface.to_packed::<EnumElem>().unwrap();

        assert_eq!(patch.path, vec![0]);
        assert_eq!(enm.children.len(), 1);
        assert_eq!(enm.children[0].body.plain_text(), "One");
        assert!(!enm.tight.get(StyleChain::default()));
    }

    #[test]
    fn single_item_patch_surface_preserves_terms_tightness_and_slot_path() {
        let pre = Content::new(
            TermsElem::new(vec![
                Packed::new(TermItem::new(text("API"), text("Definition"))),
                Packed::new(TermItem::new(text("SDK"), text("Toolkit"))),
            ])
            .with_tight(false),
        );
        let part = SlotPart {
            label: SlotStep::TermDescription(1),
            pre_content: text("Toolkit"),
        };

        let patch = single_item_patch_surface(&pre, &part).unwrap();
        let terms = patch.surface.to_packed::<TermsElem>().unwrap();

        assert_eq!(patch.path, vec![1]);
        assert_eq!(terms.children.len(), 1);
        assert_eq!(terms.children[0].term.plain_text(), "SDK");
        assert_eq!(terms.children[0].description.plain_text(), "Toolkit");
        assert!(!terms.tight.get(StyleChain::default()));
    }
}
