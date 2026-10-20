use anyhow::{ensure, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use cutcraft::{
    edit, id, media,
    project::{self, array, text},
    query, render, schema,
    store::{read_json, Store},
    time::{range, Time},
};
use serde_json::{json, Value};
use std::{
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    name = "cutcraft",
    version,
    about = "Agent-first non-linear editing. All command results are JSON."
)]
struct Cli {
    #[arg(
        short = 'p',
        long,
        global = true,
        default_value = ".",
        help = "Project directory containing cutcraft.json"
    )]
    project: PathBuf,
    #[arg(
        long,
        global = true,
        help = "Explicitly permit mutations of locked tracks"
    )]
    override_locks: bool,
    #[arg(
        long,
        global = true,
        help = "Validate an edit without writing project or history"
    )]
    dry_run: bool,
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
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
    History,
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
    /// Import a frozen image sequence as one video asset (Spec 0.5).
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
    /// Search media-library records and locate their timeline uses.
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
    },
    /// Export a low-resolution MP4 preview (max edge 640, max 24 fps).
    Preview {
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long, value_enum, default_value = "auto")]
        progress: ProgressMode,
    },
    /// Describe the media backend's supported profile.
    Capabilities,
}
#[derive(Clone, Copy, ValueEnum)]
enum ProgressMode {
    Auto,
    None,
    Human,
    Json,
}
impl ProgressMode {
    fn reporter(self) -> cutcraft::progress::Progress {
        use cutcraft::progress::{Mode, Progress};
        Progress::new(match self {
            Self::Auto if std::io::stderr().is_terminal() => Mode::Human,
            Self::Auto | Self::None => Mode::None,
            Self::Human => Mode::Human,
            Self::Json => Mode::Json,
        })
    }
}

#[derive(Args)]
struct TextItemArgs {
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
    #[arg(long)]
    font_asset: Option<String>,
    #[arg(long)]
    font_size: Option<f64>,
    #[arg(long)]
    color: Option<String>,
    #[arg(long, value_parser = ["left", "center", "right"])]
    align: Option<String>,
    #[arg(long, value_parser = ["top", "center", "bottom"])]
    position: Option<String>,
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
#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Project,
    Operation,
    Event,
    Common,
    Generator,
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
            Self::Effect => "effect",
            Self::Animation => "animation",
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum Level {
    Schema,
    Semantic,
    Media,
}
#[derive(Subcommand)]
enum AssetCmd {
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
#[derive(Subcommand)]
enum SequenceCmd {
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
#[derive(Subcommand)]
enum TrackCmd {
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
#[derive(Subcommand)]
enum ItemCmd {
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
    /// Append an ordered color effect. Parameters are a JSON object, no script required.
    Effect {
        item: String,
        #[arg(long)]
        sequence: Option<String>,
        #[arg(long, value_parser=["color-grade","curves","lut"])]
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
#[derive(Clone, ValueEnum)]
enum Ripple {
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
fn run(cli: Cli) -> Result<Value> {
    let dry = cli.dry_run;
    let override_locks = cli.override_locks;
    match cli.command {
        Cmd::Id { prefix } => return Ok(json!({"id":id(&prefix)})),
        Cmd::Capabilities => return Ok(render::capabilities()),
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
                        let store = Store::open(&cli.project)?;
                        edit::apply(&store.project()?, &value, override_locks)?;
                    }
                    Kind::Animation => {
                        let duration = cutcraft::project::array(&value["$animation"]["keyframes"])
                            .iter()
                            .map(|k| Time::from_json(&k["offset"]))
                            .collect::<Result<Vec<_>>>()?
                            .into_iter()
                            .max()
                            .unwrap_or(Time::zero());
                        cutcraft::animation::validate(&value, &duration)?;
                    }
                    Kind::Common | Kind::Generator | Kind::Effect => {}
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
    let store = Store::open(&cli.project)?;
    match cli.command {
        Cmd::Inspect=>store.project(),Cmd::History=>Ok(json!(store.history()?)),
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
            }
        },
        Cmd::Sequence{command:SequenceCmd::List{query}}=>Ok(query::sequences(&store.project()?,query.as_deref())),
        Cmd::Sequence{command:SequenceCmd::Usages{sequence}}=>query::sequence_usages(&store.project()?,&sequence),
        Cmd::Track{command:TrackCmd::List{query,sequence}}=>query::tracks(&store.project()?,sequence.as_deref(),query.as_deref()),
        Cmd::Import{file,managed,append:do_append,color_space}=>{
            ensure!(!dry,"--dry-run is for edit commands; use probe for read-only media inspection");
            ensure!(!(do_append && file.extension().is_some_and(|e|e.eq_ignore_ascii_case("cube"))), "LUT assets are effects, not timeline items; omit --append");
            let imported=media::import_with_color(&store,&file,managed,color_space.as_deref())?;
            if do_append {let appended=append(&store,text(&imported["assetId"]),override_locks)?;Ok(json!({"ok":true,"import":imported,"append":appended}))}else{Ok(imported)}
        },
        Cmd::ImportSequence{directory,pattern,files,pattern_style,start_number,end_number,missing,fps,color_space,managed,append}=>{
            ensure!(!dry,"--dry-run is not supported for image-sequence import");
            cutcraft::image_sequence::import(&store,&cutcraft::image_sequence::ImportOptions{directory,pattern,files,pattern_style:pattern_style.unwrap_or_else(||"printf".into()),start:start_number,end:end_number,missing:missing.unwrap_or_else(||"error".into()),fps,color_space,managed,append},override_locks)
        },
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
                    cutcraft::timeline::animate(&mut value,&property,effect.as_deref(),input(&file)?)?;
                    json!({"type":"item.put","sequenceId":sid,"trackId":track,"item":value})
                },
                ItemCmd::Speed{item,sequence:s,rate,ripple,animations,replace_remap,audio_policy}=>cutcraft::timeline::retime(&p,&sequence(&p,s),&item,None,Some(&rate),&ripple,&animations,replace_remap,audio_policy.as_deref())?,
                ItemCmd::TimeMap{item,sequence:s,file,ripple,animations,audio_policy}=>cutcraft::timeline::retime(&p,&sequence(&p,s),&item,Some(input(&file)?),None,&ripple,&animations,true,audio_policy.as_deref())?,
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
                    value["effects"].as_array_mut().unwrap().push(json!({"id":id("effect"),"effectType":format!("builtin.{kind}"),"version":if kind=="curves" || p["specVersion"]=="0.3.0" {"1.0.0"}else{"2.0.0"},"parameters":parameters}));
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
        Cmd::Render{sequence:s,output,overwrite,progress}=>{ensure!(!dry,"--dry-run is for edit commands");let p=store.project()?;let output=if output.is_absolute(){output}else{store.dir.join(output)};let dir=store.dir.clone();drop(store);render::render_with_progress(&p,&dir,&sequence(&p,s),&output,overwrite,&progress.reporter())},
        Cmd::Preview{sequence:s,progress}=>{ensure!(!dry,"--dry-run is for edit commands");let p=store.project()?;let dir=store.dir.clone();drop(store);render::preview_with_progress(&p,&dir,&sequence(&p,s),&progress.reporter())},
        _=>unreachable!()
    }
}
fn main() {
    match run(Cli::parse()) {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            let mut detail = json!({"message":format!("{error:#}")});
            if let Some(tool) = error.downcast_ref::<cutcraft::media_process::MediaToolError>() {
                detail["code"] = json!(tool.code());
                detail["executable"] = json!(tool.executable);
                detail["hint"] = json!(tool.hint());
            }
            eprintln!("{}", json!({"ok":false,"error":detail}));
            std::process::exit(1);
        }
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
