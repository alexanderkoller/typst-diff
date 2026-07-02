//! Per-block semantic attribution for the block diff stream.
//!
//! The block matcher works on realized block content, but later edit construction
//! needs the semantic owner and provenance that were available before matching.
//! This module carries that attribution forward as one item per matched block.

use crate::annotated::{AnnotatedContent, SemanticKind, SlotStep};
use typst::foundations::Content;

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

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}
