use crate::{
    edit, id, media,
    project::{self, array, text},
    query, render, schema,
    store::{read_json, Store},
    time::{range, Time},
};
use anyhow::{ensure, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::{
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[command(
    name = "cutcraft",
    version,
    about = "Agent-first non-linear editing. All command results are JSON."
)]
pub struct Cli {
    #[arg(
        short = 'p',
        long,
        global = true,
        default_value = ".",
        help = "Project directory containing cutcraft.json"
    )]
    pub project: PathBuf,
    #[arg(
        long,
        global = true,
        help = "Explicitly permit mutations of locked tracks"
    )]
    pub override_locks: bool,
    #[arg(
        long,
        global = true,
        help = "Validate an edit without writing project or history"
    )]
    pub dry_run: bool,
    #[command(subcommand)]
    pub command: Cmd,
    /// Materialized client-side JSON inputs; not a CLI argument.
    #[arg(skip)]
    #[serde(default)]
    pub inputs: BTreeMap<PathBuf, Value>,
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum Cmd {
    /// Open a project in the shared local editor.
    Open {
        path: Option<PathBuf>,
        #[arg(long)]
        no_browser: bool,
        /// Listen IP or localhost. Non-loopback bindings enable browser authentication.
        #[arg(long, value_parser = crate::daemon::parse_host)]
        #[serde(default)]
        host: Option<std::net::IpAddr>,
    },
    /// Close an open project. Active imports/exports must finish or be cancelled first.
    Close { path: Option<PathBuf> },
    #[command(name = "__serve", hide = true)]
    Serve {
        #[arg(long, default_value = "127.0.0.1", value_parser = crate::daemon::parse_host)]
        host: std::net::IpAddr,
    },
    /// Create a project with a 1920x1080, 30 fps root sequence.
    Init {
        #[arg(default_value = ".")]
        directory: PathBuf,
        #[arg(long, default_value = "Untitled")]
        name: String,
    },
    /// Add a built-in transition between adjacent video items.
    Transition {
        from: String,
        to: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long, default_value="crossfade",value_parser=["crossfade","dip-to-color"])]
        kind: String,
        #[arg(long)]
        duration: Time,
        #[arg(long, default_value="center",value_parser=["before","center","after"])]
        alignment: String,
        #[arg(long)]
        progress: Option<PathBuf>,
        /// Linear-sRGB color for dip-to-color.
        #[arg(long, default_value = "#000000")]
        color: String,
    },
    /// Inspect or reclaim disposable project caches.
    Cache {
        #[command(subcommand)]
        command: CacheCmd,
    },
    /// Read the canonical project snapshot.
    Inspect,
    /// Validate project, operation or event JSON offline; optionally check media files.
    Validate {
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "project")]
        kind: Kind,
        #[arg(long, value_enum, default_value = "semantic")]
        level: Level,
    },
    /// Emit an embedded JSON Schema.
    Schema {
        #[arg(value_enum, default_value = "project")]
        kind: Kind,
    },
    /// Generate a typed UUIDv7 ID.
    Id {
        #[arg(default_value="item",value_parser=["project","asset","seq","track","item","effect","transition","link","marker","op","event","actor"])]
        prefix: String,
    },
    /// Apply a complete Operation envelope (FILE or - for stdin).
    Apply { file: PathBuf },
    /// Wrap a domain command in a fresh Operation and apply it (FILE or -).
    Command {
        file: PathBuf,
        #[arg(long)]
        base_revision: Option<u64>,
    },
    /// Undo the latest applied event as a new operation; undoing undo is redo.
    Undo,
    /// Print audited applied and rejected events.
    History {
        #[command(subcommand)]
        command: Option<HistoryCmd>,
    },
    /// Probe and import a media file; optionally copy it into assets/ and append to timeline.
    Import {
        file: PathBuf,
        /// Explicit transfer/primaries declaration for untagged sources.
        #[arg(long, value_parser=["rec709", "srgb", "linear-srgb"])]
        color_space: Option<String>,
        #[arg(long)]
        managed: bool,
        #[arg(long)]
        append: bool,
    },
    /// Import a frozen image sequence as one video asset.
    ImportSequence {
        directory: PathBuf,
        #[arg(long, required_unless_present = "files", conflicts_with = "files")]
        pattern: Option<String>,
        #[arg(long, required_unless_present = "pattern", conflicts_with = "pattern")]
        files: Option<PathBuf>,
        #[arg(long, requires="pattern", value_parser=["printf","hash"])]
        pattern_style: Option<String>,
        #[arg(long, requires = "pattern")]
        start_number: Option<i64>,
        #[arg(long, requires = "pattern")]
        end_number: Option<i64>,
        #[arg(long, requires="pattern", value_parser=["error","hold","skip"])]
        missing: Option<String>,
        #[arg(long)]
        fps: Time,
        #[arg(long, value_parser=["rec709","srgb","linear-srgb"])]
        color_space: Option<String>,
        #[arg(long)]
        managed: bool,
        #[arg(long)]
        append: bool,
    },
    /// Native FFprobe information, without importing anything.
    Probe { file: PathBuf },
    /// Search, inspect or replace media-library assets.
    Asset {
        #[command(subcommand)]
        command: AssetCmd,
    },
    /// Sequence operations.
    Sequence {
        #[command(subcommand)]
        command: SequenceCmd,
    },
    /// Track operations.
    Track {
        #[command(subcommand)]
        command: TrackCmd,
    },
    /// Item operations. Advanced fields remain available through `command`/`apply`.
    Item {
        #[command(subcommand)]
        command: ItemCmd,
    },
    /// Add a sequence marker.
    Marker {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        name: String,
        #[arg(long)]
        at: Time,
        #[arg(long, default_value = "0")]
        duration: Time,
    },
    /// Resolve active timeline layers and exact source-time mappings.
    Sample {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        at: Time,
    },
    /// Export supported timelines to an MP4 using FFmpeg.
    Render {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(short, long, default_value = "output/render.mp4")]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        /// Progress on stderr; stdout remains a single result JSON.
        #[arg(long, value_enum, default_value = "auto")]
        progress: ProgressMode,
        #[command(flatten)]
        #[serde(default)]
        encoding: crate::encoding::EncodingOptions,
    },
    /// Export MP4/PNG previews, a thumbnail sheet, or boundary frame pairs.
    Preview {
        #[arg(long)]
        sequence: Option<String>,
        /// Export an evenly sampled thumbnail grid with exact time labels.
        #[arg(long, conflicts_with_all = ["boundaries", "at"])]
        #[serde(default)]
        sheet: bool,
        /// Inspect frames before/after visual item edges, or one --at time.
        #[arg(long)]
        #[serde(default)]
        boundaries: bool,
        /// Number of thumbnails (1-128); defaults to 12.
        #[arg(long, requires = "sheet")]
        #[serde(default)]
        count: Option<usize>,
        /// Sample every N seconds (positive, accepts fractions); conflicts with --count.
        #[arg(long, requires = "sheet", conflicts_with = "count")]
        #[serde(default)]
        interval: Option<Time>,
        /// Grid columns (1-16); defaults to 4.
        #[arg(long, requires = "sheet")]
        #[serde(default)]
        columns: Option<usize>,
        /// Exact sequence time for a single PNG frame.
        #[arg(long, conflicts_with_all = ["start", "duration"])]
        #[serde(default)]
        at: Option<Time>,
        /// Start of a half-open preview interval; defaults to zero.
        #[arg(long)]
        #[serde(default)]
        start: Option<Time>,
        /// Interval length; defaults to the remaining sequence duration.
        #[arg(long)]
        #[serde(default)]
        duration: Option<Time>,
        /// Resolution multiplier in (0, 1]; omission caps the long edge at 640.
        #[arg(long)]
        #[serde(default)]
        scale: Option<f64>,
        #[arg(long, value_enum, default_value = "auto")]
        progress: ProgressMode,
        #[command(flatten)]
        #[serde(default)]
        encoding: crate::encoding::EncodingOptions,
    },
    /// Diagnose local FFmpeg/ffprobe capabilities and media processing.
    Doctor {
        /// Test this H.264 encoder by encoding and decoding actual frames.
        #[arg(long, value_enum)]
        #[serde(default)]
        video_encoder: Option<crate::encoding::VideoEncoder>,
        /// Timeout in seconds for each external check (1-120).
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=120))]
        timeout: u64,
    },
    /// Describe the media backend's supported profile.
    Capabilities,
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum CacheCmd {
    /// Report managed bytes, budgets and protected/in-use entries.
    Status,
    /// Remove reclaimable entries; --dry-run reports without deleting.
    Clean,
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum HistoryCmd {
    /// Undo the latest action in the shared multi-step history.
    Undo,
    /// Redo the latest undone action.
    Redo,
}
#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
pub enum ProgressMode {
    Auto,
    None,
    Human,
    Json,
}
impl ProgressMode {
    fn reporter(self) -> crate::progress::Progress {
        use crate::progress::{Mode, Progress};
        Progress::new(match self {
            Self::Auto if std::io::stderr().is_terminal() => Mode::Human,
            Self::Auto | Self::None => Mode::None,
            Self::Human => Mode::Human,
            Self::Json => Mode::Json,
        })
    }
}

#[derive(Clone, Debug, Args, Serialize, Deserialize)]
pub struct TextItemArgs {
    #[arg(long)]
    sequence: Option<String>,
    #[arg(long)]
    track: String,
    #[arg(long, default_value = "0")]
    start: Time,
    #[arg(long)]
    duration: Time,
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    /// Read UTF-8 text (including explicit newlines) into the project.
    #[arg(long)]
    text_file: Option<PathBuf>,
    #[arg(long, conflicts_with = "font_family")]
    font_asset: Option<String>,
    /// Installed font family (e.g. PingFang SC) or a face's PostScript name.
    #[arg(long)]
    font_family: Option<String>,
    #[arg(long)]
    font_size: Option<f64>,
    #[arg(long)]
    color: Option<String>,
    #[arg(long, value_parser = ["left", "center", "right"])]
    align: Option<String>,
    #[arg(long, value_parser = ["top", "center", "bottom"])]
    position: Option<String>,
    #[command(flatten)]
    #[serde(default)]
    card: Box<CardArgs>,
}
impl TextItemArgs {
    fn command(self, sid: &str, caption: bool) -> Result<Value> {
        let content = if let Some(path) = self.text_file {
            std::fs::read_to_string(&path)
                .with_context(|| format!("read UTF-8 text {}", path.display()))?
        } else {
            self.text.unwrap()
        };
        let mut style = json!({});
        if let Some(font) = self.font_asset {
            style["fontAssetId"] = json!(font);
        }
        if let Some(font) = self.font_family {
            style["fontFamily"] = json!(font);
        }
        if let Some(size) = self.font_size {
            style["fontSizePixels"] = json!(size);
        }
        if let Some(c) = self.color {
            style["color"] = color(&c)?;
        }
        if let Some(align) = self.align {
            style["align"] = json!(align);
        }
        if let Some(position) = self.position {
            style["position"] = json!(position);
        }
        if let Some(card) = self.card.value()? {
            style["card"] = card;
        }
        let mut item = json!({"id":id("item"),"timelineRange":range(&self.start,&self.duration)?});
        if caption {
            item["type"] = json!("caption");
            item["text"] = json!(content);
            item["style"] = style;
        } else {
            style["text"] = json!(content);
            item["type"] = json!("generator");
            item["generatorType"] = json!("builtin.text");
            item["version"] = json!("1.0.0");
            item["parameters"] = style;
        }
        Ok(json!({"type":"item.put","sequenceId":sid,"trackId":self.track,"item":item}))
    }
}
#[derive(Clone, Debug, Default, Args, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CardArgs {
    #[arg(long, requires = "card_height")]
    card_width: Option<f64>,
    #[arg(long, requires = "card_width")]
    card_height: Option<f64>,
    #[arg(long, requires = "card_width")]
    card_padding: Option<f64>,
    #[arg(long, requires = "card_width")]
    card_radius: Option<f64>,
    #[arg(long, requires = "card_width", conflicts_with = "card_gradient_start")]
    card_color: Option<String>,
    #[arg(long, requires_all = ["card_width", "card_gradient_end"])]
    card_gradient_start: Option<String>,
    #[arg(long, requires = "card_gradient_start")]
    card_gradient_end: Option<String>,
    #[arg(long, requires = "card_gradient_start", value_parser = ["horizontal", "vertical"])]
    card_gradient_direction: Option<String>,
    #[arg(long, requires = "card_width")]
    card_border_width: Option<f64>,
    #[arg(long, requires = "card_border_width")]
    card_border_color: Option<String>,
    #[arg(long, requires = "card_width")]
    card_shadow_color: Option<String>,
    #[arg(long, requires = "card_shadow_color", allow_hyphen_values = true)]
    card_shadow_x: Option<f64>,
    #[arg(long, requires = "card_shadow_color", allow_hyphen_values = true)]
    card_shadow_y: Option<f64>,
    #[arg(long, requires = "card_shadow_color")]
    card_shadow_blur: Option<f64>,
    #[arg(long, requires = "card_width")]
    text_stroke_width: Option<f64>,
    #[arg(long, requires = "text_stroke_width")]
    text_stroke_color: Option<String>,
    #[arg(long, requires = "card_width")]
    line_height: Option<f64>,
    #[arg(long, requires = "card_width", value_parser = ["top", "center", "bottom"])]
    vertical_align: Option<String>,
}
impl CardArgs {
    fn value(&self) -> Result<Option<Value>> {
        ensure!(
            [
                self.card_width,
                self.card_height,
                self.card_padding,
                self.card_radius,
                self.card_border_width,
                self.card_shadow_x,
                self.card_shadow_y,
                self.card_shadow_blur,
                self.text_stroke_width,
                self.line_height
            ]
            .into_iter()
            .flatten()
            .all(f64::is_finite),
            "TEXT_CARD_INVALID: finite numbers required"
        );
        let supplied = serde_json::to_value(self)?;
        if supplied.as_object().unwrap().values().all(Value::is_null) {
            return Ok(None);
        }
        let width = self
            .card_width
            .context("TEXT_CARD_INVALID: --card-width required")?;
        let height = self
            .card_height
            .context("TEXT_CARD_INVALID: --card-height required")?;
        ensure!(
            self.card_color.is_none() || self.card_gradient_start.is_none(),
            "TEXT_CARD_INVALID: solid and gradient conflict"
        );
        ensure!(
            self.card_gradient_start.is_some() == self.card_gradient_end.is_some(),
            "TEXT_CARD_INVALID: gradient requires both colors"
        );
        ensure!(
            self.card_gradient_direction.is_none() || self.card_gradient_start.is_some(),
            "TEXT_CARD_INVALID: gradient direction needs colors"
        );
        ensure!(
            self.card_border_color.is_none() || self.card_border_width.is_some(),
            "TEXT_CARD_INVALID: border color needs width"
        );
        ensure!(
            self.text_stroke_color.is_none() || self.text_stroke_width.is_some(),
            "TEXT_CARD_INVALID: stroke color needs width"
        );
        ensure!(
            (self.card_shadow_x.is_none()
                && self.card_shadow_y.is_none()
                && self.card_shadow_blur.is_none())
                || self.card_shadow_color.is_some(),
            "TEXT_CARD_INVALID: shadow needs color"
        );
        let mut card = json!({"widthPixels":width,"heightPixels":height});
        for (key, value) in [
            ("paddingPixels", self.card_padding),
            ("radiusPixels", self.card_radius),
            ("lineHeight", self.line_height),
        ] {
            if let Some(v) = value {
                card[key] = json!(v);
            }
        }
        if let Some(v) = &self.vertical_align {
            card["verticalAlign"] = json!(v);
        }
        if let Some(v) = &self.card_color {
            card["background"] = json!({"type":"solid","color":color(v)?});
        }
        if let (Some(a), Some(b)) = (&self.card_gradient_start, &self.card_gradient_end) {
            card["background"] = json!({"type":"linear-gradient","startColor":color(a)?,"endColor":color(b)?,"direction":self.card_gradient_direction.as_deref().unwrap_or("horizontal")});
        }
        for (key, width, c, default) in [
            (
                "border",
                self.card_border_width,
                &self.card_border_color,
                "#ffffff",
            ),
            (
                "textStroke",
                self.text_stroke_width,
                &self.text_stroke_color,
                "#000000",
            ),
        ] {
            if let Some(w) = width {
                card[key] =
                    json!({"widthPixels":w,"color":color(c.as_deref().unwrap_or(default))?});
            }
        }
        if let Some(c) = &self.card_shadow_color {
            card["shadow"] = json!({"color":color(c)?,"offsetXPixels":self.card_shadow_x.unwrap_or(0.),"offsetYPixels":self.card_shadow_y.unwrap_or(0.),"sigmaPixels":self.card_shadow_blur.unwrap_or(0.)});
        }
        crate::schema::validate("text-card", &card)?;
        Ok(Some(card))
    }
}
#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
pub enum Kind {
    Project,
    Operation,
    Event,
    Common,
    Generator,
    TextCard,
    Effect,
    Animation,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Operation => "operation",
            Self::Event => "event",
            Self::Common => "common",
            Self::Generator => "generator",
            Self::TextCard => "text-card",
            Self::Effect => "effect",
            Self::Animation => "animation",
        }
    }
}
#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
pub enum Level {
    Schema,
    Semantic,
    Media,
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum AssetCmd {
    /// Replace a file asset for all its uses, preserving asset and stream IDs.
    Replace {
        asset: String,
        file: PathBuf,
        /// Copy the new file into assets/; default inherits the old storage mode.
        #[arg(long, conflicts_with = "external")]
        managed: bool,
        /// Reference the new file in place instead of copying it.
        #[arg(long, conflicts_with = "managed")]
        external: bool,
        #[arg(long, value_parser=["rec709", "srgb", "linear-srgb"])]
        color_space: Option<String>,
        /// Complete old-to-new stream mapping; repeat for each stream. New IDs come from probe.
        #[arg(long, value_name = "OLD=NEW")]
        #[serde(default)]
        stream_map: Vec<String>,
    },
    /// List/search assets by case-insensitive literal substring; filters combine with AND.
    #[command(visible_alias = "search")]
    List {
        /// Search ID, name, URI, decoded local path and tags.
        query: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, value_parser = ["media", "image", "subtitle", "font", "lut", "other"])]
        kind: Option<String>,
        #[arg(long, value_parser=["file","image-sequence"])]
        source_type: Option<String>,
        /// Exact tag match, ignoring case.
        #[arg(long)]
        tag: Option<String>,
        /// No known built-in references, including disabled items and non-root sequences.
        #[arg(long)]
        unused: bool,
    },
    /// Return the complete asset record and its resolved local path.
    Info {
        asset: String,
        #[arg(long)]
        summary: bool,
    },
    /// Page through an image sequence's fixed frame manifest.
    Frames {
        asset: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Locate direct media/font/LUT references across all sequences.
    Usages { asset: String },
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum SequenceCmd {
    /// Remove an exact interval from all or explicitly selected tracks.
    Cut {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        start: Time,
        #[arg(long)]
        duration: Time,
        #[arg(long, required_unless_present = "track", conflicts_with = "track")]
        all_tracks: bool,
        #[arg(long, value_name = "TRACK_ID", required_unless_present = "all_tracks")]
        track: Vec<String>,
        #[arg(long, default_value="remove-gap", value_parser=["none","remove-gap"])]
        ripple: String,
        /// Auto cuts sequence markers only when every track is selected.
        #[arg(long, default_value="auto", value_parser=["auto","cut","keep"])]
        markers: String,
    },
    /// List all sequences, optionally filtering by ID/name substring.
    List { query: Option<String> },
    /// Locate parent timeline items that embed this sequence.
    Usages { sequence: String },
    Add {
        #[arg(long, default_value = "Sequence")]
        name: String,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        #[arg(long, default_value = "30")]
        fps: Time,
        #[arg(long)]
        duration: Option<Time>,
    },
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum TrackCmd {
    /// List tracks across all sequences, optionally filtering ID/name.
    List {
        query: Option<String>,
        #[arg(long)]
        sequence: Option<String>,
    },
    Add {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long,default_value="video",value_parser=["video","audio","subtitle","data"])]
        kind: String,
        #[arg(long, default_value = "Track")]
        name: String,
        #[arg(long,default_value="forbid",value_parser=["forbid","transition-only","allow"])]
        overlap: String,
    },
}
#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
pub enum ItemCmd {
    /// Find stored items across all sequences (including disabled items).
    #[command(visible_alias = "search")]
    List {
        /// Search item ID/name/tags/text and referenced asset names/paths.
        query: Option<String>,
        #[arg(long)]
        asset: Option<String>,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        track: Option<String>,
        /// Find items covering this sequence-local time, using [start, end).
        #[arg(long, requires = "sequence")]
        at: Option<Time>,
    },
    /// Return a full item and its containing sequence/track, using its exact ID.
    Info {
        item: String,
    },
    /// Set a registered animation slot from a $animation JSON envelope.
    Animate {
        item: String,
        property: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        effect: Option<String>,
        #[arg(long)]
        file: PathBuf,
    },
    /// Constant signed speed; compiles to source/timeline ranges and fit playback.
    Speed {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        rate: String,
        #[arg(long, default_value="none",value_parser=["none","insert","remove-gap"])]
        ripple: String,
        #[arg(long, default_value="keep-local",value_parser=["keep-local","stretch"])]
        animations: String,
        #[arg(long)]
        replace_remap: bool,
        #[arg(long,value_parser=["mute","resample"])]
        audio_policy: Option<String>,
    },
    /// Set an exact source time map; final key offset determines item duration.
    TimeMap {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value="none",value_parser=["none","insert","remove-gap"])]
        ripple: String,
        #[arg(long, default_value="keep-local",value_parser=["keep-local","stretch"])]
        animations: String,
        #[arg(long,value_parser=["mute","resample"])]
        audio_policy: Option<String>,
    },
    /// Set a non-destructive transform; omitted fields keep their current value.
    Transform {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        x: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        y: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        scale_x: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        scale_y: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        rotation: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        anchor_x: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        anchor_y: Option<f64>,
        #[arg(long)]
        crop_left: Option<f64>,
        #[arg(long)]
        crop_right: Option<f64>,
        #[arg(long)]
        crop_top: Option<f64>,
        #[arg(long)]
        crop_bottom: Option<f64>,
        #[arg(long)]
        reset: bool,
    },
    /// Append an ordered visual effect. Parameters are a JSON object, no script required.
    Effect {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long, value_parser=["color-grade","curves","lut","gaussian-blur"])]
        kind: String,
        #[arg(long, default_value = "{}")]
        parameters: String,
    },
    /// Remove one effect by its stable ID.
    EffectRemove {
        item: String,
        effect: String,
        #[arg(long)]
        sequence: Option<String>,
    },
    Add {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        track: String,
        #[arg(long)]
        asset: String,
        #[arg(long)]
        stream: String,
        #[arg(long, default_value = "0")]
        start: Time,
        #[arg(long)]
        duration: Time,
        #[arg(long, default_value = "0")]
        source_start: Time,
        #[arg(long)]
        source_duration: Option<Time>,
    },
    Color {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        track: String,
        #[arg(long, default_value = "0")]
        start: Time,
        #[arg(long)]
        duration: Time,
        #[arg(long, default_value = "#335577")]
        color: String,
    },
    Caption(TextItemArgs),
    /// Add editable native text to a video track.
    Text(TextItemArgs),
    Move {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        track: Option<String>,
        #[arg(long)]
        start: Option<Time>,
        #[arg(long, default_value = "none", value_enum)]
        ripple: Ripple,
    },
    Trim {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        start: Time,
        #[arg(long)]
        duration: Time,
        #[arg(long, requires = "source_duration")]
        source_start: Option<Time>,
        #[arg(long, requires = "source_start")]
        source_duration: Option<Time>,
        #[arg(long, default_value = "none", value_enum)]
        ripple: Ripple,
    },
    Split {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        at: Time,
    },
    Remove {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long)]
        cascade: bool,
        #[arg(long, default_value = "none", value_enum)]
        ripple: Ripple,
    },
}
#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
pub enum Ripple {
    None,
    Insert,
    RemoveGap,
    Overwrite,
}
impl Ripple {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Insert => "insert",
            Self::RemoveGap => "remove-gap",
            Self::Overwrite => "overwrite",
        }
    }
}
fn input(path: &Path) -> Result<Value> {
    if path == Path::new("-") {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(serde_json::from_str(&s)?)
    } else {
        read_json(path)
    }
}
fn sequence(p: &Value, s: Option<String>) -> String {
    s.unwrap_or_else(|| text(&p["rootSequenceId"]).into())
}
fn track(kind: &str, name: &str, overlap: &str) -> Value {
    json!({"id":id("track"),"kind":kind,"name":name,"overlapPolicy":overlap,"items":[]})
}
fn append(store: &Store, aid: &str, override_locks: bool) -> Result<Value> {
    let p = store.project()?;
    let asset = &p["assets"][aid];
    let sid = text(&p["rootSequenceId"]);
    let seq = &p["sequences"][sid];
    let start = project::sequence_duration(seq)?;
    let mut commands = vec![];
    let mut members = vec![];
    for stream in array(&asset["streams"]) {
        if stream["metadata"]["role"] == "attached-picture" {
            continue;
        }
        let kind = match text(&stream["kind"]) {
            "video" | "image" => "video",
            "audio" => "audio",
            _ => continue,
        };
        let duration = if stream["kind"] == "image" {
            Time::integer(5)
        } else {
            Time::from_json(&stream["duration"])
                .context("cannot append media of unknown duration")?
        };
        let mut t = track(
            kind,
            &format!("{} {}", text(&asset["name"]), kind),
            "forbid",
        );
        let iid = id("item");
        let mut item = json!({"id":iid,"type":"media","enabled":true,"effects":[],"assetId":aid,"streamId":stream["id"],"timelineRange":range(&start,&duration)?});
        if stream["kind"] != "image" {
            item["sourceRange"] = range(&Time::zero(), &duration)?;
        }
        t["items"] = json!([item]);
        commands.push(json!({"type":"track.put","sequenceId":sid,"track":t}));
        members.push(iid);
    }
    ensure!(!commands.is_empty(), "no playable streams to append");
    if members.len() > 1 {
        commands.push(json!({"type":"link.put","sequenceId":sid,"link":{"id":id("link"),"kind":"av","itemIds":members}}));
    }
    store.command(
        json!({"type":"batch","commands":commands}),
        override_locks,
        false,
    )
}
fn color(input: &str) -> Result<Value> {
    let s = input.strip_prefix('#').unwrap_or(input);
    ensure!(
        s.len() == 6 || s.len() == 8,
        "color must be #RRGGBB or #RRGGBBAA"
    );
    ensure!(s.bytes().all(|c| c.is_ascii_hexdigit()), "invalid color");
    let values = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map(|v| v as f64 / 255.0))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(json!({"r":values[0],"g":values[1],"b":values[2],"a":values.get(3).copied().unwrap_or(1.0)}))
}
/// The command adapter and the server use the same typed application requests.
/// No server or media runtime is created for ordinary one-shot commands.
pub struct ApplicationHost;
impl ApplicationHost {
    pub fn execute(&self, request: Cli) -> Result<Value> {
        execute(request)
    }
}

pub fn dispatch(cli: Cli) -> Result<Value> {
    let cli = prepare(cli)?;
    if !matches!(
        cli.command,
        Cmd::Open { .. }
            | Cmd::Close { .. }
            | Cmd::Serve { .. }
            | Cmd::Id { .. }
            | Cmd::Schema { .. }
            | Cmd::Capabilities
            | Cmd::Doctor { .. }
            | Cmd::Init { .. }
    ) {
        if let Some(result) = crate::daemon::route(&cli)? {
            return Ok(result);
        }
    }
    ApplicationHost.execute(cli)
}

/// Resolve caller-relative inputs before crossing a process boundary. In
/// particular, a service must never read its own stdin for a client's `-`.
pub fn prepare(mut cli: Cli) -> Result<Cli> {
    let cwd = std::env::current_dir()?;
    let absolute = |path: &mut PathBuf| {
        if path.is_relative() && path != Path::new("-") {
            *path = cwd.join(&*path);
        }
    };
    absolute(&mut cli.project);
    let mut capture = |path: &mut PathBuf| -> Result<()> {
        absolute(path);
        if !cli.inputs.contains_key(path) {
            cli.inputs.insert(path.clone(), input(path)?);
        }
        Ok(())
    };
    match &mut cli.command {
        // Reject NaN/Infinity before JSON transport can turn them into null.
        Cmd::Preview { scale, .. } => crate::preview::validate_scale(*scale)?,
        Cmd::Open { path, .. } | Cmd::Close { path } => {
            if let Some(path) = path {
                absolute(path);
            }
        }
        Cmd::Init { directory, .. } => absolute(directory),
        Cmd::Apply { file } | Cmd::Command { file, .. } => capture(file)?,
        Cmd::Validate {
            file: Some(file), ..
        } => {
            absolute(file);
            if file.file_name().is_some_and(|n| n == "cutcraft.json") {
                // Canonical snapshots must be read by Store under its OS lock.
                cli.project = file
                    .parent()
                    .context("project file has no parent")?
                    .to_path_buf();
            } else {
                capture(file)?;
            }
        }
        Cmd::Import { file, .. }
        | Cmd::Probe { file }
        | Cmd::Asset {
            command: AssetCmd::Replace { file, .. },
        } => absolute(file),
        Cmd::ImportSequence {
            directory, files, ..
        } => {
            absolute(directory);
            if let Some(file) = files {
                capture(file)?;
            }
        }
        Cmd::Transition {
            progress: Some(file),
            ..
        } => capture(file)?,
        Cmd::Item { command } => match command {
            ItemCmd::Animate { file, .. } | ItemCmd::TimeMap { file, .. } => capture(file)?,
            ItemCmd::Caption(args) | ItemCmd::Text(args) => {
                args.card.value()?;
                if let Some(file) = args.text_file.take() {
                    args.text = Some(
                        std::fs::read_to_string(&file)
                            .with_context(|| format!("read UTF-8 text {}", file.display()))?,
                    );
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(cli)
}

fn open_store(path: &Path) -> Result<Store> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match Store::open(path) {
            Err(error)
                if format!("{error:#}").contains("PROJECT_LOCKED")
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

pub fn execute(cli: Cli) -> Result<Value> {
    execute_inner(cli, None)
}

pub fn execute_with_progress(cli: Cli, progress: &crate::progress::Progress) -> Result<Value> {
    crate::media_process::with_cancellation(progress.cancellation(), || {
        execute_inner(cli, Some(progress))
    })
}

fn execute_inner(cli: Cli, reporter: Option<&crate::progress::Progress>) -> Result<Value> {
    let input = |path: &Path| -> Result<Value> {
        if let Some(value) = cli.inputs.get(path) {
            Ok(value.clone())
        } else {
            input(path)
        }
    };
    let dry = cli.dry_run;
    let override_locks = cli.override_locks;
    match cli.command {
        Cmd::Open {
            path,
            no_browser,
            host,
        } => {
            ensure!(!dry, "--dry-run is for edit commands");
            return crate::daemon::open(path.as_deref().unwrap_or(&cli.project), no_browser, host);
        }
        Cmd::Close { path } => {
            ensure!(!dry, "--dry-run is for edit commands");
            return crate::daemon::close(path.as_deref().unwrap_or(&cli.project));
        }
        Cmd::Serve { host } => {
            crate::daemon::serve(host)?;
            return Ok(json!({"ok":true}));
        }
        Cmd::Asset {
            command:
                AssetCmd::Replace {
                    asset,
                    file,
                    managed,
                    external,
                    color_space,
                    stream_map,
                },
        } => {
            ensure!(
                !(managed && external),
                "--managed and --external are mutually exclusive"
            );
            let store = open_store(&cli.project)?;
            let project = store.project()?;
            let dir = store.dir.clone();
            drop(store);
            let prepared = media::prepare_replace(
                &project,
                &dir,
                &asset,
                &file,
                &media::ReplaceOptions {
                    managed: if managed {
                        Some(true)
                    } else if external {
                        Some(false)
                    } else {
                        None
                    },
                    color_space: color_space.as_deref(),
                    stream_map: &stream_map,
                    dry_run: dry,
                },
            )?;
            let store = open_store(&dir)?;
            return media::commit_replace(&store, prepared, override_locks);
        }
        Cmd::Import {
            file,
            managed,
            append: do_append,
            color_space,
        } => {
            ensure!(
                !dry,
                "--dry-run is for edit commands; use probe for read-only media inspection"
            );
            ensure!(
                !(do_append
                    && file
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("cube"))),
                "LUT assets are effects, not timeline items; omit --append"
            );
            let store = open_store(&cli.project)?;
            let expected_id = store.project()?["id"].clone();
            let dir = store.dir.clone();
            drop(store);
            let prepared = media::prepare_import(&dir, &file, managed, color_space.as_deref())?;
            if let Some(progress) = reporter {
                progress.check_cancelled()?;
            }
            let store = open_store(&dir)?;
            ensure!(store.project()?["id"] == expected_id, "PROJECT_REPLACED");
            if let Some(progress) = reporter {
                progress.check_cancelled()?;
            }
            let imported = media::commit_import(&store, prepared)?;
            return if do_append {
                let appended = append(&store, text(&imported["assetId"]), override_locks)?;
                Ok(json!({"ok":true,"import":imported,"append":appended}))
            } else {
                Ok(imported)
            };
        }
        Cmd::ImportSequence {
            directory,
            pattern,
            files,
            pattern_style,
            start_number,
            end_number,
            missing,
            fps,
            color_space,
            managed,
            append,
        } => {
            ensure!(!dry, "--dry-run is not supported for image-sequence import");
            let prepared_list = if let Some(value) = files.as_ref().and_then(|f| cli.inputs.get(f))
            {
                let file = tempfile::NamedTempFile::new()?;
                serde_json::to_writer(file.as_file(), value)?;
                Some(file)
            } else {
                None
            };
            let files = prepared_list
                .as_ref()
                .map(|f| f.path().to_path_buf())
                .or(files);
            let store = open_store(&cli.project)?;
            let p = store.project()?;
            ensure!(crate::capabilities::supports(text(&p["specVersion"]),crate::capabilities::Capability::ImageSequences), "PROJECT_UPGRADE_REQUIRED: image sequences require Spec 0.1.0 or internal draft 0.5.0");
            let dir = store.dir.clone();
            drop(store);
            let options = crate::image_sequence::ImportOptions {
                directory,
                pattern,
                files,
                pattern_style: pattern_style.unwrap_or_else(|| "printf".into()),
                start: start_number,
                end: end_number,
                missing: missing.unwrap_or_else(|| "error".into()),
                fps,
                color_space,
                managed,
                append,
            };
            let prepared = crate::image_sequence::prepare_import(&dir, &options)?;
            if let Some(progress) = reporter {
                progress.check_cancelled()?;
            }
            let store = open_store(&dir)?;
            ensure!(store.project()?["id"] == p["id"], "PROJECT_REPLACED");
            if let Some(progress) = reporter {
                progress.check_cancelled()?;
            }
            return crate::image_sequence::commit_import(&store, prepared, override_locks);
        }
        Cmd::Id { prefix } => return Ok(json!({"id":id(&prefix)})),
        Cmd::Capabilities => return Ok(render::capabilities()),
        Cmd::Doctor {
            timeout,
            video_encoder,
        } => return crate::doctor::run_with_encoder(timeout, video_encoder),
        Cmd::Schema { kind } => {
            return Ok(serde_json::from_str(
                schema::SCHEMAS
                    .iter()
                    .find(|(k, _)| *k == kind.name())
                    .unwrap()
                    .1,
            )?)
        }
        Cmd::Probe { file } => return media::probe(&file.canonicalize()?),
        Cmd::Init { directory, name } => {
            ensure!(!dry, "--dry-run is for edit commands");
            let s = Store::init(&directory, &name)?;
            return Ok(json!({"ok":true,"directory":s.dir,"project":s.project()?}));
        }
        Cmd::Validate { file, kind, level } => {
            let path = file.unwrap_or_else(|| cli.project.join("cutcraft.json"));
            let _snapshot_lock = if path.file_name().is_some_and(|n| n == "cutcraft.json") {
                Some(Store::open(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(std::path::Path::new(".")),
                )?)
            } else {
                None
            };
            let value = input(&path)?;
            schema::validate(kind.name(), &value)?;
            if !matches!(level, Level::Schema) {
                match kind {
                    Kind::Project => project::validate(&value)?,
                    Kind::Event => {
                        let before = value["revisionBefore"].as_u64().unwrap();
                        let after = value["revisionAfter"].as_u64().unwrap();
                        ensure!(
                            if value["status"] == "applied" {
                                before.checked_add(1) == Some(after)
                            } else {
                                before == after
                            },
                            "invalid event revisions"
                        );
                    }
                    Kind::Operation => {
                        let store = open_store(&cli.project)?;
                        edit::apply(&store.project()?, &value, override_locks)?;
                    }
                    Kind::Animation => {
                        let duration = crate::project::array(&value["$animation"]["keyframes"])
                            .iter()
                            .map(|k| Time::from_json(&k["offset"]))
                            .collect::<Result<Vec<_>>>()?
                            .into_iter()
                            .max()
                            .unwrap_or(Time::zero());
                        crate::animation::validate(&value, &duration)?;
                    }
                    Kind::Common | Kind::Generator | Kind::TextCard | Kind::Effect => {}
                }
            }
            if matches!(level, Level::Media) {
                ensure!(
                    matches!(kind, Kind::Project),
                    "media validation applies to projects"
                );
                return media::validate(
                    &value,
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(&cli.project),
                );
            }
            return Ok(json!({"ok":true,"kind":kind.name(),"path":path}));
        }
        _ => {}
    }
    let store = open_store(&cli.project)?;
    match cli.command {
        Cmd::Cache { command } => {
            let manager = crate::cache::Manager::new(&store.dir);
            let project = store.project()?;
            match command {
                CacheCmd::Status => manager.status(&project),
                CacheCmd::Clean => manager.clean(&project, dry),
            }
        },
        Cmd::Inspect=>store.project(),Cmd::History{command}=>match command {
            None=>Ok(json!(store.history()?)),
            Some(HistoryCmd::Undo)=>store.history_undo(override_locks,dry),
            Some(HistoryCmd::Redo)=>store.history_redo(override_locks,dry),
        },
        Cmd::Apply{file}=>store.apply(&input(&file)?,override_locks,dry),
        Cmd::Command{file,base_revision}=>{let mut op=edit::operation(&store.project()?,input(&file)?);if let Some(r)=base_revision {op["baseRevision"]=json!(r);}store.apply(&op,override_locks,dry)},
        Cmd::Undo=>store.undo(override_locks,dry),
        Cmd::Asset{command}=>{
            let p=store.project()?;
            match command {
                AssetCmd::List{query,name,path,kind,source_type,tag,unused}=>Ok(query::assets(&p,&store.dir,&query::AssetFilter{query,name,path,kind,source_type,tag,unused})),
                AssetCmd::Info{asset,summary}=>query::asset_info_summary(&p,&store.dir,&asset,summary),
                AssetCmd::Frames{asset,offset,limit}=>query::asset_frames(&p,&store.dir,&asset,offset,limit),
                AssetCmd::Usages{asset}=>query::asset_usages(&p,&asset),
                AssetCmd::Replace{..}=>unreachable!("replacement handled before opening the editing lock"),
            }
        },
        Cmd::Sequence{command:SequenceCmd::Cut{sequence:s,start,duration,all_tracks,track,ripple,markers}}=>{
            let p=store.project()?;
            let sid=sequence(&p,s);
            let plan=crate::range_edit::cut(&p,&sid,&crate::range_edit::CutOptions{
                start:&start,duration:&duration,all_tracks,tracks:&track,ripple:&ripple,markers:&markers,
            })?;
            let mut operation=edit::operation(&p,plan.command);
            operation["metadata"]=json!({"cutcraft.rangeEdit":plan.summary});
            let mut result=store.apply(&operation,override_locks,dry)?;
            result["cut"]=plan.summary;
            Ok(result)
        },
        Cmd::Sequence{command:SequenceCmd::List{query}}=>Ok(query::sequences(&store.project()?,query.as_deref())),
        Cmd::Sequence{command:SequenceCmd::Usages{sequence}}=>query::sequence_usages(&store.project()?,&sequence),
        Cmd::Track{command:TrackCmd::List{query,sequence}}=>query::tracks(&store.project()?,sequence.as_deref(),query.as_deref()),
        Cmd::Sequence{command:SequenceCmd::Add{name,width,height,fps,duration}}=>{
            let p=store.project()?;let mut seq=project::new(&name)["sequences"].as_object().unwrap().values().next().unwrap().clone();
            seq["name"]=json!(name);seq["format"]["width"]=json!(width);seq["format"]["height"]=json!(height);seq["format"]["frameRate"]=fps.json()?;
            if let Some(d)=duration {seq["durationMode"]=json!("fixed");seq["duration"]=d.json()?;}
            let _=p;store.command(json!({"type":"sequence.put","sequenceId":id("seq"),"sequence":seq}),override_locks,dry)
        },
        Cmd::Track{command:TrackCmd::Add{sequence:s,kind,name,overlap}}=>store.command(json!({"type":"track.put","sequenceId":sequence(&store.project()?,s),"track":track(&kind,&name,&overlap)}),override_locks,dry),
        Cmd::Item{command}=>{
            let p=store.project()?;let command=match command {
                ItemCmd::List{query,asset,sequence,track,at}=>return query::items(&p,&store.dir,&query::ItemFilter{query,asset,sequence,track,at}),
                ItemCmd::Info{item}=>return query::item_info(&p,&item),
                ItemCmd::Animate{item,property,sequence:s,effect,file}=>{
                    let sid=sequence(&p,s);let (track,mut value)=find_item(&p,&sid,&item)?;
                    crate::timeline::animate(&mut value,&property,effect.as_deref(),input(&file)?)?;
                    json!({"type":"item.put","sequenceId":sid,"trackId":track,"item":value})
                },
                ItemCmd::Speed{item,sequence:s,rate,ripple,animations,replace_remap,audio_policy}=>crate::timeline::retime(&p,&sequence(&p,s),&item,None,Some(&rate),&ripple,&animations,replace_remap,audio_policy.as_deref())?,
                ItemCmd::TimeMap{item,sequence:s,file,ripple,animations,audio_policy}=>crate::timeline::retime(&p,&sequence(&p,s),&item,Some(input(&file)?),None,&ripple,&animations,true,audio_policy.as_deref())?,
                ItemCmd::Transform{item,sequence:s,x,y,scale_x,scale_y,rotation,anchor_x,anchor_y,crop_left,crop_right,crop_top,crop_bottom,reset}=>{
                    let sid=sequence(&p,s);let (track,mut value)=find_item(&p,&sid,&item)?;
                    if reset {value["video"]["transform"]=json!({});}
                    for (group,key,n) in [("position","x",x),("position","y",y),("scale","x",scale_x),("scale","y",scale_y),("anchor","x",anchor_x),("anchor","y",anchor_y),("crop","left",crop_left),("crop","right",crop_right),("crop","top",crop_top),("crop","bottom",crop_bottom)] {
                        if let Some(n)=n {
                            ensure!(n.is_finite(),"transform value must be finite");
                            if value["video"]["transform"][group].is_null() && group!="crop" {let default=if group=="scale" {1.}else{0.5};value["video"]["transform"][group]=json!({"x":default,"y":default});}
                            value["video"]["transform"][group][key]=json!(n);
                        }
                    }
                    if let Some(n)=rotation {ensure!(n.is_finite(),"rotation must be finite");value["video"]["transform"]["rotationDegrees"]=json!(n);}
                    json!({"type":"item.put","sequenceId":sid,"trackId":track,"item":value})
                },
                ItemCmd::Effect{item,sequence:s,kind,parameters}=>{
                    let sid=sequence(&p,s);let (track,mut value)=find_item(&p,&sid,&item)?;
                    let parameters:Value=serde_json::from_str(&parameters)?;
                    ensure!(parameters.is_object(),"effect parameters must be a JSON object");
                    if value["effects"].is_null(){value["effects"]=json!([]);}
                    value["effects"].as_array_mut().unwrap().push(json!({"id":id("effect"),"effectType":format!("builtin.{kind}"),"version":if kind=="curves" || kind=="gaussian-blur" || p["specVersion"]=="0.3.0" {"1.0.0"}else{"2.0.0"},"parameters":parameters}));
                    json!({"type":"item.put","sequenceId":sid,"trackId":track,"item":value})
                },
                ItemCmd::EffectRemove{item,effect,sequence:s}=>{
                    let sid=sequence(&p,s);let (track,mut value)=find_item(&p,&sid,&item)?;
                    ensure!(array(&value["effects"]).iter().any(|e|e["id"]==effect),"effect not found");
                    value["effects"].as_array_mut().unwrap().retain(|e|e["id"]!=effect);
                    json!({"type":"item.put","sequenceId":sid,"trackId":track,"item":value})
                },
                ItemCmd::Add{sequence:s,track,asset,stream,start,duration,source_start,source_duration}=>{
                    let info=array(&p["assets"][&asset]["streams"]).iter().find(|s|s["id"]==stream).context("asset/stream not found")?;
                    let mut item=json!({"id":id("item"),"type":"media","assetId":asset,"streamId":stream,"timelineRange":range(&start,&duration)?});
                    if info["kind"]!="image" {item["sourceRange"]=range(&source_start,&source_duration.unwrap_or(duration))?;}
                    json!({"type":"item.put","sequenceId":sequence(&p,s),"trackId":track,"item":item})
                },
                ItemCmd::Color{sequence:s,track,start,duration,color:c}=>json!({"type":"item.put","sequenceId":sequence(&p,s),"trackId":track,"item":{"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":color(&c)?},"timelineRange":range(&start,&duration)?}}),
                ItemCmd::Caption(args)=>{let sid=sequence(&p,args.sequence.clone());args.command(&sid,true)?},
                ItemCmd::Text(args)=>{let sid=sequence(&p,args.sequence.clone());args.command(&sid,false)?},
                ItemCmd::Move{item,sequence:s,track,start,ripple}=>{
                    let mut c=json!({"type":"item.move","sequenceId":sequence(&p,s),"itemId":item,"ripple":ripple.name()});
                    if let Some(track)=track {c["toTrackId"]=json!(track);}
                    if let Some(start)=start {c["timelineStart"]=start.json()?;}c
                },
                ItemCmd::Trim{item,sequence:s,start,duration,source_start,source_duration,ripple}=>{
                    let mut c=json!({"type":"item.trim","sequenceId":sequence(&p,s),"itemId":item,"timelineRange":range(&start,&duration)?,"ripple":ripple.name()});
                    if let (Some(a),Some(b))=(source_start,source_duration) {c["sourceRange"]=range(&a,&b)?;}c
                },
                ItemCmd::Split{item,sequence:s,at}=>json!({"type":"item.split","sequenceId":sequence(&p,s),"itemId":item,"at":at.json()?,"leftItemId":id("item"),"rightItemId":id("item")}),
                ItemCmd::Remove{item,sequence:s,cascade,ripple}=>json!({"type":"item.remove","sequenceId":sequence(&p,s),"itemId":item,"cascade":cascade,"ripple":ripple.name()}),
            };store.command(command,override_locks,dry)
        },
        Cmd::Transition{from,to,sequence:s,kind,duration,alignment,progress,color:c}=>{
            let p=store.project()?;let sid=sequence(&p,s);let mut tr=json!({"id":id("transition"),"fromItemId":from,"toItemId":to,"transitionType":format!("builtin.{kind}"),"version":"2.0.0","duration":duration.json()?,"alignment":alignment});
            if kind=="dip-to-color"{tr["parameters"]=json!({"color":color(&c)?});}
            if let Some(path)=progress{tr["progress"]=input(&path)?;}
            store.command(json!({"type":"transition.put","sequenceId":sid,"transition":tr}),override_locks,dry)
        },
        Cmd::Marker{sequence:s,name,at,duration}=>store.command(json!({"type":"marker.put","sequenceId":sequence(&store.project()?,s),"marker":{"id":id("marker"),"name":name,"start":at.json()?,"duration":duration.json()?}}),override_locks,dry),
        Cmd::Sample{sequence:s,at}=>{let p=store.project()?;project::sample(&p,&sequence(&p,s),&at)},
        Cmd::Render{sequence:s,output,overwrite,progress,encoding}=>{ensure!(!dry,"--dry-run is for edit commands");let p=store.project()?;let output=if output.is_absolute(){output}else{store.dir.join(output)};let dir=store.dir.clone();drop(store);render::render_with_options(&p,&dir,&sequence(&p,s),&output,overwrite,&encoding,reporter.unwrap_or(&progress.reporter()))},
        Cmd::Preview{sequence:s,at,start,duration,scale,progress,sheet,boundaries,count,columns,interval,encoding}=>{
            ensure!(!dry,"--dry-run is for edit commands");
            let p=store.project()?;let dir=store.dir.clone();drop(store);
            let local_progress = progress.reporter();
            let progress = reporter.unwrap_or(&local_progress);
            let review = crate::preview_review::Options { sheet, boundaries, count, columns, interval };
            let selection = crate::preview::ExportOptions { at, start, duration, scale };
            review.validate(&selection)?;
            encoding.validate()?;
            ensure!(!(sheet || boundaries || selection.at.is_some()) || encoding.is_default(), "ENCODING_OPTIONS_CONFLICT: video encoding only applies to MP4 previews");
            if sheet || boundaries {
                crate::preview_review::export(&p,&dir,&sequence(&p,s),&selection,&review,progress)
            } else if selection.at.is_none() && selection.start.is_none() && selection.duration.is_none() && scale.is_none() {
                render::preview_with_encoding(&p,&dir,&sequence(&p,s),&encoding,progress)
            } else {
                crate::preview::export_with_encoding(&p,&dir,&sequence(&p,s),&selection,&encoding,progress)
            }
        },
        _=>unreachable!()
    }
}

fn find_item(p: &Value, sid: &str, item: &str) -> anyhow::Result<(String, Value)> {
    for track in array(&p["sequences"][sid]["tracks"]) {
        for value in array(&track["items"]) {
            if value["id"] == item {
                return Ok((text(&track["id"]).to_string(), value.clone()));
            }
        }
    }
    anyhow::bail!("item not found in sequence: {item}")
}
