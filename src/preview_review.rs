//! Read-only visual review artifacts built from the interactive evaluator.
use crate::{
    frame_render::PreviewEngine,
    preview::{self, ExportOptions},
    progress::Progress,
    project::{self, array, enabled},
    time::{bounds, Time},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

#[derive(Default)]
pub struct Options {
    pub sheet: bool,
    pub boundaries: bool,
    pub count: Option<usize>,
    pub interval: Option<Time>,
    pub columns: Option<usize>,
}
impl Options {
    // Also enforced for deserialized ApplicationHost requests, not only clap.
    pub fn validate(&self, selection: &ExportOptions) -> Result<()> {
        preview::validate_scale(selection.scale)?;
        ensure!(
            !(self.sheet && self.boundaries),
            "PREVIEW_OPTIONS_CONFLICT: choose --sheet or --boundaries"
        );
        ensure!(
            self.sheet
                || (self.count.is_none() && self.columns.is_none() && self.interval.is_none()),
            "PREVIEW_OPTIONS_CONFLICT: --count/--columns/--interval require --sheet"
        );
        ensure!(
            self.interval.is_none() || self.count.is_none(),
            "PREVIEW_OPTIONS_CONFLICT: --interval cannot use --count"
        );
        ensure!(
            self.interval
                .as_ref()
                .is_none_or(|interval| interval > &Time::zero()),
            "PREVIEW_INVALID_INTERVAL: expected a positive interval in seconds"
        );
        ensure!(
            !self.sheet || selection.at.is_none(),
            "PREVIEW_OPTIONS_CONFLICT: --sheet cannot use --at"
        );
        ensure!(
            selection.at.is_none() || (selection.start.is_none() && selection.duration.is_none()),
            "PREVIEW_OPTIONS_CONFLICT: --at cannot use --start/--duration"
        );
        ensure!(
            (1..=128).contains(&self.count.unwrap_or(12)),
            "PREVIEW_INVALID_COUNT: expected 1..128"
        );
        ensure!(
            (1..=16).contains(&self.columns.unwrap_or(4)),
            "PREVIEW_INVALID_COLUMNS: expected 1..16"
        );
        Ok(())
    }
}

struct Cell {
    time: Option<Time>,
    metadata: Value,
    label: String,
}
struct Plan {
    cells: Vec<Cell>,
    boundaries: Vec<Value>,
    columns: usize,
    fps: Time,
}
fn plan(project: &Value, sid: &str, selection: &ExportOptions, options: &Options) -> Result<Plan> {
    options.validate(selection)?;
    ensure!(
        options.sheet || options.boundaries,
        "PREVIEW_REVIEW_MODE_REQUIRED"
    );
    let seq = project["sequences"]
        .get(sid)
        .context("sequence not found")?;
    let end = project::sequence_duration(seq)?;
    ensure!(end > Time::zero(), "PREVIEW_EMPTY_SEQUENCE");
    let fps = Time::from_json(&seq["format"]["frameRate"])?;
    let start = selection.start.clone().unwrap_or_else(Time::zero);
    let duration = selection
        .duration
        .clone()
        .unwrap_or_else(|| end.sub(&start));
    let stop = start.add(&duration);
    ensure!(
        start.nonnegative() && start < end && duration > Time::zero() && stop <= end,
        "PREVIEW_RANGE_OUT_OF_BOUNDS"
    );
    let mut cells = Vec::new();
    let mut boundaries = Vec::new();
    let columns;
    if options.sheet {
        let (count, step) = if let Some(interval) = &options.interval {
            let count = duration.div(interval)?;
            ensure!(count <= Time::integer(128), "PREVIEW_TOO_MANY_THUMBNAILS: increase --interval or narrow --start/--duration (maximum 128)");
            (count.ceil_i64()? as usize, interval.clone())
        } else {
            let count = options.count.unwrap_or(12);
            (count, duration.div(&Time::integer(count as i64))?)
        };
        columns = options.columns.unwrap_or(4).min(count);
        for i in 0..count {
            let at = start.add(&step.mul(&Time::integer(i as i64)));
            cells.push(Cell {
                label: format!("{}\nt={} s", i + 1, at),
                metadata: json!({"index":i,"time":at.json()?}),
                time: Some(at),
            });
        }
    } else {
        columns = 2;
        let mut edges: BTreeMap<Time, Vec<Value>> = BTreeMap::new();
        if let Some(at) = &selection.at {
            ensure!(at.nonnegative() && at <= &end, "PREVIEW_TIME_OUT_OF_BOUNDS");
            edges.insert(at.clone(), Vec::new());
        } else {
            for kind in ["video", "subtitle"] {
                let solo = array(&seq["tracks"])
                    .iter()
                    .any(|tr| tr["kind"] == kind && enabled(tr) && tr["solo"] == true);
                for track in array(&seq["tracks"])
                    .iter()
                    .filter(|tr| tr["kind"] == kind && enabled(tr) && (!solo || tr["solo"] == true))
                {
                    for item in array(&track["items"]).iter().filter(|item| enabled(item)) {
                        let (a, b) = bounds(&item["timelineRange"])?;
                        for (at, edge) in [(a, "start"), (b, "end")] {
                            if at >= start && at <= stop {
                                edges.entry(at).or_default().push(
                                    json!({"trackId":track["id"],"itemId":item["id"],"edge":edge}),
                                );
                                ensure!(edges.len() <= 64, "PREVIEW_TOO_MANY_BOUNDARIES: narrow --start/--duration (maximum 64)");
                            }
                        }
                    }
                }
            }
        }
        ensure!(!edges.is_empty(), "PREVIEW_NO_BOUNDARIES");
        for (index, (boundary, items)) in edges.into_iter().enumerate() {
            boundaries.push(json!({"index":index,"time":boundary.json()?,"items":items}));
            let after = boundary.mul(&fps).ceil_i64()?;
            for (side, frame_index) in [("before", after - 1), ("after", after)] {
                let at = Time::integer(frame_index).div(&fps)?;
                let at = (at.nonnegative() && at < end).then_some(at);
                let label = if let Some(at) = &at {
                    format!("B{} {} #{}\nt={} s", index + 1, side, frame_index, at)
                } else {
                    format!("B{} {}\nN/A", index + 1, side)
                };
                cells.push(Cell {
                    metadata: json!({"index":cells.len(),"boundaryIndex":index,"side":side,"time":at.as_ref().map(Time::json).transpose()?,"frameIndex":at.as_ref().map(|_|frame_index)}),
                    time: at,
                    label,
                });
            }
        }
    }
    Ok(Plan {
        cells,
        boundaries,
        columns,
        fps,
    })
}

/// One engine per review; only the completed PNG is published, atomically.
pub fn export(
    project: &Value,
    dir: &Path,
    sid: &str,
    selection: &ExportOptions,
    options: &Options,
    progress: &Progress,
) -> Result<Value> {
    options.validate(selection)?;
    preview::validate(project, dir, progress)?;
    let plan = plan(project, sid, selection, options)?;
    let seq = &project["sequences"][sid];
    let long_edge = seq["format"]["width"]
        .as_f64()
        .context("width")?
        .max(seq["format"]["height"].as_f64().context("height")?);
    let scale = selection.scale.unwrap_or((320. / long_edge).min(1.));
    let manager = crate::cache::Manager::new(dir);
    let cache = manager.directory(crate::cache::Kind::Sheets)?;
    let scratch = manager.scratch()?;
    let mut engine =
        PreviewEngine::with_scale(project, dir, sid, scratch.path(), progress, Some(scale))?;
    // Labels are outside the picture. A minimum width keeps exact i64 rational times readable.
    let cell_width = engine.width.max(384);
    let cell_height = engine.height + 44;
    let width = cell_width * plan.columns;
    let height = cell_height * plan.cells.len().div_ceil(plan.columns);
    ensure!(
        width <= 32768 && height <= 32768 && width * height <= 16_000_000,
        "PREVIEW_SHEET_TOO_LARGE: reduce count, range, or scale"
    );
    let mut pixels = vec![0u8; width * height * 8];
    for pixel in pixels.chunks_exact_mut(8) {
        pixel.copy_from_slice(&[0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0xff, 0xff]);
    }
    let mut cells = Vec::new();
    let mut filters = Vec::new();
    let total = plan.cells.len() as u64;
    progress.start("review_frames", Some(total), "frames");
    for (i, cell) in plan.cells.into_iter().enumerate() {
        progress.check_cancelled()?;
        let x = i % plan.columns * cell_width;
        let y = i / plan.columns * cell_height;
        let image_x = x + (cell_width - engine.width) / 2;
        if let Some(at) = &cell.time {
            let frame = engine.video(at, true)?;
            for row in 0..engine.height {
                let dest = ((y + row) * width + image_x) * 8;
                let source = row * engine.width * 8;
                pixels[dest..dest + engine.width * 8]
                    .copy_from_slice(&frame[source..source + engine.width * 8]);
            }
        }
        let mut metadata = cell.metadata;
        metadata["x"] = json!(image_x);
        metadata["y"] = json!(y);
        metadata["width"] = json!(engine.width);
        metadata["height"] = json!(engine.height);
        cells.push(metadata);
        let file = format!("label-{i}.txt");
        fs::write(scratch.path().join(&file), cell.label)?;
        filters.push(format!(
            "drawtext=textfile={file}:expansion=none:fontsize=12:fontcolor=white:x={}:y={}",
            x + 6,
            y + engine.height + 4
        ));
        progress.advance_phase("review_frames", Some(total), "frames", (i + 1) as u64);
    }
    fs::write(scratch.path().join("sheet.rgba"), pixels)?;
    fs::write(
        scratch.path().join("labels.txt"),
        format!("[0:v]{}[sheet]", filters.join(",")),
    )?;
    progress.start("encode_sheet", Some(1), "images");
    let result = progress.output(Command::new("ffmpeg").current_dir(scratch.path()).args([
        "-nostdin",
        "-v",
        "error",
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgba64le",
        "-video_size",
        &format!("{width}x{height}"),
        "-i",
        "sheet.rgba",
        "-filter_complex_threads",
        "2",
        crate::media_process::graph_file_option()?,
        "labels.txt",
        "-map",
        "[sheet]",
        "-frames:v",
        "1",
        "-threads",
        "2",
        "-c:v",
        "png",
        "-pix_fmt",
        "rgba64be",
        "-f",
        "image2pipe",
        "pipe:1",
    ]))?;
    ensure!(
        result.status.success(),
        "MEDIA_PROCESS_FAILED: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mode = if options.sheet { "sheet" } else { "boundaries" };
    let key = crate::store::hash(
        &json!({"project":project,"sequenceId":sid,"mode":mode,"cells":cells,"boundaries":plan.boundaries,"scale":scale,"columns":plan.columns,"fonts":crate::fonts::signatures(project,dir)?,"profile":"review-v1","renderer":env!("CARGO_PKG_VERSION")}),
    )?;
    let output = cache.join(format!("{key}.png"));
    progress.start("finalize", None, "files");
    progress.check_cancelled()?;
    let entry = manager.entry(&output)?;
    let _build = entry.build_lock()?;
    entry.publish(&result.stdout)?;
    progress.finish();
    Ok(
        json!({"ok":true,"preview":true,"mode":mode,"output":output,"image":output,"mime":"image/png","revision":project["revision"],"sequenceId":sid,"width":width,"height":height,"columns":plan.columns,"scale":scale,"frameRate":plan.fps.json()?,"cells":cells,"boundaries":plan.boundaries}),
    )
}
