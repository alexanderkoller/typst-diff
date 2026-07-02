//! Lightweight decision and fallback identifiers.
//!
//! This module is intentionally small: it names fallback debt without retaining
//! Typst content trees or reconstructing decisions after the fact. Call sites
//! should emit these codes at the point where a fallback decision is made.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub enum FallbackCode {
    PositionalSequencePairing,
    ContextVisibleTextPairing,
    VisibleTextOwnerBlockMatching,
    UniqueChangedSlotPair,
    SlotBearingDescendantPair,
    DuplicateEditPruningByTextSignature,
    UniqueWrapperBodyRecovery,
    UniquePartialItemContainerMapping,
    AnonymousOpaquePreSurfaceGrafting,
    WordDiffOrOpaqueReplacementLadder,
    BroadEmptyBlockEquationCarrierRecognition,
    FootnoteMarkerMatchingByVisibleNumber,
    RenderedRegionSourceStringAlignParsing,
    GeneratedTypstSnippetPanicPath,
}

impl FallbackCode {
    pub const ALL: &'static [Self] = &[
        Self::PositionalSequencePairing,
        Self::ContextVisibleTextPairing,
        Self::VisibleTextOwnerBlockMatching,
        Self::UniqueChangedSlotPair,
        Self::SlotBearingDescendantPair,
        Self::DuplicateEditPruningByTextSignature,
        Self::UniqueWrapperBodyRecovery,
        Self::UniquePartialItemContainerMapping,
        Self::AnonymousOpaquePreSurfaceGrafting,
        Self::WordDiffOrOpaqueReplacementLadder,
        Self::BroadEmptyBlockEquationCarrierRecognition,
        Self::FootnoteMarkerMatchingByVisibleNumber,
        Self::RenderedRegionSourceStringAlignParsing,
        Self::GeneratedTypstSnippetPanicPath,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::PositionalSequencePairing => "FB-001-positional-sequence-pairing",
            Self::ContextVisibleTextPairing => "FB-002-context-visible-text-pairing",
            Self::VisibleTextOwnerBlockMatching => "FB-003-visible-text-owner-block-matching",
            Self::UniqueChangedSlotPair => "FB-004-unique-changed-slot-pair",
            Self::SlotBearingDescendantPair => "FB-005-slot-bearing-descendant-pair",
            Self::DuplicateEditPruningByTextSignature => {
                "FB-006-duplicate-edit-pruning-by-text-signature"
            }
            Self::UniqueWrapperBodyRecovery => "FB-007-unique-wrapper-body-recovery",
            Self::UniquePartialItemContainerMapping => {
                "FB-008-unique-partial-item-container-mapping"
            }
            Self::AnonymousOpaquePreSurfaceGrafting => {
                "FB-009-anonymous-opaque-pre-surface-grafting"
            }
            Self::WordDiffOrOpaqueReplacementLadder => {
                "FB-010-word-diff-or-opaque-replacement-ladder"
            }
            Self::BroadEmptyBlockEquationCarrierRecognition => {
                "FB-011-broad-empty-block-equation-carrier-recognition"
            }
            Self::FootnoteMarkerMatchingByVisibleNumber => {
                "FB-012-footnote-marker-matching-by-visible-number"
            }
            Self::RenderedRegionSourceStringAlignParsing => {
                "FB-013-rendered-region-source-string-align-parsing"
            }
            Self::GeneratedTypstSnippetPanicPath => "FB-014-generated-typst-snippet-panic-path",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub enum DecisionProof {
    ExactPath,
    RecordedContext,
    SemanticOwner,
    ContainerSlot,
    StyleContext,
    RenderedTag,
    OpaqueVisualCarrier,
    Unsupported,
}

