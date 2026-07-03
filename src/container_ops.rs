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
use crate::content_tree;
use crate::normalize::normalize_list_item_runs;
use crate::patch_surface::PatchSurface;
use typst::foundations::{Content, Packed, SequenceElem, StyleChain, StyledElem, Styles};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, GridCell, GridChild, GridElem, GridItem,
    PadElem, PlaceElem, StackChild, StackElem,
};
use typst::model::{
    EnumElem, EnumItem, FigureCaption, FigureElem, FootnoteBody, FootnoteElem, ListElem, ListItem,
    ParElem, ParbreakElem, QuoteElem, TableCell, TableChild, TableElem, TableItem, TermsElem,
};
use typst::text::StrikeElem;
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
            Self::Quote => SemanticKind::Quote,
            Self::Wrapper(kind) => SemanticKind::Wrapper(kind.clone()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SlotPart {
    pub(crate) label: SlotStep,
    pub(crate) pre_content: Content,
}

pub(crate) struct SlotMapping {
    pub(crate) patch_surface: PatchSurface,
    pub(crate) children: Vec<AnnotatedContent>,
    pub(crate) slots: Vec<SemanticSlot>,
}

trait ContainerOps: Sync {
    fn kind(&self, content: &Content) -> Option<ContainerKind>;
    fn slot_parts(&self, content: &Content) -> Option<Vec<SlotPart>>;

    fn map_slots(
        &self,
        _pre_container: &Content,
        _realized: &Content,
        _parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        None
    }

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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        map_item_container_slots(
            pre_container,
            realized,
            parts,
            |label| matches!(label, SlotStep::ListItem(_)),
            single_list_item_patch_surface,
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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        map_item_container_slots(
            pre_container,
            realized,
            parts,
            |label| matches!(label, SlotStep::EnumItem(_)),
            single_enum_item_patch_surface,
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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        map_item_container_slots(
            pre_container,
            realized,
            parts,
            |label| matches!(label, SlotStep::Term(_) | SlotStep::TermDescription(_)),
            single_terms_item_patch_surface,
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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        let paths = realized_container_child_paths(realized, parts.len(), |content| {
            content.is::<TableElem>()
        });
        map_realized_or_pre_container_patch_surface(pre_container, realized, parts, paths)
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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        let paths = realized_container_child_paths(realized, parts.len(), |content| {
            content.is::<GridElem>()
        });
        map_realized_or_pre_container_patch_surface(pre_container, realized, parts, paths)
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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        let paths = realized_container_child_paths(realized, parts.len(), |content| {
            content.is::<StackElem>()
        });
        map_realized_or_pre_container_patch_surface(pre_container, realized, parts, paths)
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

    fn insert_child(
        &self,
        content: &mut Content,
        index: usize,
        insertion: Content,
        before: bool,
    ) -> Option<()> {
        let figure = content.to_packed_mut::<FigureElem>()?;
        if index == 0 && !before && figure.caption.get_cloned(StyleChain::default()).is_none() {
            let insertion = unwrap_figure_caption_payload(insertion);
            if contains_strike(&insertion) {
                figure.body = Content::sequence([figure.body.clone(), insertion]);
                return Some(());
            }
            figure
                .caption
                .set(Some(Packed::new(FigureCaption::new(insertion))));
            return Some(());
        }
        None
    }

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        // Figure slots are authored positions on FigureElem; realized v/caption
        // scaffolding is not part of the patch-surface path contract.
        let paths = figure_slot_paths(&parts);
        if paths.len() != parts.len() {
            return None;
        }
        if let Some(mapping) = map_figure_realized_patch_surface(realized, parts.clone()) {
            return Some(mapping);
        }
        Some(map_explicit_patch_surface(
            PatchSurface::pre_container(pre_container.clone()),
            parts,
            paths,
        ))
    }
}

fn unwrap_figure_caption_payload(content: Content) -> Content {
    let mut content = content;

    if let Some(caption) = content.to_packed::<FigureCaption>() {
        return caption.body.clone();
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = unwrap_figure_caption_payload(styled.child.clone());
        return content;
    }

    if let Some(strike) = content.to_packed_mut::<StrikeElem>() {
        strike.body = unwrap_figure_caption_payload(strike.body.clone());
        return content;
    }

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = seq
            .children
            .iter()
            .cloned()
            .map(unwrap_figure_caption_payload)
            .collect();
        return content;
    }

    content
}

fn contains_strike(content: &Content) -> bool {
    if content.is::<StrikeElem>() {
        return true;
    }

    if let Some(styled) = content.to_packed::<StyledElem>() {
        return contains_strike(&styled.child);
    }

    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.iter().any(contains_strike);
    }

    if let Some(caption) = content.to_packed::<FigureCaption>() {
        return contains_strike(&caption.body);
    }

    false
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

    fn map_slots(
        &self,
        pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        if parts.len() != 1 || parts[0].label != SlotStep::QuoteBody {
            return None;
        }
        map_realized_or_pre_container_patch_surface(
            pre_container,
            realized,
            parts,
            quote_realized_body_paths(realized),
        )
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

    fn map_slots(
        &self,
        _pre_container: &Content,
        realized: &Content,
        parts: Vec<SlotPart>,
    ) -> Option<SlotMapping> {
        let mut parts = parts.into_iter();
        let part = parts.next()?;
        if parts.next().is_some() || part.label != SlotStep::WrapperBody {
            return None;
        }

        let wrapper_kind = wrapper_kind_of(_pre_container)?;
        let pre_content = normalize_list_item_runs(part.pre_content);
        let surface = match wrapper_slot_patch_surface(realized, &wrapper_kind, &pre_content) {
            Some(surface) => surface,
            None => {
                return map_realized_or_pre_container_patch_surface(
                    _pre_container,
                    realized,
                    vec![SlotPart {
                        label: SlotStep::WrapperBody,
                        pre_content,
                    }],
                    None,
                );
            }
        };
        let Some(path) = direct_wrapper_body_path(&surface)
            .or_else(|| realized_wrapper_body_path(&surface, &wrapper_kind, &pre_content))
        else {
            return map_realized_or_pre_container_patch_surface(
                _pre_container,
                realized,
                vec![SlotPart {
                    label: SlotStep::WrapperBody,
                    pre_content,
                }],
                None,
            );
        };
        let mut tree = anonymous_realized_tree(&surface);
        let realized_body = tree.get_path(&path)?.realized.clone();
        let replacement = annotate_realized(&pre_content, &realized_body);
        content_tree::replace_annotated_at_path(&mut tree, &path, replacement).then(|| {
            SlotMapping {
                patch_surface: PatchSurface::grafted_block_body(tree.realized),
                children: tree.children,
                slots: vec![SemanticSlot {
                    label: SlotStep::WrapperBody,
                    path,
                    patch_path: None,
                }],
            }
        })
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

fn direct_wrapper_body_path(content: &Content) -> Option<Vec<usize>> {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let mut path = direct_wrapper_body_path(&styled.child)?;
        path.insert(0, 0);
        return Some(path);
    }

    wrapper_kind_of(content).map(|_| vec![0])
}

fn realized_wrapper_body_path(
    content: &Content,
    kind: &WrapperKind,
    expected_body: &Content,
) -> Option<Vec<usize>> {
    let expected_text = expected_body.plain_text();
    if expected_text.trim().is_empty() {
        return None;
    }
    let mut wrapper_path = find_realized_path(content, &mut |child| {
        wrapper_kind_of(child).as_ref() == Some(kind)
            && wrapper_body_of(child).is_some_and(|body| body.plain_text() == expected_text)
    })?;
    wrapper_path.push(0);
    Some(wrapper_path)
}

fn wrapper_slot_patch_surface(
    realized: &Content,
    kind: &WrapperKind,
    pre_body: &Content,
) -> Option<Content> {
    if direct_wrapper_body_path(realized).is_some()
        || realized_wrapper_body_path(realized, kind, pre_body).is_some()
    {
        return Some(realized.clone());
    }

    let wrapper_path = container_owned_unique_wrapper_path(realized, kind)?;
    let wrapper = content_tree::realized_content_at_path(realized, &wrapper_path)?;
    let patched_wrapper = replace_wrapper_body(wrapper, pre_body.clone())?;
    content_tree::replace_realized_content_at_path(realized, &wrapper_path, patched_wrapper)
}

fn container_owned_unique_wrapper_path(
    content: &Content,
    kind: &WrapperKind,
) -> Option<Vec<usize>> {
    let mut paths = Vec::new();
    collect_container_owned_wrapper_paths(content, kind, &mut Vec::new(), &mut paths);
    (paths.len() == 1).then(|| paths.remove(0))
}

fn collect_container_owned_wrapper_paths(
    content: &Content,
    kind: &WrapperKind,
    prefix: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
) {
    if wrapper_kind_of(content).as_ref() == Some(kind) {
        out.push(prefix.clone());
    }
    for (index, child) in realized_child_contents(content).into_iter().enumerate() {
        prefix.push(index);
        collect_container_owned_wrapper_paths(&child, kind, prefix, out);
        prefix.pop();
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
    if let Some(mapping) = ops.map_slots(pre, realized, parts) {
        return mapping;
    }
    empty_mapping(realized)
}

pub(crate) fn realized_child_contents(content: &Content) -> Vec<Content> {
    realized_child_contents_with_styles(content, &Styles::new())
}

fn realized_child_contents_with_styles(content: &Content, styles: &Styles) -> Vec<Content> {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.clone();
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let styles = StyleChain::new(styles).chain(&styled.styles).to_map();
        return vec![materialize_style_dependent_fields_with_styles(
            &styled.child,
            &styles,
        )];
    }
    if let Some(par) = content.to_packed::<ParElem>() {
        return vec![par.body.clone()];
    }
    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::new(styles))
    {
        return vec![body];
    }
    if let Some(footnote) = content.to_packed::<FootnoteElem>()
        && let FootnoteBody::Content(body) = &footnote.body
    {
        return vec![body.clone()];
    }
    if let Some(body) = wrapper_body_of_with_styles(content, styles) {
        return vec![body];
    }
    if let Some(children) = ops_for(content).and_then(|ops| ops.child_contents(content)) {
        return children;
    }
    vec![]
}

pub(crate) fn materialize_style_dependent_fields(content: &Content, styles: &Styles) -> Content {
    materialize_style_dependent_fields_with_styles(content, styles)
}

fn materialize_style_dependent_fields_with_styles(content: &Content, styles: &Styles) -> Content {
    let chain = StyleChain::new(styles);
    let mut result = content.clone();
    result.materialize(chain);

    if let Some(seq) = result.to_packed_mut::<SequenceElem>() {
        for child in &mut seq.children {
            *child = materialize_style_dependent_fields_with_styles(child, styles);
        }
        return result;
    }
    if let Some(par) = result.to_packed_mut::<ParElem>() {
        par.body = materialize_style_dependent_fields_with_styles(&par.body, styles);
        return result;
    }
    if let Some(styled) = result.to_packed_mut::<StyledElem>() {
        let styles = StyleChain::new(styles).chain(&styled.styles).to_map();
        styled.child = materialize_style_dependent_fields_with_styles(&styled.child, &styles);
        return result;
    }
    if let Some(elem) = result.to_packed_mut::<BoxElem>()
        && elem.body.get_cloned(StyleChain::default()).is_none()
        && let Some(body) = elem.body.get_cloned(chain)
    {
        elem.body.set(Some(body));
        return result;
    }
    if let Some(elem) = result.to_packed_mut::<BlockElem>()
        && elem.body.get_cloned(StyleChain::default()).is_none()
        && let Some(body) = elem.body.get_cloned(chain)
    {
        elem.body.set(Some(body));
        return result;
    }
    if let Some(elem) = result.to_packed_mut::<RectElem>()
        && elem.body.get_cloned(StyleChain::default()).is_none()
        && let Some(body) = elem.body.get_cloned(chain)
    {
        elem.body.set(Some(body));
        return result;
    }
    if let Some(elem) = result.to_packed_mut::<CircleElem>()
        && elem.body.get_cloned(StyleChain::default()).is_none()
        && let Some(body) = elem.body.get_cloned(chain)
    {
        elem.body.set(Some(body));
        return result;
    }
    if let Some(elem) = result.to_packed_mut::<EllipseElem>()
        && elem.body.get_cloned(StyleChain::default()).is_none()
        && let Some(body) = elem.body.get_cloned(chain)
    {
        elem.body.set(Some(body));
    }

    result
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

    if let Some(footnote) = result.to_packed_mut::<FootnoteElem>() {
        if index == 0 {
            footnote.body = FootnoteBody::Content(replacement);
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

fn direct_index_paths(len: usize) -> Vec<Vec<usize>> {
    (0..len).map(|index| vec![index]).collect()
}

fn realized_container_child_paths(
    realized: &Content,
    slot_count: usize,
    container_matches: impl Fn(&Content) -> bool + Copy,
) -> Option<Vec<Vec<usize>>> {
    if container_matches(realized) && realized_child_contents(realized).len() == slot_count {
        return Some(direct_index_paths(slot_count));
    }

    if let Some(seq) = realized.to_packed::<SequenceElem>() {
        if seq.children.len() == slot_count {
            return Some(direct_index_paths(slot_count));
        }
    }

    if let Some(styled) = realized.to_packed::<StyledElem>() {
        return prepend_realized_shell_path(realized_container_child_paths(
            &styled.child,
            slot_count,
            container_matches,
        ));
    }

    if let Some(block) = realized.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return prepend_realized_shell_path(realized_container_child_paths(
            &body,
            slot_count,
            container_matches,
        ));
    }

    let leaf_paths = collect_leaf_block_child_paths(realized);
    if leaf_paths.len() == slot_count {
        return Some(leaf_paths);
    }

    None
}

fn prepend_realized_shell_path(paths: Option<Vec<Vec<usize>>>) -> Option<Vec<Vec<usize>>> {
    paths.map(|paths| prepend_path_index(0, paths))
}

fn map_realized_patch_surface(
    patch_surface: PatchSurface,
    parts: Vec<SlotPart>,
    paths: Vec<Vec<usize>>,
) -> Option<SlotMapping> {
    if parts.len() != paths.len() {
        return None;
    }

    let mut tree = anonymous_realized_tree(patch_surface.as_content());
    let mut slots = Vec::new();

    for (part, path) in parts.into_iter().zip(paths) {
        let Some(realized_child) = tree.get_path(&path).map(|child| child.realized.clone()) else {
            return None;
        };
        let realized_child = if realized_child.plain_text().is_empty()
            && !part.pre_content.plain_text().is_empty()
        {
            part.pre_content.clone()
        } else {
            realized_child
        };
        let pre_content = normalize_list_item_runs(part.pre_content);
        let replacement = annotate_realized(&pre_content, &realized_child);
        if !content_tree::replace_annotated_at_path(&mut tree, &path, replacement) {
            return None;
        }
        slots.push(SemanticSlot {
            label: part.label,
            path,
            patch_path: None,
        });
    }

    Some(SlotMapping {
        patch_surface: patch_surface.map_content(|_| tree.realized),
        children: tree.children,
        slots,
    })
}

fn map_realized_or_pre_container_patch_surface(
    pre_container: &Content,
    realized: &Content,
    parts: Vec<SlotPart>,
    realized_paths: Option<Vec<Vec<usize>>>,
) -> Option<SlotMapping> {
    if let Some(paths) = realized_paths {
        return map_realized_patch_surface(
            PatchSurface::pre_container(realized.clone()),
            parts,
            paths,
        );
    }

    if collect_leaf_block_child_paths(realized).len() >= parts.len() {
        return None;
    }

    let patch_surface = container_owned_opaque_patch_surface(realized, pre_container);
    let paths = collect_leaf_block_child_paths(patch_surface.as_content());
    map_realized_patch_surface(patch_surface, parts, paths)
}

fn map_item_container_slots(
    pre_container: &Content,
    realized: &Content,
    parts: Vec<SlotPart>,
    label_belongs_to_container: impl Fn(&SlotStep) -> bool,
    single_item_patch: impl Fn(&Content, &SlotPart) -> Option<SingleItemPatch>,
) -> Option<SlotMapping> {
    if parts
        .iter()
        .any(|part| !label_belongs_to_container(&part.label))
    {
        return None;
    }

    if let Some(mapping) = map_single_item_container_by_span(realized, &parts, |part| {
        single_item_patch(pre_container, part)
    }) {
        return Some(mapping);
    }
    if let Some(mapping) = map_single_item_container_by_unique_text(realized, &parts, |part| {
        single_item_patch(pre_container, part)
    }) {
        return Some(mapping);
    }

    let paths = realized_item_container_child_paths(realized, parts.len());
    map_realized_or_pre_container_patch_surface(pre_container, realized, parts, paths)
}

fn realized_item_container_child_paths(
    realized: &Content,
    slot_count: usize,
) -> Option<Vec<Vec<usize>>> {
    realized_container_child_paths(realized, slot_count, |content| {
        content.is::<ListElem>()
            || content.is::<EnumElem>()
            || content.is::<TermsElem>()
            || content.is::<GridElem>()
            || content.is::<TableElem>()
    })
}

fn quote_realized_body_paths(realized: &Content) -> Option<Vec<Vec<usize>>> {
    let leaf_paths = collect_leaf_block_child_paths(realized);
    if let Some(path) = leaf_paths.into_iter().next() {
        return Some(vec![path]);
    }
    realized_container_child_paths(realized, 1, |content| content.is::<QuoteElem>())
}

fn map_single_item_container_by_span(
    realized: &Content,
    parts: &[SlotPart],
    single_item_patch: impl Fn(&SlotPart) -> Option<SingleItemPatch>,
) -> Option<SlotMapping> {
    let realized_spans = provenance_spans(realized);
    if realized_spans.is_empty() {
        return None;
    }
    let matches: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| {
            provenance_spans_overlap(&provenance_spans(&part.pre_content), &realized_spans)
                .then_some(index)
        })
        .collect();
    if matches.len() != 1 {
        return None;
    }

    let part = parts.get(matches[0])?;
    let patch = single_item_patch(part)?;
    let mut tree = anonymous_realized_tree(&patch.surface);
    let path = patch.path;
    let pre_content = normalize_list_item_runs(part.pre_content.clone());
    let replacement = annotate_realized(&pre_content, realized);
    content_tree::replace_annotated_at_path(&mut tree, &path, replacement).then(|| SlotMapping {
        patch_surface: PatchSurface::layout_preserving_sequence(tree.realized),
        children: tree.children,
        slots: vec![SemanticSlot {
            label: part.label.clone(),
            path,
            patch_path: None,
        }],
    })
}

fn provenance_spans(content: &Content) -> Vec<typst::syntax::Span> {
    let mut spans = Vec::new();
    collect_provenance_spans(content, &mut spans);
    spans
}

fn collect_provenance_spans(content: &Content, out: &mut Vec<typst::syntax::Span>) {
    let span = content.span();
    if !span.is_detached() && !out.contains(&span) {
        out.push(span);
    }
    for child in realized_child_contents(content) {
        collect_provenance_spans(&child, out);
    }
}

fn provenance_spans_overlap(
    source_spans: &[typst::syntax::Span],
    realized_spans: &[typst::syntax::Span],
) -> bool {
    realized_spans
        .iter()
        .any(|span| source_spans.contains(span))
}

fn map_single_item_container_by_unique_text(
    realized: &Content,
    parts: &[SlotPart],
    single_item_patch: impl Fn(&SlotPart) -> Option<SingleItemPatch>,
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

    let part = parts.get(matches[0])?;
    let patch = single_item_patch(part)?;
    let mut tree = anonymous_realized_tree(&patch.surface);
    let path = patch.path;
    let pre_content = normalize_list_item_runs(part.pre_content.clone());
    let replacement = annotate_realized(&pre_content, realized);
    content_tree::replace_annotated_at_path(&mut tree, &path, replacement).then(|| SlotMapping {
        patch_surface: PatchSurface::layout_preserving_sequence(tree.realized),
        children: tree.children,
        slots: vec![SemanticSlot {
            label: part.label.clone(),
            path,
            patch_path: None,
        }],
    })
}

fn figure_slot_paths(parts: &[SlotPart]) -> Vec<Vec<usize>> {
    let mut paths = Vec::with_capacity(parts.len());
    for part in parts {
        match part.label {
            SlotStep::FigureBody => paths.push(vec![0]),
            SlotStep::FigureCaption => paths.push(vec![1]),
            _ => return vec![],
        }
    }
    paths
}

fn map_figure_realized_patch_surface(
    realized: &Content,
    parts: Vec<SlotPart>,
) -> Option<SlotMapping> {
    let caption_path = find_realized_figure_caption_path(realized)?;
    let body_part = parts
        .iter()
        .find(|part| matches!(part.label, SlotStep::FigureBody))?;
    let body_path = find_realized_figure_body_path(
        realized,
        &caption_path,
        &normalize_list_item_runs(body_part.pre_content.clone()),
    )?;

    let tree = anonymous_realized_tree(realized);
    let mut children = Vec::new();
    let mut slots = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        let (path, patch_path) = match part.label {
            SlotStep::FigureBody => (vec![index], Some(body_path.clone())),
            SlotStep::FigureCaption => (vec![index], Some(caption_path.clone())),
            _ => return None,
        };
        let realized_child = patch_path
            .as_ref()
            .and_then(|path| tree.get_path(path).map(|child| child.realized.clone()))
            .unwrap_or_else(|| part.pre_content.clone());
        let pre_content = normalize_list_item_runs(part.pre_content);
        children.push(annotate_realized(&pre_content, &realized_child));
        slots.push(SemanticSlot {
            label: part.label,
            path,
            patch_path,
        });
    }

    Some(SlotMapping {
        patch_surface: PatchSurface::pre_container(realized.clone()),
        children,
        slots,
    })
}

fn find_realized_figure_caption_path(content: &Content) -> Option<Vec<usize>> {
    find_realized_path(content, &mut |child| child.is::<FigureCaption>())
}

fn find_realized_figure_body_path(
    content: &Content,
    caption_path: &[usize],
    pre_body: &Content,
) -> Option<Vec<usize>> {
    if let Some(path) = find_realized_slot_surface_path(content, caption_path, pre_body) {
        return Some(path);
    }

    collect_leaf_block_child_paths(content)
        .into_iter()
        .filter(|path| !path_is_under(path, caption_path))
        .find(|path| {
            content_tree::realized_content_at_path(content, path)
                .is_some_and(|child| !child.is::<ParbreakElem>() && child.func().name() != "v")
        })
}

fn find_realized_slot_surface_path(
    content: &Content,
    exclude_path: &[usize],
    pre_slot: &Content,
) -> Option<Vec<usize>> {
    let pre_kind = ContainerKind::of(pre_slot)?;
    find_realized_descendant_path_outside(content, exclude_path, &mut |child| {
        ContainerKind::of(child).as_ref() == Some(&pre_kind)
    })
}

fn path_is_under(path: &[usize], ancestor: &[usize]) -> bool {
    !ancestor.is_empty() && path.starts_with(ancestor)
}

fn find_realized_descendant_path_outside(
    content: &Content,
    exclude_path: &[usize],
    predicate: &mut impl FnMut(&Content) -> bool,
) -> Option<Vec<usize>> {
    fn walk(
        content: &Content,
        prefix: &[usize],
        exclude_path: &[usize],
        predicate: &mut impl FnMut(&Content) -> bool,
    ) -> Option<Vec<usize>> {
        for (index, child) in realized_child_contents(content).into_iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(index);
            if path_is_under(&path, exclude_path) {
                continue;
            }
            if predicate(&child) {
                return Some(path);
            }
            if let Some(found) = walk(&child, &path, exclude_path, predicate) {
                return Some(found);
            }
        }
        None
    }

    walk(content, &[], exclude_path, predicate)
}

fn find_realized_path(
    content: &Content,
    predicate: &mut impl FnMut(&Content) -> bool,
) -> Option<Vec<usize>> {
    if predicate(content) {
        return Some(vec![]);
    }
    for (index, child) in realized_child_contents(content).into_iter().enumerate() {
        if let Some(mut path) = find_realized_path(&child, predicate) {
            path.insert(0, index);
            return Some(path);
        }
    }
    None
}

fn map_explicit_patch_surface(
    patch_surface: PatchSurface,
    parts: Vec<SlotPart>,
    paths: Vec<Vec<usize>>,
) -> SlotMapping {
    let mut tree = anonymous_realized_tree(patch_surface.as_content());
    let mut slots = Vec::new();

    for (part, path) in parts.into_iter().zip(paths) {
        let pre_content = normalize_list_item_runs(part.pre_content);
        let replacement = annotate_realized(&pre_content, &pre_content);
        if content_tree::replace_annotated_at_path(&mut tree, &path, replacement) {
            slots.push(SemanticSlot {
                label: part.label,
                path,
                patch_path: None,
            });
        }
    }

    SlotMapping {
        patch_surface: patch_surface.map_content(|_| tree.realized),
        children: tree.children,
        slots,
    }
}

struct SingleItemPatch {
    surface: Content,
    path: Vec<usize>,
}

fn single_list_item_patch_surface(
    pre_container: &Content,
    part: &SlotPart,
) -> Option<SingleItemPatch> {
    let SlotStep::ListItem(index) = part.label else {
        return None;
    };
    let mut list = pre_container.to_packed::<ListElem>()?.clone();
    let mut item = list.children.get(index)?.clone();
    item.body = part.pre_content.clone();
    list.children = vec![item];
    Some(SingleItemPatch {
        surface: list.pack(),
        path: vec![0],
    })
}

fn single_enum_item_patch_surface(
    pre_container: &Content,
    part: &SlotPart,
) -> Option<SingleItemPatch> {
    let SlotStep::EnumItem(index) = part.label else {
        return None;
    };
    let mut enm = pre_container.to_packed::<EnumElem>()?.clone();
    let mut item = enm.children.get(index)?.clone();
    item.body = part.pre_content.clone();
    enm.children = vec![item];
    Some(SingleItemPatch {
        surface: enm.pack(),
        path: vec![0],
    })
}

fn single_terms_item_patch_surface(
    pre_container: &Content,
    part: &SlotPart,
) -> Option<SingleItemPatch> {
    let mut terms = pre_container.to_packed::<TermsElem>()?.clone();
    let item_index = match part.label {
        SlotStep::Term(index) | SlotStep::TermDescription(index) => index,
        _ => return None,
    };
    let mut item = terms.children.get(item_index)?.clone();
    let path = match part.label {
        SlotStep::Term(_) => {
            item.term = part.pre_content.clone();
            vec![0]
        }
        SlotStep::TermDescription(_) => {
            item.description = part.pre_content.clone();
            vec![1]
        }
        _ => return None,
    };
    terms.children = vec![item];
    Some(SingleItemPatch {
        surface: terms.pack(),
        path,
    })
}

fn container_owned_opaque_patch_surface(
    realized: &Content,
    pre_container: &Content,
) -> PatchSurface {
    if let Some(surface) = container_owned_grafted_surface(realized, pre_container) {
        PatchSurface::grafted_block_body(surface)
    } else {
        PatchSurface::opaque_visual(container_owned_pre_surface(pre_container))
    }
}

fn container_owned_grafted_surface(realized: &Content, pre_container: &Content) -> Option<Content> {
    let mut content = realized.clone();

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = container_owned_grafted_surface(&styled.child, pre_container)
            .unwrap_or_else(|| container_owned_pre_surface(pre_container));
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

fn container_owned_pre_surface(pre_container: &Content) -> Content {
    if contains_nested_item_container(pre_container) {
        Content::sequence([Content::new(ParbreakElem::new()), pre_container.clone()])
    } else {
        pre_container.clone()
    }
}

fn contains_nested_item_container(content: &Content) -> bool {
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

fn empty_mapping(realized: &Content) -> SlotMapping {
    SlotMapping {
        patch_surface: PatchSurface::pre_container(realized.clone()),
        children: vec![],
        slots: vec![],
    }
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
    wrapper_body_of_with_styles(content, &Styles::new())
}

fn wrapper_body_of_with_styles(content: &Content, styles: &Styles) -> Option<Content> {
    let chain = StyleChain::new(styles);
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
        return e.body.get_cloned(chain);
    }
    if let Some(e) = content.to_packed::<BlockElem>() {
        return match e.body.get_cloned(chain) {
            Some(BlockBody::Content(b)) => Some(b),
            _ => None,
        };
    }
    if let Some(e) = content.to_packed::<RectElem>() {
        return e.body.get_cloned(chain);
    }
    if let Some(e) = content.to_packed::<CircleElem>() {
        return e.body.get_cloned(chain);
    }
    if let Some(e) = content.to_packed::<EllipseElem>() {
        return e.body.get_cloned(chain);
    }
    None
}

trait IndexedCellItem {
    fn cell_body(&self) -> Option<Content>;
    fn replace_cell_body(&mut self, replacement: Content) -> Option<()>;
}

impl IndexedCellItem for TableItem {
    fn cell_body(&self) -> Option<Content> {
        match self {
            TableItem::Cell(cell) => Some(cell.body.clone()),
            _ => None,
        }
    }

    fn replace_cell_body(&mut self, replacement: Content) -> Option<()> {
        match self {
            TableItem::Cell(cell) => {
                cell.body = replacement;
                Some(())
            }
            _ => None,
        }
    }
}

impl IndexedCellItem for GridItem {
    fn cell_body(&self) -> Option<Content> {
        match self {
            GridItem::Cell(cell) => Some(cell.body.clone()),
            _ => None,
        }
    }

    fn replace_cell_body(&mut self, replacement: Content) -> Option<()> {
        match self {
            GridItem::Cell(cell) => {
                cell.body = replacement;
                Some(())
            }
            _ => None,
        }
    }
}

enum IndexedCellItems<'a, Item> {
    Ordinary(&'a Item),
    Group(&'a [Item]),
}

enum IndexedCellItemsMut<'a, Item> {
    Ordinary(&'a mut Item),
    Group(&'a mut [Item]),
}

trait IndexedCellChild {
    type Item: IndexedCellItem;

    fn cell_items(&self) -> IndexedCellItems<'_, <Self as IndexedCellChild>::Item>;
    fn cell_items_mut(&mut self) -> IndexedCellItemsMut<'_, <Self as IndexedCellChild>::Item>;
}

impl IndexedCellChild for TableChild {
    type Item = TableItem;

    fn cell_items(&self) -> IndexedCellItems<'_, <Self as IndexedCellChild>::Item> {
        match self {
            TableChild::Item(item) => IndexedCellItems::Ordinary(item),
            TableChild::Header(header) => IndexedCellItems::Group(&header.children),
            TableChild::Footer(footer) => IndexedCellItems::Group(&footer.children),
        }
    }

    fn cell_items_mut(&mut self) -> IndexedCellItemsMut<'_, <Self as IndexedCellChild>::Item> {
        match self {
            TableChild::Item(item) => IndexedCellItemsMut::Ordinary(item),
            TableChild::Header(header) => IndexedCellItemsMut::Group(&mut header.children),
            TableChild::Footer(footer) => IndexedCellItemsMut::Group(&mut footer.children),
        }
    }
}

impl IndexedCellChild for GridChild {
    type Item = GridItem;

    fn cell_items(&self) -> IndexedCellItems<'_, <Self as IndexedCellChild>::Item> {
        match self {
            GridChild::Item(item) => IndexedCellItems::Ordinary(item),
            GridChild::Header(header) => IndexedCellItems::Group(&header.children),
            GridChild::Footer(footer) => IndexedCellItems::Group(&footer.children),
        }
    }

    fn cell_items_mut(&mut self) -> IndexedCellItemsMut<'_, <Self as IndexedCellChild>::Item> {
        match self {
            GridChild::Item(item) => IndexedCellItemsMut::Ordinary(item),
            GridChild::Header(header) => IndexedCellItemsMut::Group(&mut header.children),
            GridChild::Footer(footer) => IndexedCellItemsMut::Group(&mut footer.children),
        }
    }
}

fn table_cell_bodies(children: &[TableChild]) -> Vec<Content> {
    indexed_cell_bodies(children)
}

fn grid_cell_bodies(children: &[GridChild]) -> Vec<Content> {
    indexed_cell_bodies(children)
}

fn indexed_cell_bodies<Child: IndexedCellChild>(children: &[Child]) -> Vec<Content> {
    let mut cells = Vec::new();
    for child in children {
        match child.cell_items() {
            IndexedCellItems::Ordinary(item) => {
                if let Some(body) = item.cell_body() {
                    cells.push(body);
                }
            }
            IndexedCellItems::Group(items) => {
                cells.extend(items.iter().filter_map(IndexedCellItem::cell_body));
            }
        }
    }
    cells
}

fn replace_table_child_cell(
    children: &mut [TableChild],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    replace_indexed_child_cell(children, target, replacement, index)
}

fn replace_grid_child_cell(
    children: &mut [GridChild],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    replace_indexed_child_cell(children, target, replacement, index)
}

fn replace_indexed_child_cell<Child: IndexedCellChild>(
    children: &mut [Child],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for child in children {
        let found = match child.cell_items_mut() {
            IndexedCellItemsMut::Ordinary(item) => {
                replace_one_indexed_cell(item, target, replacement.clone(), index)
            }
            IndexedCellItemsMut::Group(items) => {
                replace_indexed_item_cell(items, target, replacement.clone(), index)
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn replace_indexed_item_cell<Item: IndexedCellItem>(
    items: &mut [Item],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for item in items {
        if replace_one_indexed_cell(item, target, replacement.clone(), index).is_some() {
            return Some(());
        }
    }
    None
}

fn replace_one_indexed_cell<Item: IndexedCellItem>(
    item: &mut Item,
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    item.cell_body()?;
    if *index == target {
        return item.replace_cell_body(replacement);
    }
    *index += 1;
    None
}

fn ordinary_table_insert_index(
    children: &[TableChild],
    target: usize,
    before: bool,
) -> Option<usize> {
    ordinary_cell_insert_index(children, target, before)
}

fn ordinary_grid_insert_index(
    children: &[GridChild],
    target: usize,
    before: bool,
) -> Option<usize> {
    ordinary_cell_insert_index(children, target, before)
}

fn ordinary_cell_insert_index<Child: IndexedCellChild>(
    children: &[Child],
    target: usize,
    before: bool,
) -> Option<usize> {
    let mut seen = 0usize;
    for (child_index, child) in children.iter().enumerate() {
        match child.cell_items() {
            IndexedCellItems::Ordinary(item) if item.cell_body().is_some() => {
                if seen == target {
                    return Some(if before { child_index } else { child_index + 1 });
                }
                seen += 1;
            }
            IndexedCellItems::Group(items) => {
                for item in items {
                    if item.cell_body().is_some() {
                        if seen == target {
                            return None;
                        }
                        seen += 1;
                    }
                }
            }
            IndexedCellItems::Ordinary(_) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::Packed;
    use typst::layout::{GridFooter, GridHeader};
    use typst::model::{TableFooter, TableHeader};
    use typst::text::TextElem;

    fn text(value: &str) -> Content {
        TextElem::packed(value)
    }

    fn table_cell(value: &str) -> TableItem {
        TableItem::Cell(Packed::new(TableCell::new(text(value))))
    }

    fn grid_cell(value: &str) -> GridItem {
        GridItem::Cell(Packed::new(GridCell::new(text(value))))
    }

    #[test]
    fn indexed_table_cells_include_header_body_and_footer() {
        let mut children = vec![
            TableChild::Header(Packed::new(TableHeader::new(vec![table_cell("Header")]))),
            TableChild::Item(table_cell("Body")),
            TableChild::Footer(Packed::new(TableFooter::new(vec![table_cell("Footer")]))),
        ];

        let bodies = table_cell_bodies(&children);
        assert_eq!(
            bodies
                .iter()
                .map(|body| body.plain_text().to_string())
                .collect::<Vec<_>>(),
            ["Header", "Body", "Footer"]
        );

        replace_table_child_cell(&mut children, 2, text("New footer"), &mut 0).unwrap();
        let bodies = table_cell_bodies(&children);
        assert_eq!(bodies[2].plain_text(), "New footer");
        assert_eq!(ordinary_table_insert_index(&children, 0, true), None);
        assert_eq!(ordinary_table_insert_index(&children, 1, false), Some(2));
    }

    #[test]
    fn indexed_grid_cells_include_header_body_and_footer() {
        let mut children = vec![
            GridChild::Header(Packed::new(GridHeader::new(vec![grid_cell("Header")]))),
            GridChild::Item(grid_cell("Body")),
            GridChild::Footer(Packed::new(GridFooter::new(vec![grid_cell("Footer")]))),
        ];

        let bodies = grid_cell_bodies(&children);
        assert_eq!(
            bodies
                .iter()
                .map(|body| body.plain_text().to_string())
                .collect::<Vec<_>>(),
            ["Header", "Body", "Footer"]
        );

        replace_grid_child_cell(&mut children, 2, text("New footer"), &mut 0).unwrap();
        let bodies = grid_cell_bodies(&children);
        assert_eq!(bodies[2].plain_text(), "New footer");
        assert_eq!(ordinary_grid_insert_index(&children, 0, true), None);
        assert_eq!(ordinary_grid_insert_index(&children, 1, false), Some(2));
    }
}
