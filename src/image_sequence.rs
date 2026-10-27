//! Frozen image-sequence manifests. Discovery is an import operation, never a
//! renderer operation; all sampling uses the same rational source clock as video.
use crate::media_process::MediaCommand;
use crate::{
    animation, id, media,
    project::{self, array, text},
    store::{self, Store},
    time::{bounds, range, Time},
};
use anyhow::{bail, ensure, Context, Result};
use num_traits::ToPrimitive;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    process::Command,
};

pub const MAX_FRAMES: usize = 1_000_000;
pub fn is_sequence(a: &Value) -> bool {
    a["source"]["type"] == "image-sequence"
}

#[derive(Debug)]
pub struct Pattern {
    prefix: String,
    suffix: String,
    padding: usize,
}
impl Pattern {
    pub fn parse(input: &str, style: &str) -> Result<Self> {
        ensure!(
            !input.contains('/') && !input.contains('\\'),
            "INVALID_SEQUENCE_PATTERN: basename only"
        );
        let mut parts = [String::new(), String::new()];
        let mut padding = None;
        let mut chars = input.chars().peekable();
        ensure!(matches!(style, "printf" | "hash"), "INVALID_PATTERN_STYLE");
        while let Some(c) = chars.next() {
            let side = usize::from(padding.is_some());
            if style == "hash" && c == '#' {
                ensure!(
                    padding.is_none(),
                    "INVALID_SEQUENCE_PATTERN: multiple slots"
                );
                let mut n = 1;
                while chars.peek() == Some(&'#') {
                    chars.next();
                    n += 1;
                }
                ensure!(n <= 19, "INVALID_SEQUENCE_PATTERN: width must be 1..19");
                padding = Some(n);
            } else if style == "printf" && c == '%' {
                match chars.next() {
                    Some('%') => parts[side].push('%'),
                    Some('d') => {
                        ensure!(
                            padding.is_none(),
                            "INVALID_SEQUENCE_PATTERN: multiple slots"
                        );
                        padding = Some(0);
                    }
                    Some('0') => {
                        ensure!(
                            padding.is_none(),
                            "INVALID_SEQUENCE_PATTERN: multiple slots"
                        );
                        let mut digits = String::new();
                        while chars.peek().is_some_and(char::is_ascii_digit) {
                            digits.push(chars.next().unwrap());
                        }
                        ensure!(
                            !digits.starts_with('0') && chars.next() == Some('d'),
                            "INVALID_SEQUENCE_PATTERN: use %d or %0Nd"
                        );
                        let n: usize = digits
                            .parse()
                            .context("INVALID_SEQUENCE_PATTERN: missing width")?;
                        ensure!(
                            (1..=19).contains(&n),
                            "INVALID_SEQUENCE_PATTERN: width must be 1..19"
                        );
                        padding = Some(n);
                    }
                    _ => bail!("INVALID_SEQUENCE_PATTERN: use %d, %0Nd or %%"),
                }
            } else {
                parts[side].push(c);
            }
        }
        let [prefix, suffix] = parts;
        Ok(Self {
            prefix,
            suffix,
            padding: padding.context("INVALID_SEQUENCE_PATTERN: missing slot")?,
        })
    }
    pub fn number(&self, name: &str) -> Option<i64> {
        let digits = name
            .strip_prefix(&self.prefix)?
            .strip_suffix(&self.suffix)?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n = digits.parse::<i64>().ok()?;
        (format!("{n:0width$}", width = self.padding) == digits).then_some(n)
    }
}

pub struct ImportOptions {
    pub directory: PathBuf,
    pub pattern: Option<String>,
    pub files: Option<PathBuf>,
    pub pattern_style: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub missing: String,
    pub fps: Time,
    pub color_space: Option<String>,
    pub managed: bool,
    pub append: bool,
}
#[derive(Debug)]
pub struct Discovery {
    pub files: Vec<PathBuf>,
    pub missing_ranges: Vec<(i64, i64)>,
}
pub fn discover(root: &Path, o: &ImportOptions) -> Result<Discovery> {
    ensure!(
        o.pattern.is_some() != o.files.is_some(),
        "choose exactly one of --pattern / --files"
    );
    ensure!(
        matches!(o.missing.as_str(), "error" | "hold" | "skip"),
        "INVALID_MISSING_POLICY"
    );
    if let Some(file) = &o.files {
        ensure!(
            o.start.is_none()
                && o.end.is_none()
                && o.missing == "error"
                && o.pattern_style == "printf",
            "pattern options cannot be used with --files"
        );
        let list = store::read_json(file)?;
        let list = list
            .as_array()
            .context("frame list must be a JSON array of paths")?;
        ensure!(
            !list.is_empty() && list.len() <= MAX_FRAMES,
            "INVALID_FRAME_COUNT: limit {MAX_FRAMES}"
        );
        let mut files = vec![];
        for entry in list {
            let path = PathBuf::from(
                entry
                    .as_str()
                    .context("frame list member must be a path string")?,
            );
            ensure!(
                !path.as_os_str().is_empty()
                    && path
                        .components()
                        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
                "FRAME_PATH_INVALID: relative paths inside DIRECTORY only"
            );
            ensure!(
                root.join(&path).is_file(),
                "MEDIA_MISSING: {}",
                path.display()
            );
            files.push(path);
        }
        return Ok(Discovery {
            files,
            missing_ranges: vec![],
        });
    }
    let pat = Pattern::parse(o.pattern.as_deref().unwrap(), &o.pattern_style)?;
    ensure!(
        o.start.is_none_or(|n| n >= 0) && o.end.is_none_or(|n| n >= 0),
        "frame numbers must be nonnegative"
    );
    ensure!(
        !matches!((o.start,o.end),(Some(a),Some(b)) if a>b),
        "INVALID_FRAME_RANGE"
    );
    let mut found = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name();
        if let Some(n) = name.to_str().and_then(|s| pat.number(s)) {
            if o.start.is_some_and(|a| n < a) || o.end.is_some_and(|b| n > b) {
                continue;
            }
            ensure!(
                found.insert(n, PathBuf::from(name)).is_none(),
                "DUPLICATE_FRAME_NUMBER: {n}"
            );
            ensure!(
                found.len() <= MAX_FRAMES,
                "INVALID_FRAME_COUNT: limit {MAX_FRAMES}"
            );
        }
    }
    let start = o
        .start
        .or_else(|| found.first_key_value().map(|(n, _)| *n))
        .context("NO_MATCHING_FRAMES")?;
    let end = o
        .end
        .or_else(|| found.last_key_value().map(|(n, _)| *n))
        .context("NO_MATCHING_FRAMES")?;
    ensure!(!found.is_empty(), "NO_MATCHING_FRAMES");
    let count = end
        .checked_sub(start)
        .and_then(|n| n.checked_add(1))
        .context("INVALID_FRAME_RANGE")?;
    ensure!(
        count > 0 && count as u64 <= MAX_FRAMES as u64,
        "INVALID_FRAME_COUNT: range limit {MAX_FRAMES}"
    );
    let mut files = vec![];
    let mut missing_ranges: Vec<(i64, i64)> = vec![];
    let mut previous = None;
    for n in start..=end {
        if let Some(path) = found.get(&n) {
            files.push(path.clone());
            previous = Some(path);
        } else {
            if let Some(last) = missing_ranges
                .last_mut()
                .filter(|(_, b)| b.checked_add(1) == Some(n))
            {
                last.1 = n;
            } else {
                missing_ranges.push((n, n));
            }
            if o.missing == "hold" {
                files.push(
                    previous
                        .context("MISSING_FIRST_FRAME: hold has no previous frame in range")?
                        .clone(),
                );
            }
        }
    }
    ensure!(
        o.missing != "error" || missing_ranges.is_empty(),
        "MISSING_FRAMES: {}",
        serde_json::to_string(&missing_ranges)?
    );
    Ok(Discovery {
        files,
        missing_ranges,
    })
}

pub fn aggregate(frames: &[Value]) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"cutcraft.image-sequence.v1\0");
    hash.update((frames.len() as u64).to_be_bytes());
    for frame in frames {
        let h = text(&frame["fingerprint"]["value"]);
        ensure!(
            frame["fingerprint"]["algorithm"] == "sha256"
                && h.len() == 64
                && h.bytes().all(|b| b.is_ascii_hexdigit()),
            "FRAME_FINGERPRINT_INVALID"
        );
        for pair in h.as_bytes().chunks_exact(2) {
            hash.update([u8::from_str_radix(std::str::from_utf8(pair)?, 16)?]);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn relative_frame_path(uri: &str) -> Result<PathBuf> {
    ensure!(
        !uri.is_empty()
            && !uri.starts_with('/')
            && !uri.contains('\\')
            && !uri.contains('?')
            && !uri.contains('#')
            && url::Url::parse(uri).is_err(),
        "FRAME_URI_INVALID: {uri}"
    );
    // A synthetic drive makes this URI convertible on Windows as well as Unix.
    // Derive the base path through the same URL conversion as the candidate.
    let base = url::Url::parse("file:///C:/cutcraft-sequence-root/")?;
    let base_path = base
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("FRAME_URI_INVALID"))?;
    let full = base
        .join(uri)?
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("FRAME_URI_INVALID"))?;
    let rel = full
        .strip_prefix(&base_path)
        .context("FRAME_URI_ESCAPES_ROOT")?;
    ensure!(
        !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_))),
        "FRAME_URI_INVALID"
    );
    Ok(rel.into())
}
pub fn frame_path(dir: &Path, a: &Value, frame: &Value) -> Result<PathBuf> {
    Ok(media::local_path(dir, text(&a["uri"]))?.join(relative_frame_path(text(&frame["uri"]))?))
}
pub fn validate_asset(a: &Value) -> Result<()> {
    ensure!(
        a["kind"] == "media"
            && a["source"]["version"] == "1.0.0"
            && matches!(text(&a["storage"]), "managed" | "external"),
        "IMAGE_SEQUENCE_INVALID"
    );
    ensure!(
        text(&a["uri"]).ends_with('/'),
        "IMAGE_SEQUENCE_ROOT_MUST_BE_DIRECTORY_URI"
    );
    let root = url::Url::parse("file:///project/")?.join(text(&a["uri"]))?;
    ensure!(
        root.scheme() == "file" && root.query().is_none() && root.fragment().is_none(),
        "IMAGE_SEQUENCE_ROOT_MUST_BE_LOCAL"
    );
    let frames = array(&a["source"]["frames"]);
    ensure!(
        !frames.is_empty() && frames.len() <= MAX_FRAMES,
        "INVALID_FRAME_COUNT"
    );
    let mut paths = BTreeMap::new();
    for frame in frames {
        let p = relative_frame_path(text(&frame["uri"]))?;
        let fp = text(&frame["fingerprint"]["value"]).to_lowercase();
        if let Some(old) = paths.insert(p, fp.clone()) {
            ensure!(old == fp, "CONFLICTING_FRAME_FINGERPRINTS");
        }
    }
    ensure!(
        a["fingerprint"]["algorithm"] == "sha256"
            && aggregate(frames)?.eq_ignore_ascii_case(text(&a["fingerprint"]["value"])),
        "SEQUENCE_FINGERPRINT_MISMATCH"
    );
    let streams = array(&a["streams"]);
    ensure!(
        streams.len() == 1 && streams[0]["kind"] == "video" && streams[0]["index"] == 0,
        "IMAGE_SEQUENCE_REQUIRES_ONE_VIDEO_STREAM"
    );
    let fps = Time::from_json(&streams[0]["video"]["frameRate"])?;
    ensure!(fps > Time::zero(), "INVALID_FRAME_RATE");
    ensure!(
        Time::from_json(&streams[0]["duration"])?
            == Time::integer(frames.len() as i64).div(&fps)?,
        "IMAGE_SEQUENCE_DURATION_MISMATCH"
    );
    Ok(())
}

// FFmpeg treats some multi-page containers as a single image. Inspect their
// structural page/animation markers too, rather than silently taking page one.
fn reject_animation(path: &Path) -> Result<()> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = [0u8; 16];
    let read = f.read(&mut head)?;
    ensure!(read >= 8, "INVALID_IMAGE_HEADER");
    if &head[..8] == b"\x89PNG\r\n\x1a\n" {
        let mut pos = 8u64;
        while pos + 12 <= len {
            f.seek(SeekFrom::Start(pos))?;
            let mut h = [0u8; 8];
            f.read_exact(&mut h)?;
            ensure!(&h[4..] != b"acTL", "MULTIFRAME_IMAGE_UNSUPPORTED");
            pos = pos
                .checked_add(u32::from_be_bytes(h[..4].try_into()?) as u64 + 12)
                .context("image size overflow")?;
            if &h[4..] == b"IEND" {
                break;
            }
        }
    } else if read >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        let mut pos = 12u64;
        while pos + 8 <= len {
            f.seek(SeekFrom::Start(pos))?;
            let mut h = [0u8; 8];
            f.read_exact(&mut h)?;
            ensure!(
                &h[..4] != b"ANIM" && &h[..4] != b"ANMF",
                "MULTIFRAME_IMAGE_UNSUPPORTED"
            );
            let size = u32::from_le_bytes(h[4..].try_into()?) as u64;
            pos = pos
                .checked_add(8 + size + (size % 2))
                .context("image size overflow")?;
        }
    } else if matches!(&head[..2], b"II" | b"MM") {
        let le = &head[..2] == b"II";
        let num = |bytes: &[u8]| -> u64 {
            if le {
                bytes.iter().rev().fold(0, |a, b| (a << 8) | *b as u64)
            } else {
                bytes.iter().fold(0, |a, b| (a << 8) | *b as u64)
            }
        };
        let magic = num(&head[2..4]);
        let (offset, size, entry) = match magic {
            42 => (num(&head[4..8]), 2, 12),
            43 if read == 16 && num(&head[4..6]) == 8 => (num(&head[8..16]), 8, 20),
            _ => bail!("INVALID_TIFF_HEADER"),
        };
        ensure!(
            offset > 0 && offset.checked_add(size).is_some_and(|end| end <= len),
            "INVALID_TIFF_IFD"
        );
        f.seek(SeekFrom::Start(offset))?;
        let mut b = [0u8; 8];
        f.read_exact(&mut b[..size as usize])?;
        let entries = num(&b[..size as usize]);
        let next = offset
            .checked_add(size)
            .and_then(|x| {
                num(&b[..size as usize])
                    .checked_mul(entry)
                    .and_then(|n| x.checked_add(n))
            })
            .context("INVALID_TIFF_IFD")?;
        let width = if magic == 42 { 4 } else { 8 };
        ensure!(
            next.checked_add(width as u64).is_some_and(|end| end <= len),
            "INVALID_TIFF_IFD"
        );
        f.seek(SeekFrom::Start(next))?;
        f.read_exact(&mut b[..width])?;
        ensure!(num(&b[..width]) == 0, "MULTIFRAME_IMAGE_UNSUPPORTED");
        for i in 0..entries {
            let pos = offset + size + i * entry;
            f.seek(SeekFrom::Start(pos))?;
            let mut tag = [0u8; 20];
            f.read_exact(&mut tag[..entry as usize])?;
            let code = num(&tag[..2]);
            let count = num(&tag[4..4 + width]);
            ensure!(
                code != 330 || count == 0,
                "MULTIFRAME_IMAGE_UNSUPPORTED: TIFF SubIFDs"
            );
            if code == 338 {
                ensure!(
                    num(&tag[2..4]) == 3 && count <= 16,
                    "TIFF_ALPHA_UNSUPPORTED"
                );
                let bytes = count * 2;
                let value = 4 + width;
                let mut extra = [0u8; 32];
                if bytes <= width as u64 {
                    extra[..bytes as usize].copy_from_slice(&tag[value..value + bytes as usize]);
                } else {
                    let start = num(&tag[value..value + width]);
                    ensure!(
                        start.checked_add(bytes).is_some_and(|end| end <= len),
                        "INVALID_TIFF_ALPHA"
                    );
                    f.seek(SeekFrom::Start(start))?;
                    f.read_exact(&mut extra[..bytes as usize])?;
                }
                for v in extra[..bytes as usize].chunks_exact(2) {
                    ensure!(num(v) != 1, "PREMULTIPLIED_ALPHA_UNSUPPORTED");
                }
            }
        }
    }
    Ok(())
}
fn image_info(path: &Path, declared: Option<&str>) -> Result<Value> {
    ensure!(
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff"
        ),
        "IMAGE_FORMAT_UNSUPPORTED"
    );
    reject_animation(path)?;
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-read_intervals",
            "%+#2",
            "-of",
            "json",
            "-f",
            "image2",
            "-pattern_type",
            "none",
            "-i",
        ])
        .arg(path)
        .media_output()?;
    ensure!(
        out.status.success() && out.stderr.is_empty(),
        "IMAGE_PROBE_FAILED: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let native: Value = serde_json::from_slice(&out.stdout)?;
    let streams = array(&native["streams"]);
    ensure!(
        streams.len() == 1
            && streams[0]["codec_type"] == "video"
            && streams[0]["nb_read_frames"] == "1",
        "MULTIFRAME_OR_INVALID_IMAGE"
    );
    let s = &streams[0];
    ensure!(
        matches!(text(&s["color_primaries"]), "" | "unknown" | "bt709"),
        "COLOR_SPACE_UNSUPPORTED: image primaries"
    );
    ensure!(
        s["width"].as_u64().is_some_and(|n| n > 0) && s["height"].as_u64().is_some_and(|n| n > 0),
        "INVALID_IMAGE_DIMENSIONS"
    );
    let detected = match (text(&s["color_primaries"]), text(&s["color_transfer"])) {
        ("bt709", "bt709") => Some("rec709"),
        ("bt709" | "unknown" | "", "iec61966-2-1") => Some("srgb"),
        ("bt709", "linear") => Some("linear-srgb"),
        (_, "unknown" | "") => None,
        _ => bail!("COLOR_SPACE_UNSUPPORTED: image transfer/primaries"),
    };
    if let (Some(a), Some(b)) = (declared, detected) {
        ensure!(a == b, "COLOR_SPACE_CONFLICT");
    }
    let space = detected
        .or(declared)
        .context("COLOR_SPACE_REQUIRED: import-sequence with --color-space")?;
    crate::visual::Space::parse(space)?;
    let sar = text(&s["sample_aspect_ratio"])
        .replace(':', "/")
        .parse::<Time>()
        .ok()
        .filter(|n| n > &Time::zero())
        .unwrap_or(Time::integer(1));
    // Normalize all sequence output to straight RGBA; opaque files become alpha=1.
    Ok(
        json!({"width":s["width"],"height":s["height"],"pixelAspect":sar.json()?,"colorSpace":space,"alphaMode":"straight"}),
    )
}
pub fn validate_media(dir: &Path, a: &Value) -> Result<()> {
    validate_asset(a)?;
    let mut seen = BTreeSet::new();
    let v = &a["streams"][0]["video"];
    ensure!(
        v["alphaMode"] == "straight",
        "IMAGE_SEQUENCE_ALPHA_UNSUPPORTED: use straight RGBA"
    );
    for frame in array(&a["source"]["frames"]) {
        let path = frame_path(dir, a, frame)?;
        if !seen.insert(path.clone()) {
            continue;
        }
        ensure!(path.is_file(), "MEDIA_MISSING: {}", path.display());
        ensure!(
            media::fingerprint(&path)?.eq_ignore_ascii_case(text(&frame["fingerprint"]["value"])),
            "FINGERPRINT_MISMATCH: {}",
            path.display()
        );
        let info = image_info(&path, Some(text(&v["colorSpace"])))?;
        for k in ["width", "height", "pixelAspect", "colorSpace", "alphaMode"] {
            ensure!(
                v[k] == info[k],
                "IMAGE_SEQUENCE_FORMAT_MISMATCH: {k}: {}",
                path.display()
            );
        }
    }
    Ok(())
}

pub struct PreparedImport {
    aid: String,
    asset: Value,
    duration: Time,
    fps: Time,
    append: bool,
    frame_count: usize,
    file_count: usize,
    missing: String,
    missing_ranges: Vec<(i64, i64)>,
    staging: Option<tempfile::TempDir>,
}

fn check_support(p: &Value) -> Result<()> {
    ensure!(crate::capabilities::supports(text(&p["specVersion"]), crate::capabilities::Capability::ImageSequences),
        "PROJECT_UPGRADE_REQUIRED: image sequences require Spec 0.1.0 or internal draft 0.5.0; create a new 0.1.0 project (automatic migration is not supported)");
    Ok(())
}

pub fn import(store: &Store, o: &ImportOptions, override_locks: bool) -> Result<Value> {
    check_support(&store.project()?)?;
    commit_import(store, prepare_import(&store.dir, o)?, override_locks)
}

/// Resolve and copy the frozen manifest without holding an editing lock.
pub fn prepare_import(dir: &Path, o: &ImportOptions) -> Result<PreparedImport> {
    ensure!(o.fps > Time::zero(), "INVALID_FRAME_RATE");
    let root = o.directory.canonicalize()?;
    ensure!(root.is_dir(), "image-sequence source must be a directory");
    let selected = discover(&root, o)?;
    let base = url::Url::from_directory_path(&root)
        .map_err(|_| anyhow::anyhow!("invalid source directory"))?;
    let aid = id("asset");
    let staging = if o.managed {
        Some(tempfile::tempdir_in(dir.join("assets"))?)
    } else {
        None
    };
    let mut frames = vec![];
    let mut known: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut format = None;
    for rel in &selected.files {
        crate::media_process::check_cancellation()?;
        let path = root.join(rel);
        let target =
            url::Url::from_file_path(&path).map_err(|_| anyhow::anyhow!("invalid frame path"))?;
        let uri = base
            .make_relative(&target)
            .context("invalid relative frame path")?;
        let normalized = relative_frame_path(&uri)?;
        let fp = if let Some(fp) = known.get(&normalized) {
            fp.clone()
        } else {
            let before = media::fingerprint(&path)?;
            let info = image_info(&path, o.color_space.as_deref())?;
            if let Some(old) = &format {
                ensure!(
                    *old == info,
                    "IMAGE_SEQUENCE_FORMAT_MISMATCH: {}",
                    path.display()
                );
            } else {
                format = Some(info);
            }
            if let Some(stage) = &staging {
                let dest = stage.path().join(&normalized);
                fs::create_dir_all(dest.parent().unwrap())?;
                // Keep a writable handle for FlushFileBuffers on Windows.
                let mut copied = File::create(&dest)?;
                media::copy_cancellable(&mut File::open(&path)?, &mut copied)?;
                copied.sync_all()?;
                ensure!(
                    media::fingerprint(&dest)? == before,
                    "SOURCE_CHANGED_DURING_IMPORT"
                );
            }
            ensure!(
                media::fingerprint(&path)? == before,
                "SOURCE_CHANGED_DURING_IMPORT"
            );
            known.insert(normalized, before.clone());
            before
        };
        frames.push(json!({"uri":uri,"fingerprint":{"algorithm":"sha256","value":fp}}));
    }
    let duration = Time::integer(frames.len() as i64).div(&o.fps)?;
    let mut video = format.context("EMPTY_IMAGE_SEQUENCE")?;
    video["frameRate"] = o.fps.json()?;
    let project_base = url::Url::from_directory_path(dir).unwrap();
    let root_uri = if o.managed {
        format!("assets/{aid}/")
    } else {
        project_base
            .make_relative(&base)
            .unwrap_or_else(|| base.to_string())
    };
    // A sequence whose external root is the project directory still needs a directory URI.
    let root_uri = if root_uri.is_empty() {
        "./".into()
    } else {
        root_uri
    };
    let a = json!({"name":root.file_name().unwrap_or_default().to_string_lossy(),"kind":"media","uri":root_uri,"storage":if o.managed{"managed"}else{"external"},"availability":"online","fingerprint":{"algorithm":"sha256","value":aggregate(&frames)?},"source":{"type":"image-sequence","version":"1.0.0","frames":frames},"streams":[{"id":"stream_v0","kind":"video","index":0,"duration":duration.json()?,"video":video}]});
    validate_asset(&a)?;
    if let Some(stage) = &staging {
        sync_tree(stage.path())?;
    }
    Ok(PreparedImport {
        aid,
        asset: a,
        duration,
        fps: o.fps.clone(),
        append: o.append,
        frame_count: selected.files.len(),
        file_count: known.len(),
        missing: o.missing.clone(),
        missing_ranges: selected.missing_ranges,
        staging,
    })
}

pub fn commit_import(
    store: &Store,
    prepared: PreparedImport,
    override_locks: bool,
) -> Result<Value> {
    let p = store.project()?;
    check_support(&p)?;
    let PreparedImport {
        aid,
        asset: a,
        duration,
        fps,
        append,
        frame_count,
        file_count,
        missing,
        missing_ranges,
        staging,
    } = prepared;
    let dest = store.dir.join("assets").join(&aid);
    let mut commands = vec![json!({"type":"asset.put","assetId":aid,"asset":a})];
    let mut appended = Value::Null;
    if append {
        let sid = text(&p["rootSequenceId"]);
        let tid = id("track");
        let iid = id("item");
        let start = project::sequence_duration(&p["sequences"][sid])?;
        let item = json!({"id":iid,"type":"media","assetId":aid,"streamId":"stream_v0","timelineRange":range(&start,&duration)?,"sourceRange":range(&Time::zero(),&duration)?});
        commands.push(json!({"type":"track.put","sequenceId":sid,"track":{"id":tid,"kind":"video","name":a["name"],"overlapPolicy":"forbid","items":[item]}}));
        appended = json!({"sequenceId":sid,"trackId":tid,"itemId":iid});
    }
    let operation = crate::edit::operation(&p, json!({"type":"batch","commands":commands}));
    crate::edit::apply(&p, &operation, override_locks)?;
    if let Some(stage) = &staging {
        ensure!(!dest.exists(), "managed destination already exists");
        fs::rename(stage.path(), &dest)?;
        crate::store::sync_dir(&store.dir.join("assets"))?;
    }
    let result = store.apply(&operation, override_locks, false);
    if result.is_err()
        && staging.is_some()
        && !store.dir.join(".cutcraft/transaction.json").exists()
    {
        let _ = fs::remove_dir_all(&dest);
    }
    let committed = result?;
    Ok(
        json!({"ok":true,"assetId":aid,"streamId":"stream_v0","sourceType":"image-sequence","revision":committed["project"]["revision"],"frameRate":fps.json()?,"frameCount":frame_count,"fileCount":file_count,"duration":duration.json()?,"missingPolicy":missing,"missingRanges":missing_ranges,"append":appended}),
    )
}
fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() {
            sync_tree(&path)?;
        }
    }
    crate::store::sync_dir(path)?;
    Ok(())
}

pub fn selection(
    a: &Value,
    item: &Value,
    t: &Time,
    left: bool,
    handle: bool,
) -> Result<Vec<(usize, Time)>> {
    let n = array(&a["source"]["frames"]).len() as i64;
    let fps = Time::from_json(&a["streams"][0]["video"]["frameRate"])?;
    ensure!(n > 0 && fps > Time::zero(), "INVALID_IMAGE_SEQUENCE_CLOCK");
    let duration = Time::integer(n).div(&fps)?;
    let (source, upper, _) = animation::mapping(item, t, left)?;
    ensure!(
        source.nonnegative() && (source < duration || (upper && source == duration)),
        "SOURCE_OUT_OF_BOUNDS"
    );
    let floor = |x: &Time| {
        x.0.floor()
            .to_integer()
            .to_i64()
            .context("frame index overflow")
    };
    let (ss, se) = bounds(&item["sourceRange"])?;
    let lo = if handle {
        0
    } else {
        floor(&ss.mul(&fps))?.clamp(0, n - 1)
    };
    let hi = if handle {
        n - 1
    } else {
        (se.mul(&fps).ceil_i64()? - 1).clamp(0, n - 1)
    };
    ensure!(lo <= hi, "SOURCE_OUT_OF_BOUNDS");
    let x = source.mul(&fps);
    match text(&item["playback"]["videoSampling"]["method"]) {
        "" | "frame-hold" => Ok(vec![(
            (if upper { x.ceil_i64()? - 1 } else { floor(&x)? }).clamp(lo, hi) as usize,
            Time::integer(1),
        )]),
        "frame-blend" => {
            let i = floor(&x)?.clamp(lo, hi);
            let j = (i + 1).min(hi);
            if i == j {
                return Ok(vec![(i as usize, Time::integer(1))]);
            }
            let w = x
                .sub(&Time::integer(i))
                .max(Time::zero())
                .min(Time::integer(1));
            if w == Time::zero() {
                return Ok(vec![(i as usize, Time::integer(1))]);
            }
            Ok(vec![
                (i as usize, Time::integer(1).sub(&w)),
                (j as usize, w),
            ])
        }
        _ => bail!("PROCESSOR_UNAVAILABLE: optical-flow"),
    }
}
pub fn sample(a: &Value, item: &Value, t: &Time, left: bool, handle: bool) -> Result<Value> {
    Ok(json!(selection(a, item, t, left, handle)?
        .into_iter()
        .map(|(i, w)| Ok(
            json!({"index":i,"uri":a["source"]["frames"][i]["uri"],"weight":w.json()?})
        ))
        .collect::<Result<Vec<Value>>>()?))
}
pub fn decode(path: &Path, w: usize, h: usize) -> Result<Vec<u8>> {
    let size = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(8))
        .context("frame size overflow")?;
    let filter = format!("scale={w}:{h}:flags=bilinear:out_range=full,format=rgba64le");
    let out = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-threads",
            "2",
            "-noautorotate",
            "-f",
            "image2",
            "-pattern_type",
            "none",
            "-i",
        ])
        .arg(path)
        .args([
            "-an",
            "-threads",
            "2",
            "-filter_threads",
            "2",
            "-vf",
            &filter,
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgba64le",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .media_output()?;
    ensure!(
        out.status.success() && out.stdout.len() == size,
        "IMAGE_DECODE_FAILED: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
}
