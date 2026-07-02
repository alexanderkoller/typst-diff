use std::hash::{Hash, Hasher};

use serde::Serialize;
use typst::foundations::Content;

use crate::decision::DecisionEvent;
use crate::diff::PageRegionKind;

#[derive(Clone, Serialize)]
pub struct PipelineTraceEvent {
    pub stage: &'static str,
    pub event: &'static str,
    pub reason: Option<String>,
    pub snapshot_ref: Option<String>,
    pub old_block_index: Option<usize>,
    pub new_block_index: Option<usize>,
    pub old_slot_path: Option<Vec<usize>>,
    pub new_slot_path: Option<Vec<usize>>,
    pub old: Option<TraceContentSummary>,
    pub new: Option<TraceContentSummary>,
    pub similarity: Option<f64>,
    pub threshold: Option<f64>,
    pub selected_edit_kind: Option<String>,
}

impl PipelineTraceEvent {
    pub fn new(stage: &'static str, event: &'static str) -> Self {
        Self {
            stage,
            event,
            reason: None,
            snapshot_ref: None,
            old_block_index: None,
            new_block_index: None,
            old_slot_path: None,
            new_slot_path: None,
            old: None,
            new: None,
            similarity: None,
            threshold: None,
            selected_edit_kind: None,
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn snapshot_ref(mut self, snapshot_ref: impl Into<String>) -> Self {
        self.snapshot_ref = Some(snapshot_ref.into());
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

    pub fn old_slot_path(mut self, path: Vec<usize>) -> Self {
        self.old_slot_path = Some(path);
        self
    }

    pub fn new_slot_path(mut self, path: Vec<usize>) -> Self {
        self.new_slot_path = Some(path);
        self
    }

    pub fn old_content(mut self, content: &Content) -> Self {
        self.old = Some(TraceContentSummary::from_content(content));
        self
    }

    pub fn new_content(mut self, content: &Content) -> Self {
        self.new = Some(TraceContentSummary::from_content(content));
        self
    }

    pub fn similarity(mut self, similarity: f64) -> Self {
        self.similarity = Some(similarity);
        self
    }

    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = Some(threshold);
        self
    }

    pub fn selected_edit_kind(mut self, selected_edit_kind: impl Into<String>) -> Self {
        self.selected_edit_kind = Some(selected_edit_kind.into());
        self
    }
}

#[derive(Clone, Serialize)]
pub struct TraceContentSummary {
    pub content_hash: u64,
    pub kind: String,
    pub plain_text_len: usize,
    pub plain_text_preview: String,
}

impl TraceContentSummary {
    pub fn from_content(content: &Content) -> Self {
        let plain_text = content.plain_text();
        Self {
            content_hash: content_hash(content),
            kind: content.func().name().to_string(),
            plain_text_len: plain_text.chars().count(),
            plain_text_preview: preview_trace_text(plain_text.as_str()),
        }
    }
}

pub trait DebugEventSink {
    fn pipeline_trace_event(&mut self, _event: &PipelineTraceEvent) -> anyhow::Result<()> {
        Ok(())
    }

    fn decision_event(&mut self, _event: &DecisionEvent) -> anyhow::Result<()> {
        Ok(())
    }

    fn rendered_region_trace_start(
        &mut self,
        _trace: &RenderedRegionTraceStart,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn rendered_region_trace_event(&mut self, _event: &FrameTraceEvent) -> anyhow::Result<()> {
        Ok(())
    }

    fn rendered_region_trace_end(&mut self, _trace: &RenderedRegionTraceEnd) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RenderedRegionTraceStart {
    pub trace_id: String,
    pub side: String,
    pub kind: PageRegionKind,
    pub page: usize,
    pub page_width_pt: f64,
    pub page_height_pt: f64,
    pub semantic_region_exists: bool,
}

#[derive(Clone)]
pub struct RenderedRegionTraceEnd {
    pub trace_id: String,
    pub side: String,
    pub kind: PageRegionKind,
    pub page: usize,
    pub extracted_text: String,
    pub event_count: usize,
}

#[derive(Clone)]
pub struct FrameTraceEvent {
    pub trace_id: String,
    pub side: String,
    pub kind: PageRegionKind,
    pub page: usize,
    pub event_index: usize,
    pub frame_path: Vec<usize>,
    pub group_depth: usize,
    pub local_x_pt: f64,
    pub local_y_pt: f64,
    pub absolute_x_pt: f64,
    pub absolute_y_pt: f64,
    pub item_kind: &'static str,
    pub text: Option<String>,
    pub text_len: Option<usize>,
    pub tag_direction: Option<&'static str>,
    pub tag_element: Option<String>,
    pub artifact_depth_before: usize,
    pub artifact_depth_after: usize,
    pub changed_artifact_state: bool,
    pub in_region_band: Option<bool>,
    pub included: Option<bool>,
    pub excluded_reason: Option<&'static str>,
    pub group_origin_before_x_pt: Option<f64>,
    pub group_origin_before_y_pt: Option<f64>,
    pub group_origin_after_x_pt: Option<f64>,
    pub group_origin_after_y_pt: Option<f64>,
    pub group_offset_x_pt: Option<f64>,
    pub group_offset_y_pt: Option<f64>,
}

pub fn emit_pipeline_trace_event(
    debug_events: &mut Option<&mut dyn DebugEventSink>,
    event: PipelineTraceEvent,
) -> anyhow::Result<()> {
    if let Some(sink) = debug_events.as_deref_mut() {
        sink.pipeline_trace_event(&event)?;
    }
    Ok(())
}

fn content_hash(content: &Content) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn preview_trace_text(text: &str) -> String {
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
