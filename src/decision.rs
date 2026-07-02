//! Lightweight decision and fallback identifiers.
//!
//! This module is intentionally small: it names fallback debt without retaining
//! Typst content trees or reconstructing decisions after the fact. Call sites
//! should emit these codes at the point where a fallback decision is made.

use std::collections::BTreeMap;
use std::io::Write;

use anyhow::Result;
use serde::Serialize;

const MAX_EXAMPLES_PER_CODE: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub enum FallbackCode {
    PositionalSequencePairing,
    ContextVisibleTextPairing,
    VisibleTextOwnerBlockMatching,
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

    pub const fn explanation(self) -> &'static str {
        match self {
            Self::PositionalSequencePairing => {
                "paired source and realized sequence children by position"
            }
            Self::ContextVisibleTextPairing => "paired context output by visible text",
            Self::VisibleTextOwnerBlockMatching => "matched owner/block by visible text",
            Self::UniqueWrapperBodyRecovery => "recovered wrapper body through a unique wrapper",
            Self::UniquePartialItemContainerMapping => {
                "mapped a partial item container through a unique visible item"
            }
            Self::AnonymousOpaquePreSurfaceGrafting => {
                "grafted an anonymous pre-realization opaque surface"
            }
            Self::WordDiffOrOpaqueReplacementLadder => {
                "selected word diff or opaque replacement after structural routes failed"
            }
            Self::BroadEmptyBlockEquationCarrierRecognition => {
                "recognized an empty block as an equation carrier"
            }
            Self::FootnoteMarkerMatchingByVisibleNumber => {
                "matched footnote marker/body by visible number"
            }
            Self::RenderedRegionSourceStringAlignParsing => {
                "parsed rendered-region wrapper from source text"
            }
            Self::GeneratedTypstSnippetPanicPath => {
                "built generated Typst snippets for rendered-region output"
            }
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

#[derive(Clone, Debug, Serialize)]
pub struct DecisionEvent {
    pub phase: &'static str,
    pub code: FallbackCode,
    pub warning_code: &'static str,
    pub explanation: &'static str,
    pub block_index: Option<usize>,
    pub old_block_index: Option<usize>,
    pub new_block_index: Option<usize>,
    pub path: Option<Vec<usize>>,
    pub preview: Option<String>,
}

impl DecisionEvent {
    pub fn fallback(phase: &'static str, code: FallbackCode) -> Self {
        Self {
            phase,
            code,
            warning_code: code.label(),
            explanation: code.explanation(),
            block_index: None,
            old_block_index: None,
            new_block_index: None,
            path: None,
            preview: None,
        }
    }

    pub fn block_index(mut self, index: usize) -> Self {
        self.block_index = Some(index);
        self
    }

    pub fn old_block_index(mut self, index: usize) -> Self {
        self.old_block_index = Some(index);
        self
    }

    pub fn new_block_index(mut self, index: usize) -> Self {
        self.new_block_index = Some(index);
        self
    }

    pub fn path(mut self, path: Vec<usize>) -> Self {
        self.path = Some(path);
        self
    }

    pub fn preview(mut self, preview: impl Into<String>) -> Self {
        let preview = bounded_preview(&preview.into());
        if !preview.is_empty() {
            self.preview = Some(preview);
        }
        self
    }

    pub fn compact_message(&self) -> String {
        let mut message = format!(
            "typst-diff fallback warning [{}] phase={} {}",
            self.warning_code, self.phase, self.explanation
        );
        if let Some(index) = self.block_index {
            message.push_str(&format!(" block={index}"));
        }
        if let Some(index) = self.old_block_index {
            message.push_str(&format!(" old_block={index}"));
        }
        if let Some(index) = self.new_block_index {
            message.push_str(&format!(" new_block={index}"));
        }
        if let Some(path) = &self.path {
            message.push_str(&format!(" path={path:?}"));
        }
        if let Some(preview) = &self.preview {
            message.push_str(&format!(" preview={preview:?}"));
        }
        message
    }
}

pub trait DecisionSink {
    fn fallback_decision(&mut self, event: DecisionEvent) -> Result<()>;
}

#[derive(Default, Serialize)]
pub struct FallbackWarningsDocument {
    pub schema_version: u32,
    pub total_count: usize,
    pub warnings: Vec<FallbackWarningSummary>,
}

#[derive(Clone, Serialize)]
pub struct FallbackWarningSummary {
    pub warning_code: &'static str,
    pub count: usize,
    pub trace_event: &'static str,
    pub examples: Vec<DecisionEvent>,
}

#[derive(Default)]
pub struct DecisionRecorder {
    events: Vec<DecisionEvent>,
}

impl DecisionRecorder {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn fallback_warnings_document(&self) -> FallbackWarningsDocument {
        let mut grouped: BTreeMap<&'static str, FallbackWarningSummary> = BTreeMap::new();
        for event in &self.events {
            let summary =
                grouped
                    .entry(event.warning_code)
                    .or_insert_with(|| FallbackWarningSummary {
                        warning_code: event.warning_code,
                        count: 0,
                        trace_event: "decision_event",
                        examples: Vec::new(),
                    });
            summary.count += 1;
            if summary.examples.len() < MAX_EXAMPLES_PER_CODE {
                summary.examples.push(event.clone());
            }
        }
        FallbackWarningsDocument {
            schema_version: 1,
            total_count: self.events.len(),
            warnings: grouped.into_values().collect(),
        }
    }

    pub fn emit_stderr_warnings(&self, quiet: bool, mut stderr: impl Write) -> Result<()> {
        if quiet || self.events.is_empty() {
            return Ok(());
        }
        for event in self.events.iter().take(10) {
            writeln!(stderr, "{}", event.compact_message())?;
        }
        if self.events.len() > 10 {
            writeln!(
                stderr,
                "typst-diff fallback warning summary: {} additional fallback decisions suppressed; rerun with --debug for counts or --debug-trace for JSONL events",
                self.events.len() - 10
            )?;
        }
        Ok(())
    }
}

impl DecisionSink for DecisionRecorder {
    fn fallback_decision(&mut self, event: DecisionEvent) -> Result<()> {
        self.events.push(event);
        Ok(())
    }
}

pub fn bounded_preview(text: &str) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ch in single_line.chars().take(120) {
        out.push(ch);
    }
    if single_line.chars().count() > 120 {
        out.push_str("...");
    }
    out
}
