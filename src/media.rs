//! FFprobe adapter. Native timestamps never enter canonical project data.
use crate::media_process::MediaCommand;
use crate::{
    id,
    project::{array, text},
    time::Time,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

pub fn fingerprint(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        crate::media_process::check_cancellation()?;
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn copy_cancellable(
    input: &mut impl std::io::Read,
    output: &mut impl std::io::Write,
) -> Result<()> {
    let mut buffer = [0_u8; 65536];
    loop {
        crate::media_process::check_cancellation()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..count])?;
    }
}
pub fn probe(path: &Path) -> Result<Value> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .media_output()?;
    ensure!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn positive_time(value: &Value) -> Option<Time> {
    value
        .as_str()
        .and_then(|s| s.parse::<Time>().ok())
        .filter(|t| t > &Time::zero())
}
fn stream_duration(s: &Value, fallback: &Value) -> Result<Option<Time>> {
    if let (Some(ticks), Some(base)) = (s["duration_ts"].as_i64(), s["time_base"].as_str()) {
        if ticks > 0 {
            return Ok(Some(Time::integer(ticks).mul(&base.parse()?)));
        }
    }
    Ok(positive_time(&s["duration"]).or_else(|| positive_time(fallback)))
}
pub fn asset(path: &Path, uri: &str, managed: bool) -> Result<Value> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
    {
        crate::visual::Cube::read(path)?;
        return Ok(
            json!({"name":path.file_name().unwrap().to_string_lossy(),"kind":"lut","uri":uri,"storage":if managed {"managed"}else{"external"},"availability":"online","fingerprint":{"algorithm":"sha256","value":fingerprint(path)?},"streams":[]}),
        );
    }
    let native = probe(path)?;
    let image = matches!(
        path.extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff"
    );
    let mut streams = vec![];
    for s in array(&native["streams"]) {
        let (kind, prefix) = match text(&s["codec_type"]) {
            "video" if image || s["disposition"]["attached_pic"] == 1 => ("image", "i"),
            "video" => ("video", "v"),
            "audio" => ("audio", "a"),
            "subtitle" => ("subtitle", "s"),
            _ => ("data", "d"),
        };
        let index = s["index"]
            .as_u64()
            .context("ffprobe omitted stream index")?;
        let mut stream = json!({"id":format!("stream_{prefix}{index}"),"kind":kind,"index":index});
        if s["disposition"]["attached_pic"] == 1 {
            stream["metadata"] = json!({"role":"attached-picture"});
        }
        if let Some(codec) = s["codec_name"].as_str() {
            stream["codec"] = json!(codec);
        }
        if kind != "image" {
            if let Some(duration) = stream_duration(s, &native["format"]["duration"])? {
                stream["duration"] = duration.json()?;
            }
        }
        if kind == "video" || kind == "image" {
            let aspect = text(&s["sample_aspect_ratio"])
                .replace(':', "/")
                .parse::<Time>()
                .ok()
                .filter(|t| t > &Time::zero())
                .unwrap_or(Time::integer(1));
            stream["video"] =
                json!({"width":s["width"],"height":s["height"],"pixelAspect":aspect.json()?});
            if s["color_primaries"] == "bt709" {
                let space = match text(&s["color_transfer"]) {
                    "bt709" => Some("rec709"),
                    "iec61966-2-1" => Some("srgb"),
                    "linear" => Some("linear-srgb"),
                    _ => None,
                };
                if let Some(space) = space {
                    stream["video"]["colorSpace"] = json!(space);
                }
            }
            if kind == "video" {
                let rate = positive_time(&s["avg_frame_rate"])
                    .or_else(|| positive_time(&s["r_frame_rate"]))
                    .context("video stream has no usable frame rate")?;
                stream["video"]["frameRate"] = rate.json()?;
            }
        }
        if kind == "audio" {
            stream["audio"] = json!({"sampleRate":text(&s["sample_rate"]).parse::<u64>()?,"channels":s["channels"]});
        }
        streams.push(stream);
    }
    ensure!(!streams.is_empty(), "media has no streams");
    Ok(
        json!({"name":path.file_name().context("file name missing")?.to_string_lossy(),"kind":if image {"image"}else{"media"},"uri":uri,"storage":if managed{"managed"}else{"external"},"availability":"online","fingerprint":{"algorithm":"sha256","value":fingerprint(path)?},"streams":streams}),
    )
}
pub fn local_path(dir: &Path, uri: &str) -> Result<PathBuf> {
    let base = url::Url::from_directory_path(dir.canonicalize()?)
        .map_err(|_| anyhow::anyhow!("invalid project directory"))?;
    let url = base.join(uri)?;
    ensure!(url.scheme() == "file", "REMOTE_MEDIA_UNSUPPORTED: {uri}");
    url.to_file_path()
        .map_err(|_| anyhow::anyhow!("invalid file URI"))
}
pub fn import(store: &crate::store::Store, path: &Path, managed: bool) -> Result<Value> {
    import_with_color(store, path, managed, None)
}
pub struct PreparedImport {
    pub asset_id: String,
    asset: Value,
    staged: Option<(tempfile::NamedTempFile, PathBuf)>,
}

pub fn import_with_color(
    store: &crate::store::Store,
    path: &Path,
    managed: bool,
    color_space: Option<&str>,
) -> Result<Value> {
    commit_import(
        store,
        prepare_import(&store.dir, path, managed, color_space)?,
    )
}

/// Probe, hash and copy without holding the project editing lock.
pub fn prepare_import(
    dir: &Path,
    path: &Path,
    managed: bool,
    color_space: Option<&str>,
) -> Result<PreparedImport> {
    let path = path.canonicalize()?;
    ensure!(path.is_file(), "asset must be a file");
    let aid = id("asset");
    let mut asset = asset(
        &path,
        url::Url::from_file_path(&path)
            .map_err(|_| anyhow::anyhow!("invalid asset path"))?
            .as_str(),
        false,
    )?;
    let mut staged = None;
    if let Some(space) = color_space {
        crate::visual::Space::parse(space)?;
        ensure!(
            asset["kind"] != "lut",
            "--color-space describes image/video sources, not LUT files"
        );
        for stream in asset["streams"].as_array_mut().unwrap() {
            if matches!(text(&stream["kind"]), "image" | "video") {
                stream["video"]["colorSpace"] = json!(space);
            }
        }
    }
    if managed {
        let ext = path.extension().and_then(|v| v.to_str()).unwrap_or("bin");
        let rel = format!("assets/{aid}.{ext}");
        let dest = dir.join(&rel);
        let mut input = File::open(&path)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir.join("assets"))?;
        copy_cancellable(&mut input, &mut tmp)?;
        tmp.as_file().sync_all()?;
        ensure!(
            fingerprint(tmp.path())? == text(&asset["fingerprint"]["value"]),
            "source changed during import"
        );
        // URL-encode each path segment, retaining relative project portability.
        let base = url::Url::from_directory_path(dir).unwrap();
        asset["uri"] = json!(base
            .make_relative(&url::Url::from_file_path(&dest).unwrap())
            .unwrap());
        asset["storage"] = json!("managed");
        staged = Some((tmp, dest));
    } else {
        let base = url::Url::from_directory_path(dir).unwrap();
        let target = url::Url::from_file_path(&path).unwrap();
        asset["uri"] = json!(base
            .make_relative(&target)
            .unwrap_or_else(|| target.to_string()));
    }
    Ok(PreparedImport {
        asset_id: aid,
        asset,
        staged,
    })
}

/// Publish prepared media and commit its asset through the durable Store.
pub fn commit_import(store: &crate::store::Store, prepared: PreparedImport) -> Result<Value> {
    let PreparedImport {
        asset_id: aid,
        asset,
        staged,
    } = prepared;
    let copied = if let Some((temp, dest)) = staged {
        temp.persist_noclobber(&dest).map_err(|e| e.error)?;
        crate::store::sync_dir(dest.parent().context("asset has no parent")?)?;
        Some(dest)
    } else {
        None
    };
    let result = store.command(
        json!({"type":"asset.put","assetId":aid,"asset":asset}),
        false,
        false,
    );
    if result.is_err() && !store.dir.join(".cutcraft/transaction.json").exists() {
        if let Some(path) = copied {
            let _ = std::fs::remove_file(path);
        }
    }
    let mut result = result?;
    result["assetId"] = json!(aid);
    Ok(result)
}

pub fn validate(project: &Value, dir: &Path) -> Result<Value> {
    crate::project::validate(project)?;
    if crate::frame_render::required(project) {
        crate::frame_render::preflight(project)?;
    }
    // No plugin runtime is installed in the basic backend. Preserve arbitrary
    // plugins at the domain layer, but never report them as media-ready.
    for seq in project["sequences"].as_object().unwrap().values() {
        for transition in array(&seq["transitions"]) {
            ensure!(
                matches!(
                    text(&transition["transitionType"]),
                    "builtin.crossfade" | "builtin.dip-to-color"
                ),
                "PLUGIN_UNAVAILABLE: {}",
                transition["transitionType"]
            );
        }
        for track in array(&seq["tracks"]) {
            for item in array(&track["items"]) {
                if item["type"] == "generator" {
                    crate::schema::validate_generator(item)?;
                    if let Some(aid) = item["parameters"]["fontAssetId"].as_str() {
                        ensure!(
                            project["assets"][aid]["kind"] == "font",
                            "FONT_ASSET_MISSING: {aid}"
                        );
                    }
                }
                if crate::visual::has_effects(item) {
                    let space = if item["type"] == "media" {
                        let stream = array(&project["assets"][text(&item["assetId"])]["streams"])
                            .iter()
                            .find(|s| s["id"] == item["streamId"])
                            .context("missing effect stream")?;
                        crate::visual::Space::parse(text(&stream["video"]["colorSpace"]))?
                    } else {
                        crate::visual::Space::Rec709
                    };
                    crate::visual::Pipeline::new(
                        &crate::animation::evaluated(item, &crate::time::Time::zero())?,
                        project,
                        dir,
                        space,
                    )?;
                }
            }
        }
    }
    let mut checked = vec![];
    for (aid, asset) in project["assets"].as_object().unwrap() {
        if crate::image_sequence::is_sequence(asset) {
            crate::image_sequence::validate_media(dir, asset)?;
            checked.push(json!({"assetId":aid,"sourceType":"image-sequence","frameCount":array(&asset["source"]["frames"]).len(),"status":"ok"}));
            continue;
        }
        let path = local_path(dir, text(&asset["uri"]))?;
        ensure!(path.is_file(), "MEDIA_MISSING: {}", path.display());
        if asset["kind"] == "lut" {
            crate::visual::Cube::read(&path)?;
        }
        if let Some(expected) = asset["fingerprint"]["value"].as_str() {
            ensure!(
                fingerprint(&path)?.eq_ignore_ascii_case(expected),
                "FINGERPRINT_MISMATCH: {aid}"
            );
        }
        if !array(&asset["streams"]).is_empty() {
            let actual = probe(&path)?;
            for stream in array(&asset["streams"]) {
                let s = array(&actual["streams"])
                    .iter()
                    .find(|s| s["index"] == stream["index"])
                    .context("MEDIA_STREAM_MISSING")?;
                let expected = if stream["kind"] == "image" {
                    "video"
                } else {
                    text(&stream["kind"])
                };
                ensure!(
                    text(&s["codec_type"]) == expected,
                    "MEDIA_STREAM_KIND_MISMATCH: {aid}"
                );
                if let Some(duration) = stream_duration(s, &actual["format"]["duration"])? {
                    for seq in project["sequences"].as_object().unwrap().values() {
                        for track in array(&seq["tracks"]) {
                            for item in array(&track["items"]) {
                                if text(&item["assetId"]) == aid
                                    && item["streamId"] == stream["id"]
                                    && !item["sourceRange"].is_null()
                                {
                                    ensure!(
                                        crate::time::bounds(&item["sourceRange"])?.1 <= duration,
                                        "MEDIA_SOURCE_OUT_OF_BOUNDS: {}",
                                        item["id"]
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        checked.push(json!({"assetId":aid,"path":path,"status":"ok"}));
    }
    Ok(json!({"ok":true,"level":"media","assets":checked}))
}
