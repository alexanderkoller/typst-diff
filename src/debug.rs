use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use typst::foundations::{Content, SequenceElem, StyledElem};
use typst::layout::{BlockBody, BlockElem};
use typst::model::ParElem;
use typst::syntax::Span;

use crate::annotated::{AnnotatedContent, SemanticKind, SemanticSlot, SlotStep, WrapperKind};
use crate::diff::{
    BlockOp, DiffBlock, DiffBlockDebug, DiffResult, EditContent, PageRegionKind, RealizedEdit,
    RegionPath, RenderedRegionAlignment, RenderedRegionSegmentEdit, RenderedRegionWrapper, WordOp,
};
use crate::trace::{
    DebugEventSink, FrameTraceEvent, PipelineTraceEvent, RenderedRegionTraceEnd,
    RenderedRegionTraceStart,
};

const SCHEMA_VERSION: u32 = 2;
const PREVIEW_CHARS: usize = 120;
const CONTENT_DEPTH_LIMIT: usize = 32;

pub struct DebugBundle<'a> {
    pub build_line: &'a str,
    pub args: DebugArgs,
    pub old_input: &'a Path,
    pub new_input: &'a Path,
    pub output: &'a Path,
    pub debug_dir: &'a Path,
    pub old_eval: &'a crate::eval::EvalDebug,
    pub new_eval: &'a crate::eval::EvalDebug,
    pub block_debug: &'a DiffBlockDebug,
    pub diff_result: &'a DiffResult,
    pub annotated_output: &'a Content,
    pub trace_files: Vec<DebugTraceFile>,
}

#[derive(Clone)]
pub struct DebugArgs {
    pub old_or_file: PathBuf,
    pub new: Option<PathBuf>,
    pub revision: Option<String>,
    pub output: PathBuf,
    pub log_modifications: Option<PathBuf>,
    pub compact_substitutions: bool,
    pub debug: bool,
    pub debug_trace: bool,
}

#[derive(Clone)]
pub struct DebugTraceFile {
    pub path: PathBuf,
    pub format: &'static str,
    pub present: bool,
}

pub fn default_debug_dir(output: &Path) -> PathBuf {
    output.with_extension("debug")
}

pub fn rendered_region_frame_traces_path(debug_dir: &Path) -> PathBuf {
    debug_dir.join("diff/rendered-region-frame-traces.jsonl")
}

pub fn pipeline_events_path(debug_dir: &Path) -> PathBuf {
    debug_dir.join("diff/pipeline-events.jsonl")
}

pub fn write_debug_bundle(bundle: &DebugBundle<'_>) -> Result<()> {
    fs::create_dir_all(bundle.debug_dir)
        .with_context(|| format!("failed to create debug directory {:?}", bundle.debug_dir))?;
    for subdir in ["old", "new", "diff", "output"] {
        fs::create_dir_all(bundle.debug_dir.join(subdir))
            .with_context(|| format!("failed to create debug subdirectory {subdir:?}"))?;
    }

    write_yaml(bundle.debug_dir.join("manifest.yml"), &manifest(bundle))?;
    write_yaml(
        bundle.debug_dir.join("old/raw-eval.yml"),
        &content_document("old raw eval", &bundle.old_eval.raw),
    )?;
    write_yaml(
        bundle.debug_dir.join("old/normalized.yml"),
        &content_document("old normalized", &bundle.old_eval.normalized),
    )?;
    write_yaml(
        bundle.debug_dir.join("old/realized-tree.yml"),
        &annotated_document("old realized tree", &bundle.old_eval.annotated),
    )?;
    write_yaml(
        bundle.debug_dir.join("old/blocks.yml"),
        &blocks_document("old blocks", &bundle.block_debug.old_blocks),
    )?;
    write_yaml(
        bundle.debug_dir.join("new/raw-eval.yml"),
        &content_document("new raw eval", &bundle.new_eval.raw),
    )?;
    write_yaml(
        bundle.debug_dir.join("new/normalized.yml"),
        &content_document("new normalized", &bundle.new_eval.normalized),
    )?;
    write_yaml(
        bundle.debug_dir.join("new/realized-tree.yml"),
        &annotated_document("new realized tree", &bundle.new_eval.annotated),
    )?;
    write_yaml(
        bundle.debug_dir.join("new/blocks.yml"),
        &blocks_document("new blocks", &bundle.block_debug.new_blocks),
    )?;
    write_yaml(
        bundle.debug_dir.join("diff/block-raw.yml"),
        &block_ops_document("raw block ops", &bundle.block_debug.raw_ops),
    )?;
    write_yaml(
        bundle.debug_dir.join("diff/block-matched.yml"),
        &block_ops_document("matched block ops", &bundle.block_debug.matched_ops),
    )?;
    write_yaml(
        bundle.debug_dir.join("diff/final-edits.yml"),
        &diff_result_document(bundle.diff_result),
    )?;
    write_yaml(
        bundle.debug_dir.join("diff/rendered-regions.yml"),
        &rendered_regions_document(bundle.diff_result),
    )?;
    write_yaml(
        bundle.debug_dir.join("output/annotated-content.yml"),
        &content_document("annotated output content", bundle.annotated_output),
    )?;
    fs::write(
        bundle.debug_dir.join("output/modification-log.txt"),
        bundle.diff_result.modification_log(),
    )
    .with_context(|| "failed to write debug modification log")?;
    Ok(())
}

pub struct JsonlTraceWriter {
    debug_dir: PathBuf,
    pipeline_writer: BufWriter<fs::File>,
    frame_writer: Option<BufWriter<fs::File>>,
    next_seq: u64,
    rendered_region_frame_trace_created: bool,
}

impl JsonlTraceWriter {
    pub fn create(debug_dir: &Path) -> Result<Self> {
        fs::create_dir_all(debug_dir.join("diff"))
            .with_context(|| format!("failed to create debug diff directory {:?}", debug_dir))?;
        let path = pipeline_events_path(debug_dir);
        let file = fs::File::create(&path)
            .with_context(|| format!("failed to create debug JSONL trace {:?}", path))?;
        let mut writer = Self {
            debug_dir: debug_dir.to_path_buf(),
            pipeline_writer: BufWriter::new(file),
            frame_writer: None,
            next_seq: 0,
            rendered_region_frame_trace_created: false,
        };
        let seq = writer.next_seq();
        write_jsonl(
            &mut writer.pipeline_writer,
            &JsonlSchemaRecord {
                schema_version: SCHEMA_VERSION,
                record: "schema",
                seq,
                format: "typst-diff-pipeline-events",
            },
        )?;
        Ok(writer)
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    fn ensure_frame_writer(&mut self) -> Result<&mut BufWriter<fs::File>> {
        if self.frame_writer.is_none() {
            let path = rendered_region_frame_traces_path(&self.debug_dir);
            let file = fs::File::create(&path)
                .with_context(|| format!("failed to create debug JSONL trace {:?}", path))?;
            let mut frame_writer = BufWriter::new(file);
            let seq = self.next_seq();
            write_jsonl(
                &mut frame_writer,
                &JsonlSchemaRecord {
                    schema_version: SCHEMA_VERSION,
                    record: "schema",
                    seq,
                    format: "typst-diff-rendered-region-frame-trace",
                },
            )?;
            self.frame_writer = Some(frame_writer);
            self.rendered_region_frame_trace_created = true;
        }
        Ok(self
            .frame_writer
            .as_mut()
            .expect("frame writer just created"))
    }

    pub fn finish(mut self) -> Result<Vec<DebugTraceFile>> {
        self.pipeline_writer
            .flush()
            .context("failed to flush pipeline JSONL trace")?;
        if let Some(mut frame_writer) = self.frame_writer.take() {
            frame_writer
                .flush()
                .context("failed to flush rendered-region JSONL trace")?;
        }
        Ok(vec![
            DebugTraceFile {
                path: pipeline_events_path(&self.debug_dir),
                format: "typst-diff-pipeline-events",
                present: true,
            },
            DebugTraceFile {
                path: rendered_region_frame_traces_path(&self.debug_dir),
                format: "typst-diff-rendered-region-frame-trace",
                present: self.rendered_region_frame_trace_created,
            },
        ])
    }
}

fn write_jsonl(writer: &mut BufWriter<fs::File>, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(writer.by_ref(), value)
        .context("failed to serialize debug JSONL trace record")?;
    writer
        .write_all(b"\n")
        .context("failed to write debug JSONL trace newline")?;
    Ok(())
}

impl DebugEventSink for JsonlTraceWriter {
    fn pipeline_trace_event(&mut self, event: &PipelineTraceEvent) -> Result<()> {
        let seq = self.next_seq();
        write_jsonl(
            &mut self.pipeline_writer,
            &PipelineTraceRecord {
                schema_version: SCHEMA_VERSION,
                record: "pipeline_event",
                seq,
                event,
            },
        )
    }

    fn rendered_region_trace_start(&mut self, trace: &RenderedRegionTraceStart) -> Result<()> {
        self.ensure_frame_writer()?;
        let seq = self.next_seq();
        let record = TraceStartRecord {
            schema_version: SCHEMA_VERSION,
            record: "rendered_region_trace_start",
            seq,
            trace_id: trace.trace_id.clone(),
            side: trace.side.clone(),
            region_kind: page_region_label(trace.kind),
            page: trace.page,
            page_width_pt: trace.page_width_pt,
            page_height_pt: trace.page_height_pt,
            semantic_region_exists: trace.semantic_region_exists,
        };
        let writer = self.ensure_frame_writer()?;
        write_jsonl(writer, &record)
    }

    fn rendered_region_trace_event(&mut self, event: &FrameTraceEvent) -> Result<()> {
        self.ensure_frame_writer()?;
        let seq = self.next_seq();
        let record = TraceEventRecord {
            schema_version: SCHEMA_VERSION,
            record: "rendered_region_trace_event",
            seq,
            trace_id: event.trace_id.clone(),
            side: event.side.clone(),
            region_kind: page_region_label(event.kind),
            page: event.page,
            event: summarize_frame_trace_event(event),
        };
        let writer = self.ensure_frame_writer()?;
        write_jsonl(writer, &record)
    }

    fn rendered_region_trace_end(&mut self, trace: &RenderedRegionTraceEnd) -> Result<()> {
        self.ensure_frame_writer()?;
        let seq = self.next_seq();
        let record = TraceEndRecord {
            schema_version: SCHEMA_VERSION,
            record: "rendered_region_trace_end",
            seq,
            trace_id: trace.trace_id.clone(),
            side: trace.side.clone(),
            region_kind: page_region_label(trace.kind),
            page: trace.page,
            extracted_text: trace.extracted_text.clone(),
            event_count: trace.event_count,
        };
        let writer = self.ensure_frame_writer()?;
        write_jsonl(writer, &record)
    }
}

#[derive(Serialize)]
struct JsonlSchemaRecord {
    schema_version: u32,
    record: &'static str,
    seq: u64,
    format: &'static str,
}

#[derive(Serialize)]
struct PipelineTraceRecord<'a> {
    schema_version: u32,
    record: &'static str,
    seq: u64,
    #[serde(flatten)]
    event: &'a PipelineTraceEvent,
}

#[derive(Serialize)]
struct TraceStartRecord {
    schema_version: u32,
    record: &'static str,
    seq: u64,
    trace_id: String,
    side: String,
    region_kind: String,
    page: usize,
    page_width_pt: f64,
    page_height_pt: f64,
    semantic_region_exists: bool,
}

#[derive(Serialize)]
struct TraceEventRecord {
    schema_version: u32,
    record: &'static str,
    seq: u64,
    trace_id: String,
    side: String,
    region_kind: String,
    page: usize,
    event: FrameTraceEventSummary,
}

#[derive(Serialize)]
struct TraceEndRecord {
    schema_version: u32,
    record: &'static str,
    seq: u64,
    trace_id: String,
    side: String,
    region_kind: String,
    page: usize,
    extracted_text: String,
    event_count: usize,
}

fn write_yaml(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let yaml = serde_yaml::to_string(value)
        .with_context(|| format!("failed to serialize debug YAML {:?}", path))?;
    fs::write(&path, yaml).with_context(|| format!("failed to write debug YAML {:?}", path))
}

#[derive(Serialize)]
struct Manifest {
    schema_version: u32,
    generated_by: String,
    args: ManifestArgs,
    resolved_inputs: ResolvedInputs,
    output_pdf: String,
    debug_dir: String,
    trace_files: Vec<ManifestTraceFile>,
}

#[derive(Serialize)]
struct ManifestArgs {
    old_or_file: String,
    new: Option<String>,
    revision: Option<String>,
    output: String,
    log_modifications: Option<String>,
    compact_substitutions: bool,
    debug: bool,
    debug_trace: bool,
}

#[derive(Serialize)]
struct ResolvedInputs {
    old: String,
    new: String,
}

#[derive(Serialize)]
struct ManifestTraceFile {
    path: String,
    format: String,
    present: bool,
}

fn manifest(bundle: &DebugBundle<'_>) -> Manifest {
    Manifest {
        schema_version: SCHEMA_VERSION,
        generated_by: bundle.build_line.to_string(),
        args: ManifestArgs {
            old_or_file: path_string(&bundle.args.old_or_file),
            new: bundle.args.new.as_ref().map(|path| path_string(path)),
            revision: bundle.args.revision.clone(),
            output: path_string(&bundle.args.output),
            log_modifications: bundle
                .args
                .log_modifications
                .as_ref()
                .map(|path| path_string(path)),
            compact_substitutions: bundle.args.compact_substitutions,
            debug: bundle.args.debug,
            debug_trace: bundle.args.debug_trace,
        },
        resolved_inputs: ResolvedInputs {
            old: path_string(bundle.old_input),
            new: path_string(bundle.new_input),
        },
        output_pdf: path_string(bundle.output),
        debug_dir: path_string(bundle.debug_dir),
        trace_files: bundle
            .trace_files
            .iter()
            .map(|file| ManifestTraceFile {
                path: path_string(&file.path),
                format: file.format.to_string(),
                present: file.present,
            })
            .collect(),
    }
}

#[derive(Serialize)]
struct ContentDocument {
    schema_version: u32,
    stage: String,
    stats: ContentStats,
    root: ContentSummary,
}

fn content_document(stage: &str, content: &Content) -> ContentDocument {
    ContentDocument {
        schema_version: SCHEMA_VERSION,
        stage: stage.to_string(),
        stats: content_stats(content),
        root: summarize_content(content, 0),
    }
}

#[derive(Serialize)]
struct AnnotatedDocument {
    schema_version: u32,
    stage: String,
    stats: AnnotatedStats,
    root: AnnotatedSummary,
}

fn annotated_document(stage: &str, root: &AnnotatedContent) -> AnnotatedDocument {
    AnnotatedDocument {
        schema_version: SCHEMA_VERSION,
        stage: stage.to_string(),
        stats: annotated_stats(root),
        root: summarize_annotated(root),
    }
}

#[derive(Serialize)]
struct BlocksDocument {
    schema_version: u32,
    stage: String,
    count: usize,
    blocks: Vec<BlockSummary>,
}

fn blocks_document(stage: &str, blocks: &[DiffBlock]) -> BlocksDocument {
    BlocksDocument {
        schema_version: SCHEMA_VERSION,
        stage: stage.to_string(),
        count: blocks.len(),
        blocks: blocks
            .iter()
            .enumerate()
            .map(|(index, block)| summarize_block(index, block))
            .collect(),
    }
}

#[derive(Serialize)]
struct BlockOpsDocument {
    schema_version: u32,
    stage: String,
    count: usize,
    ops: Vec<BlockOpSummary>,
}

fn block_ops_document(stage: &str, ops: &[BlockOp]) -> BlockOpsDocument {
    BlockOpsDocument {
        schema_version: SCHEMA_VERSION,
        stage: stage.to_string(),
        count: ops.len(),
        ops: ops.iter().map(summarize_block_op).collect(),
    }
}

#[derive(Serialize)]
struct DiffResultDocument {
    schema_version: u32,
    block_count: usize,
    changed_block_count: usize,
    root_page_style_count: usize,
    region_count: usize,
    rendered_region_count: usize,
    blocks: Vec<DiffBlockEditSummary>,
    regions: Vec<RegionSummary>,
    rendered_regions: Vec<RenderedRegionSummary>,
}

fn diff_result_document(result: &DiffResult) -> DiffResultDocument {
    DiffResultDocument {
        schema_version: SCHEMA_VERSION,
        block_count: result.blocks.len(),
        changed_block_count: result
            .blocks
            .iter()
            .filter(|block| !block.edits.is_empty())
            .count(),
        root_page_style_count: result.root_styles.iter().count(),
        region_count: result.regions.len(),
        rendered_region_count: result.rendered_regions.len(),
        blocks: result
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| DiffBlockEditSummary {
                index,
                page_style_count: block.page_styles.iter().count(),
                base: summarize_annotated_shallow(&block.base),
                edits: block.edits.iter().map(summarize_realized_edit).collect(),
            })
            .collect(),
        regions: result
            .regions
            .iter()
            .map(|region| RegionSummary {
                path: region_path_label(region.path),
                base: summarize_annotated_shallow(&region.base),
                edits: region.edits.iter().map(summarize_realized_edit).collect(),
            })
            .collect(),
        rendered_regions: result
            .rendered_regions
            .iter()
            .map(summarize_rendered_region)
            .collect(),
    }
}

#[derive(Serialize)]
struct RenderedRegionsDocument {
    schema_version: u32,
    rendered_region_count: usize,
    rendered_regions: Vec<RenderedRegionSummary>,
}

fn rendered_regions_document(result: &DiffResult) -> RenderedRegionsDocument {
    RenderedRegionsDocument {
        schema_version: SCHEMA_VERSION,
        rendered_region_count: result.rendered_regions.len(),
        rendered_regions: result
            .rendered_regions
            .iter()
            .map(summarize_rendered_region)
            .collect(),
    }
}

#[derive(Serialize)]
struct FrameTraceEventSummary {
    event_index: usize,
    frame_path: Vec<usize>,
    group_depth: usize,
    local_x_pt: f64,
    local_y_pt: f64,
    absolute_x_pt: f64,
    absolute_y_pt: f64,
    item_kind: String,
    text: Option<String>,
    text_len: Option<usize>,
    tag_direction: Option<String>,
    tag_element: Option<String>,
    artifact_depth_before: usize,
    artifact_depth_after: usize,
    changed_artifact_state: bool,
    in_region_band: Option<bool>,
    included: Option<bool>,
    excluded_reason: Option<String>,
    group_origin_before_x_pt: Option<f64>,
    group_origin_before_y_pt: Option<f64>,
    group_origin_after_x_pt: Option<f64>,
    group_origin_after_y_pt: Option<f64>,
    group_offset_x_pt: Option<f64>,
    group_offset_y_pt: Option<f64>,
}

fn summarize_frame_trace_event(event: &FrameTraceEvent) -> FrameTraceEventSummary {
    FrameTraceEventSummary {
        event_index: event.event_index,
        frame_path: event.frame_path.clone(),
        group_depth: event.group_depth,
        local_x_pt: event.local_x_pt,
        local_y_pt: event.local_y_pt,
        absolute_x_pt: event.absolute_x_pt,
        absolute_y_pt: event.absolute_y_pt,
        item_kind: event.item_kind.to_string(),
        text: event.text.clone(),
        text_len: event.text_len,
        tag_direction: event.tag_direction.map(str::to_string),
        tag_element: event.tag_element.clone(),
        artifact_depth_before: event.artifact_depth_before,
        artifact_depth_after: event.artifact_depth_after,
        changed_artifact_state: event.changed_artifact_state,
        in_region_band: event.in_region_band,
        included: event.included,
        excluded_reason: event.excluded_reason.map(str::to_string),
        group_origin_before_x_pt: event.group_origin_before_x_pt,
        group_origin_before_y_pt: event.group_origin_before_y_pt,
        group_origin_after_x_pt: event.group_origin_after_x_pt,
        group_origin_after_y_pt: event.group_origin_after_y_pt,
        group_offset_x_pt: event.group_offset_x_pt,
        group_offset_y_pt: event.group_offset_y_pt,
    }
}

#[derive(Serialize)]
struct ContentStats {
    node_count: usize,
    text_len: usize,
    token_count: usize,
}

fn content_stats(content: &Content) -> ContentStats {
    let mut node_count = 0;
    let _ = content.traverse::<_, ()>(&mut |_| {
        node_count += 1;
        std::ops::ControlFlow::Continue(())
    });
    ContentStats {
        node_count,
        text_len: content.plain_text().chars().count(),
        token_count: crate::diff::extract_words(content).len(),
    }
}

#[derive(Serialize)]
struct AnnotatedStats {
    node_count: usize,
    semantic_node_count: usize,
    slot_count: usize,
}

fn annotated_stats(root: &AnnotatedContent) -> AnnotatedStats {
    fn walk(node: &AnnotatedContent, stats: &mut AnnotatedStats) {
        stats.node_count += 1;
        if node.annotation.semantic_kind.is_some() {
            stats.semantic_node_count += 1;
        }
        stats.slot_count += node.annotation.slots.len();
        for child in &node.children {
            walk(child, stats);
        }
    }
    let mut stats = AnnotatedStats {
        node_count: 0,
        semantic_node_count: 0,
        slot_count: 0,
    };
    walk(root, &mut stats);
    stats
}

#[derive(Serialize)]
struct ContentSummary {
    kind: String,
    content_hash: u64,
    plain_text_len: usize,
    plain_text: String,
    plain_text_preview: String,
    child_count: usize,
    source_span: bool,
    children_omitted: bool,
    children: Vec<ContentSummary>,
}

fn summarize_content(content: &Content, depth: usize) -> ContentSummary {
    let children = content_children(content);
    let children_omitted = depth >= CONTENT_DEPTH_LIMIT && !children.is_empty();
    ContentSummary {
        kind: content_kind(content),
        content_hash: content_hash(content),
        plain_text_len: content.plain_text().chars().count(),
        plain_text: content.plain_text().to_string(),
        plain_text_preview: preview(content.plain_text().as_str()),
        child_count: children.len(),
        source_span: has_source_span(content.span()),
        children_omitted,
        children: if children_omitted {
            Vec::new()
        } else {
            children
                .iter()
                .map(|child| summarize_content(child, depth + 1))
                .collect()
        },
    }
}

fn content_children(content: &Content) -> Vec<Content> {
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
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(Default::default())
    {
        return vec![body];
    }
    Vec::new()
}

#[derive(Serialize)]
struct AnnotatedSummary {
    path: Vec<usize>,
    content: ContentSummary,
    semantic_kind: Option<String>,
    slot_count: usize,
    slots: Vec<SlotSummary>,
    footnote: bool,
    patch_surface: bool,
    equation_origin_count: usize,
    child_count: usize,
    children: Vec<AnnotatedSummary>,
}

fn summarize_annotated(node: &AnnotatedContent) -> AnnotatedSummary {
    summarize_annotated_at(node, Vec::new())
}

fn summarize_annotated_at(node: &AnnotatedContent, path: Vec<usize>) -> AnnotatedSummary {
    AnnotatedSummary {
        path: path.clone(),
        content: summarize_content(&node.realized, 0),
        semantic_kind: node
            .annotation
            .semantic_kind
            .as_ref()
            .map(semantic_kind_label),
        slot_count: node.annotation.slots.len(),
        slots: node.annotation.slots.iter().map(summarize_slot).collect(),
        footnote: node.annotation.footnote.is_some(),
        patch_surface: node.annotation.patch_surface.is_some(),
        equation_origin_count: node.annotation.equation_origins.len(),
        child_count: node.children.len(),
        children: node
            .children
            .iter()
            .enumerate()
            .map(|(index, child)| {
                let mut child_path = path.clone();
                child_path.push(index);
                summarize_annotated_at(child, child_path)
            })
            .collect(),
    }
}

fn summarize_annotated_shallow(node: &AnnotatedContent) -> AnnotatedSummary {
    AnnotatedSummary {
        path: Vec::new(),
        content: summarize_content(&node.realized, 0),
        semantic_kind: node
            .annotation
            .semantic_kind
            .as_ref()
            .map(semantic_kind_label),
        slot_count: node.annotation.slots.len(),
        slots: node.annotation.slots.iter().map(summarize_slot).collect(),
        footnote: node.annotation.footnote.is_some(),
        patch_surface: node.annotation.patch_surface.is_some(),
        equation_origin_count: node.annotation.equation_origins.len(),
        child_count: node.children.len(),
        children: Vec::new(),
    }
}

#[derive(Serialize)]
struct SlotSummary {
    label: String,
    path: Vec<usize>,
}

fn summarize_slot(slot: &SemanticSlot) -> SlotSummary {
    SlotSummary {
        label: slot_label(&slot.label),
        path: slot.path.clone(),
    }
}

#[derive(Serialize)]
struct BlockSummary {
    index: usize,
    page_style_count: usize,
    content: ContentSummary,
}

fn summarize_block(index: usize, block: &DiffBlock) -> BlockSummary {
    BlockSummary {
        index,
        page_style_count: block.page_styles.iter().count(),
        content: summarize_content(&block.content, 0),
    }
}

#[derive(Serialize)]
struct BlockOpSummary {
    kind: String,
    old: Option<BlockSummary>,
    new: Option<BlockSummary>,
}

fn summarize_block_op(op: &BlockOp) -> BlockOpSummary {
    match op {
        BlockOp::Equal(old, new) => BlockOpSummary {
            kind: "equal".to_string(),
            old: Some(summarize_block(0, old)),
            new: Some(summarize_block(0, new)),
        },
        BlockOp::Delete(old) => BlockOpSummary {
            kind: "delete".to_string(),
            old: Some(summarize_block(0, old)),
            new: None,
        },
        BlockOp::Insert(new) => BlockOpSummary {
            kind: "insert".to_string(),
            old: None,
            new: Some(summarize_block(0, new)),
        },
        BlockOp::Replace(old, new) => BlockOpSummary {
            kind: "replace".to_string(),
            old: Some(summarize_block(0, old)),
            new: Some(summarize_block(0, new)),
        },
    }
}

#[derive(Serialize)]
struct DiffBlockEditSummary {
    index: usize,
    page_style_count: usize,
    base: AnnotatedSummary,
    edits: Vec<RealizedEditSummary>,
}

#[derive(Serialize)]
struct RegionSummary {
    path: String,
    base: AnnotatedSummary,
    edits: Vec<RealizedEditSummary>,
}

#[derive(Serialize)]
struct RenderedRegionSummary {
    kind: String,
    wrapper: String,
    page_count: usize,
    changed_page_count: usize,
    pages: Vec<RenderedRegionPageSummary>,
}

#[derive(Serialize)]
struct RenderedRegionPageSummary {
    page: usize,
    old_frame_trace_ref: String,
    new_frame_trace_ref: String,
    changed: bool,
    base: ContentSummary,
    word_ops: Vec<WordOpSummary>,
    segments: Vec<RenderedRegionSegmentSummary>,
}

#[derive(Serialize)]
struct RenderedRegionSegmentSummary {
    base: ContentSummary,
    word_ops: Vec<WordOpSummary>,
}

fn summarize_rendered_region(region: &crate::diff::RenderedRegionEdit) -> RenderedRegionSummary {
    RenderedRegionSummary {
        kind: page_region_label(region.kind),
        wrapper: rendered_wrapper_label(region.wrapper),
        page_count: region.pages.len(),
        changed_page_count: region.pages.iter().filter(|page| page.changed).count(),
        pages: region
            .pages
            .iter()
            .map(|page| RenderedRegionPageSummary {
                page: page.page,
                old_frame_trace_ref: frame_trace_id("old", region.kind, page.page),
                new_frame_trace_ref: frame_trace_id("new", region.kind, page.page),
                changed: page.changed,
                base: summarize_content(&page.base, 0),
                word_ops: page.word_ops.iter().map(summarize_word_op).collect(),
                segments: page
                    .segments
                    .iter()
                    .map(summarize_rendered_region_segment)
                    .collect(),
            })
            .collect(),
    }
}

fn summarize_rendered_region_segment(
    segment: &RenderedRegionSegmentEdit,
) -> RenderedRegionSegmentSummary {
    RenderedRegionSegmentSummary {
        base: summarize_content(&segment.base, 0),
        word_ops: segment.word_ops.iter().map(summarize_word_op).collect(),
    }
}

#[derive(Serialize)]
struct RealizedEditSummary {
    kind: String,
    path: Option<Vec<usize>>,
    anchor: Option<Vec<usize>>,
    content: EditContentSummary,
}

fn summarize_realized_edit(edit: &RealizedEdit) -> RealizedEditSummary {
    match edit {
        RealizedEdit::ReplaceAt { path, content } => RealizedEditSummary {
            kind: "replace_at".to_string(),
            path: Some(path.clone()),
            anchor: None,
            content: summarize_edit_content(content),
        },
        RealizedEdit::InsertBefore { anchor, content } => RealizedEditSummary {
            kind: "insert_before".to_string(),
            path: None,
            anchor: Some(anchor.clone()),
            content: summarize_edit_content(content),
        },
        RealizedEdit::InsertAfter { anchor, content } => RealizedEditSummary {
            kind: "insert_after".to_string(),
            path: None,
            anchor: Some(anchor.clone()),
            content: summarize_edit_content(content),
        },
        RealizedEdit::Append { content } => RealizedEditSummary {
            kind: "append".to_string(),
            path: None,
            anchor: None,
            content: summarize_edit_content(content),
        },
        RealizedEdit::WholeBlock(content) => RealizedEditSummary {
            kind: "whole_block".to_string(),
            path: None,
            anchor: None,
            content: summarize_edit_content(content),
        },
    }
}

#[derive(Serialize)]
struct EditContentSummary {
    kind: String,
    content: Option<ContentSummary>,
    base: Option<ContentSummary>,
    word_ops: Vec<WordOpSummary>,
    nested_edits: Vec<RealizedEditSummary>,
}

fn summarize_edit_content(content: &EditContent) -> EditContentSummary {
    match content {
        EditContent::Inserted(content) => EditContentSummary {
            kind: "inserted".to_string(),
            content: Some(summarize_content(content, 0)),
            base: None,
            word_ops: Vec::new(),
            nested_edits: Vec::new(),
        },
        EditContent::Deleted(content) => EditContentSummary {
            kind: "deleted".to_string(),
            content: Some(summarize_content(content, 0)),
            base: None,
            word_ops: Vec::new(),
            nested_edits: Vec::new(),
        },
        EditContent::OpaqueReplacement { old, new } => EditContentSummary {
            kind: "opaque_replacement".to_string(),
            content: Some(summarize_content(new, 0)),
            base: Some(summarize_content(old, 0)),
            word_ops: Vec::new(),
            nested_edits: Vec::new(),
        },
        EditContent::Modified { base, word_ops } => EditContentSummary {
            kind: "modified".to_string(),
            content: None,
            base: Some(summarize_content(base, 0)),
            word_ops: word_ops.iter().map(summarize_word_op).collect(),
            nested_edits: Vec::new(),
        },
        EditContent::Nested { base, edits } => EditContentSummary {
            kind: "nested".to_string(),
            content: None,
            base: Some(summarize_content(&base.realized, 0)),
            word_ops: Vec::new(),
            nested_edits: edits.iter().map(summarize_realized_edit).collect(),
        },
    }
}

#[derive(Serialize)]
struct WordOpSummary {
    kind: String,
    token_count: usize,
    text_len: usize,
    text: String,
    text_preview: String,
}

fn summarize_word_op(op: &WordOp) -> WordOpSummary {
    let (kind, tokens) = match op {
        WordOp::Equal(tokens) => ("equal", tokens),
        WordOp::Delete(tokens) => ("delete", tokens),
        WordOp::Insert(tokens) => ("insert", tokens),
    };
    let text = tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>();
    WordOpSummary {
        kind: kind.to_string(),
        token_count: tokens.len(),
        text_len: text.chars().count(),
        text: text.clone(),
        text_preview: preview(&text),
    }
}

fn content_kind(content: &Content) -> String {
    content.func().name().to_string()
}

fn content_hash(content: &Content) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn has_source_span(span: Span) -> bool {
    !span.is_detached()
}

fn preview(text: &str) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ch in single_line.chars().take(PREVIEW_CHARS) {
        out.push(ch);
    }
    if single_line.chars().count() > PREVIEW_CHARS {
        out.push_str("...");
    }
    out
}

fn semantic_kind_label(kind: &SemanticKind) -> String {
    match kind {
        SemanticKind::Paragraph => "paragraph".to_string(),
        SemanticKind::Heading => "heading".to_string(),
        SemanticKind::RawBlock => "raw_block".to_string(),
        SemanticKind::List => "list".to_string(),
        SemanticKind::Enum => "enum".to_string(),
        SemanticKind::Terms => "terms".to_string(),
        SemanticKind::Table => "table".to_string(),
        SemanticKind::Grid => "grid".to_string(),
        SemanticKind::Stack => "stack".to_string(),
        SemanticKind::Figure => "figure".to_string(),
        SemanticKind::Footnote => "footnote".to_string(),
        SemanticKind::Quote => "quote".to_string(),
        SemanticKind::Equation => "equation".to_string(),
        SemanticKind::Wrapper(kind) => format!("wrapper/{}", wrapper_kind_label(kind)),
    }
}

fn wrapper_kind_label(kind: &WrapperKind) -> &'static str {
    match kind {
        WrapperKind::Align => "align",
        WrapperKind::Pad => "pad",
        WrapperKind::Place => "place",
        WrapperKind::Columns => "columns",
        WrapperKind::Box => "box",
        WrapperKind::Block => "block",
        WrapperKind::Rect => "rect",
        WrapperKind::Circle => "circle",
        WrapperKind::Ellipse => "ellipse",
    }
}

fn slot_label(step: &SlotStep) -> String {
    match step {
        SlotStep::ListItem(index) => format!("list_item[{index}]"),
        SlotStep::EnumItem(index) => format!("enum_item[{index}]"),
        SlotStep::Term(index) => format!("term[{index}]"),
        SlotStep::TermDescription(index) => format!("term_description[{index}]"),
        SlotStep::FigureBody => "figure_body".to_string(),
        SlotStep::FigureCaption => "figure_caption".to_string(),
        SlotStep::FootnoteBody => "footnote_body".to_string(),
        SlotStep::QuoteBody => "quote_body".to_string(),
        SlotStep::WrapperBody => "wrapper_body".to_string(),
        SlotStep::TableCell(index) => format!("table_cell[{index}]"),
        SlotStep::GridCell(index) => format!("grid_cell[{index}]"),
        SlotStep::StackChild(index) => format!("stack_child[{index}]"),
    }
}

fn region_path_label(path: RegionPath) -> String {
    match path {
        RegionPath::RootPage(kind) => format!("root_page/{}", page_region_label(kind)),
    }
}

fn page_region_label(kind: PageRegionKind) -> String {
    match kind {
        PageRegionKind::Header => "header",
        PageRegionKind::Footer => "footer",
        PageRegionKind::Background => "background",
        PageRegionKind::Foreground => "foreground",
    }
    .to_string()
}

fn frame_trace_id(side: &str, kind: PageRegionKind, page: usize) -> String {
    format!("{side}/{}/page-{page}", page_region_label(kind))
}

fn rendered_wrapper_label(wrapper: RenderedRegionWrapper) -> String {
    match wrapper {
        RenderedRegionWrapper::None => "none".to_string(),
        RenderedRegionWrapper::Align(alignment) => {
            format!("align/{}", rendered_alignment_label(alignment))
        }
    }
}

fn rendered_alignment_label(alignment: RenderedRegionAlignment) -> &'static str {
    match alignment {
        RenderedRegionAlignment::Left => "left",
        RenderedRegionAlignment::Center => "center",
        RenderedRegionAlignment::Right => "right",
        RenderedRegionAlignment::Start => "start",
        RenderedRegionAlignment::End => "end",
    }
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}
