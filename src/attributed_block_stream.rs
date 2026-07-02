//! Per-block semantic attribution for the block diff stream.
//!
//! The block matcher works on realized block content, but later edit construction
//! needs the semantic owner and provenance that were available before matching.
//! This module carries that attribution forward as one item per matched block.

use crate::annotated::{AnnotatedContent, SemanticKind, SlotStep};
use typst::foundations::Content;
use typst::model::{FootnoteBody, FootnoteElem};

#[derive(Clone)]
pub(crate) struct AttributedBlockClaim<'a, K> {
    pub(crate) realized: Content,
    pub(crate) owner: Option<&'a AnnotatedContent>,
    pub(crate) fallback_owner: Option<&'a AnnotatedContent>,
    pub(crate) owner_key: Option<K>,
    pub(crate) owner_path: Option<Vec<usize>>,
    pub(crate) equation_origins: Vec<Content>,
}

#[derive(Clone)]
pub(crate) struct AttributedBlock<'a, K> {
    realized: Content,
    owner: Option<&'a AnnotatedContent>,
    fallback_owner: Option<&'a AnnotatedContent>,
    owner_key: Option<K>,
    owner_path: Option<Vec<usize>>,
    owner_semantic_kind: Option<SemanticKind>,
    owner_slot_labels: Vec<SlotStep>,
    owner_has_footnote: bool,
    owner_has_patch_surface: bool,
    footnote_bodies: Vec<Content>,
    equation_origins: Vec<Content>,
}

impl<'a, K> AttributedBlock<'a, K> {
    fn from_claim(claim: AttributedBlockClaim<'a, K>) -> Self {
        let effective_owner = claim.owner.or(claim.fallback_owner);
        let owner_semantic_kind = claim
            .owner
            .or(claim.fallback_owner)
            .and_then(|owner| owner.annotation.semantic_kind.clone());
        let owner_slot_labels = effective_owner
            .map(|owner| {
                owner
                    .annotation
                    .slots
                    .iter()
                    .map(|slot| slot.label.clone())
                    .collect()
            })
            .unwrap_or_default();
        let owner_has_footnote =
            effective_owner.is_some_and(|owner| owner.annotation.footnote.is_some());
        let owner_has_patch_surface =
            effective_owner.is_some_and(|owner| owner.annotation.patch_surface.is_some());
        let footnote_bodies = effective_owner
            .map(footnote_body_contents)
            .unwrap_or_default();
        Self {
            realized: claim.realized,
            owner: claim.owner,
            fallback_owner: claim.fallback_owner,
            owner_key: claim.owner_key,
            owner_path: claim.owner_path,
            owner_semantic_kind,
            owner_slot_labels,
            owner_has_footnote,
            owner_has_patch_surface,
            footnote_bodies,
            equation_origins: claim.equation_origins,
        }
    }

    pub(crate) fn realized(&self) -> &Content {
        &self.realized
    }

    pub(crate) fn owner(&self) -> Option<&'a AnnotatedContent> {
        self.owner
    }

    pub(crate) fn fallback_owner(&self) -> Option<&'a AnnotatedContent> {
        self.fallback_owner
    }

    pub(crate) fn owner_key(&self) -> Option<&K> {
        self.owner_key.as_ref()
    }

    pub(crate) fn cloned_owner_key(&self) -> Option<K>
    where
        K: Clone,
    {
        self.owner_key.clone()
    }

    pub(crate) fn owner_path(&self) -> Option<&[usize]> {
        self.owner_path.as_deref()
    }

    pub(crate) fn owner_semantic_kind(&self) -> Option<&SemanticKind> {
        self.owner_semantic_kind.as_ref()
    }

    pub(crate) fn owner_slot_labels(&self) -> &[SlotStep] {
        &self.owner_slot_labels
    }

    pub(crate) fn owner_has_footnote(&self) -> bool {
        self.owner_has_footnote
    }

    pub(crate) fn owner_has_patch_surface(&self) -> bool {
        self.owner_has_patch_surface
    }

    pub(crate) fn footnote_bodies(&self) -> &[Content] {
        &self.footnote_bodies
    }

    pub(crate) fn equation_origins(&self) -> &[Content] {
        &self.equation_origins
    }
}

pub(crate) struct AttributedBlockStream<'a, K> {
    items: Vec<AttributedBlock<'a, K>>,
}

impl<'a, K> AttributedBlockStream<'a, K> {
    pub(crate) fn from_claims(
        claims: impl IntoIterator<Item = AttributedBlockClaim<'a, K>>,
    ) -> Self {
        Self {
            items: claims
                .into_iter()
                .map(AttributedBlock::from_claim)
                .collect(),
        }
    }

    pub(crate) fn get(&self, index: usize) -> Option<&AttributedBlock<'a, K>> {
        self.items.get(index)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &AttributedBlock<'a, K>> {
        self.items.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

fn footnote_body_contents(root: &AnnotatedContent) -> Vec<Content> {
    let mut out = Vec::new();
    collect_footnote_body_contents(root, &mut out);
    out
}

fn collect_footnote_body_contents(node: &AnnotatedContent, out: &mut Vec<Content>) {
    let slots = node
        .annotation
        .slots
        .iter()
        .filter(|slot| matches!(slot.label, SlotStep::FootnoteBody))
        .collect::<Vec<_>>();
    if !slots.is_empty() {
        for slot in slots {
            if let Some(body) = node.get_path(&slot.path) {
                push_unique_content(out, body.realized.clone());
            }
        }
        return;
    }

    if let Some(footnote) = &node.annotation.footnote
        && let Some(body) = footnote_body_content(&footnote.body)
    {
        push_unique_content(out, body);
    }
    for child in &node.children {
        collect_footnote_body_contents(child, out);
    }
}

fn footnote_body_content(content: &Content) -> Option<Content> {
    let footnote = content.to_packed::<FootnoteElem>()?;
    let FootnoteBody::Content(body) = &footnote.body else {
        return None;
    };
    Some(body.clone())
}

fn push_unique_content(out: &mut Vec<Content>, content: Content) {
    if !out.iter().any(|existing| *existing == content) {
        out.push(content);
    }
}
