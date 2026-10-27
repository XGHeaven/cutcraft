//! Explicit, fail-closed FFmpeg backend profile. The domain model is broader than
//! this first backend; unsupported semantics are errors, never silently dropped.
use crate::media_process::MediaCommand;
use crate::{
    media,
    project::{self, array, enabled, text},
    time::{bounds, Time},
    visual,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

pub fn capabilities() -> Value {
    let mut capabilities = json!({"backend":"cutcraft-reference-0.1","specVersions":["0.1.0","0.3.0","0.4.0","0.5.0"],"video":["timestamp-indexed media (CFR/VFR)","images","image sequences (printf/hash patterns or ordered manifest, rational fps, on-demand decoding)","normal compositing in linear light","exact fit/remap/reverse/hold/loop sampling","frame-hold","frame-blend","nested sequences at mapped time","adjustment layers","position/anchor/scale/rotation/crop/opacity animation","builtin.color-grade 1.0/2.0","builtin.curves 1.0","builtin.gaussian-blur 1.0 (animated sigma, linear premultiplied RGBA)","builtin.lut 1.0/2.0 (1D/3D cube)","builtin.crossfade 2.0","builtin.dip-to-color 2.0","text/captions"],"animation":["owner clock","linear","hold","cubic-bezier","exact rational time / cubic-hermite","curve-preserving trim/split"],"audio":["mix/mute/solo","sample-clock gainDb/pan","linear fades","remap with explicit mute or resample policy","freeze intervals are silent"],"processors":[{"id":"cutcraft.linear-resample","version":"1.0.0","parameters":{},"note":"linear PCM interpolation, pitch follows speed; no adaptive antialias filter"}],"unsupported":["optical-flow processor","preserve-pitch processor","custom plugins","non-normal blend modes","non-square output pixels","remote assets","loop-phase split","Log/HDR"],"legacyBackend":"0.3 static projects retain ffmpeg-basic profile","preview":"full MP4; --at PNG; --start/--duration interval MP4; optional --scale; default max edge 640, max 24 fps","imageSequences":{"specVersion":"0.1.0","specVersions":["0.1.0","0.5.0"],"maxFrames":1000000,"patterns":["printf","hash"],"missing":["error","hold","skip"],"decodeCacheMiB":64,"legacyImport":"PROJECT_UPGRADE_REQUIRED"},"performance":"offline CPU reference renderer; video uses temporary raw disk, image sequences decode on demand with a bounded per-render cache"});
    capabilities["gaussianBlur"] = json!({"specVersions":crate::capabilities::versions(crate::capabilities::Capability::GaussianBlur),"effectType":"builtin.gaussian-blur","version":"1.0.0","sigma":{"unit":"original input pixels","min":0,"max":100,"default":0,"animated":true}});
    capabilities["interactivePreview"] = json!({"specVersions":crate::capabilities::versions(crate::capabilities::Capability::InteractivePreview),"frame":"PNG, exact rational sequence time, sRGB straight alpha","playback":"continuous H.264 baseline / AAC fMP4 HLS for MSE","maxEdge":640,"maxFrameRate":{"num":24,"den":1},"maxAudioSampleRate":24000,"sourceWindows":"timestamp-preserving bounded video and phase-aligned PCM windows","decodeWindowMiB":256,"decodeCacheMiB":512,"frameCacheMiB":64,"frameCacheEntries":256,"playlistWindowSegments":24,"expiredPlayback":"restart-at-current-time","thumbnails":true,"waveforms":true,"cancellation":true,"performance":"CPU reference compositor; initial preparation and complex sequences may buffer"});
    capabilities
}
fn color(v: &Value, default: &str) -> Result<String> {
    if v.is_null() {
        return Ok(default.into());
    }
    let mut components = vec![];
    for key in ["r", "g", "b", "a"] {
        let f = v[key].as_f64().context("color must have numeric r/g/b/a")?;
        ensure!((0.0..=1.0).contains(&f), "color outside [0,1]");
        components.push((f * 255.0).round() as u8);
    }
    Ok(format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        components[0], components[1], components[2], components[3]
    ))
}
fn integer_units(time: &Time, rate: &Time, label: &str) -> Result<i64> {
    let samples = time.mul(rate);
    ensure!(
        samples.0.is_integer(),
        "BACKEND_UNSUPPORTED: {label} is not aligned to the sampling grid"
    );
    samples.ceil_i64()
}
fn signed_time(input: &str) -> Result<Time> {
    if let Some(v) = input.strip_prefix('-') {
        Ok(Time::zero().sub(&v.parse()?))
    } else {
        input.parse()
    }
}
fn native_origin(native: &Value) -> Result<Time> {
    let mut starts = vec![];
    for s in array(&native["streams"]) {
        if let (Some(pts), Some(base)) = (s["start_pts"].as_i64(), s["time_base"].as_str()) {
            starts.push(Time::integer(pts).mul(&base.parse()?));
        } else if let Some(start) = s["start_time"].as_str() {
            starts.push(signed_time(start)?);
        }
    }
    if let Some(origin) = starts.into_iter().min() {
        return Ok(origin);
    }
    native["format"]["start_time"]
        .as_str()
        .map(signed_time)
        .unwrap_or(Ok(Time::zero()))
}
/// Inspect presentation timestamps without decoding 4K pixels twice. Packet PTS
/// are sorted into presentation order (B-frame packet order is not display order).
/// Missing/duplicate/nonuniform PTS fail closed; decoding still happens at render.
fn verify_cfr(path: &Path, stream: u64, base: &Time, fps: &Time) -> Result<()> {
    let result = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            &stream.to_string(),
            "-show_packets",
            "-show_entries",
            "packet=pts",
            "-of",
            "json",
        ])
        .arg(path)
        .media_output()?;
    ensure!(
        result.status.success(),
        "FFPROBE_FAILED: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let index: Value = serde_json::from_slice(&result.stdout)?;
    let mut timestamps = array(&index["packets"])
        .iter()
        .map(|packet| {
            packet["pts"]
                .as_i64()
                .context("BACKEND_UNSUPPORTED: video packet has no presentation timestamp")
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !timestamps.is_empty(),
        "video has no presentation timestamps"
    );
    timestamps.sort_unstable();
    let step = Time::integer(1).div(fps)?;
    for pair in timestamps.windows(2) {
        let delta = Time::integer(pair[1])
            .sub(&Time::integer(pair[0]))
            .mul(base);
        ensure!(
            delta == step,
            "BACKEND_UNSUPPORTED: non-CFR presentation timestamp index"
        );
    }
    Ok(())
}
fn reject_item(item: &Value) -> Result<()> {
    if item["type"] == "generator" {
        crate::schema::validate_generator(item)?;
    }
    ensure!(
        matches!(text(&item["playback"]["mode"]), "" | "fit"),
        "BACKEND_UNSUPPORTED: loop/hold"
    );
    ensure!(
        matches!(text(&item["video"]["blendMode"]), "" | "normal"),
        "BACKEND_UNSUPPORTED: blend mode"
    );
    ensure!(
        item["audio"]["pan"].as_f64().unwrap_or(0.0) == 0.0,
        "BACKEND_UNSUPPORTED: pan"
    );
    for key in ["fadeIn", "fadeOut"] {
        if let Some(t) = item["audio"].get(key) {
            ensure!(
                Time::from_json(t)? == Time::zero(),
                "BACKEND_UNSUPPORTED: fades"
            );
        }
    }
    if let Some(version) = item["version"].as_str() {
        ensure!(
            version == "1.0.0",
            "BACKEND_UNSUPPORTED: generator version {version}"
        );
    }
    Ok(())
}
pub fn render(
    project: &Value,
    dir: &Path,
    sid: &str,
    output: &Path,
    overwrite: bool,
) -> Result<Value> {
    render_with_progress(
        project,
        dir,
        sid,
        output,
        overwrite,
        &crate::progress::Progress::default(),
    )
}
pub fn render_with_progress(
    project: &Value,
    dir: &Path,
    sid: &str,
    output: &Path,
    overwrite: bool,
    progress: &crate::progress::Progress,
) -> Result<Value> {
    let result = if crate::frame_render::required(project) {
        crate::frame_render::render_with_progress(project, dir, sid, output, overwrite, progress)
    } else {
        render_internal(project, dir, sid, output, overwrite, false, progress)
    }?;
    progress.finish();
    Ok(result)
}
fn render_internal(
    project: &Value,
    dir: &Path,
    sid: &str,
    output: &Path,
    overwrite: bool,
    lossless: bool,
    progress: &crate::progress::Progress,
) -> Result<Value> {
    progress.start("validate", None, "assets");
    media::validate(project, dir)?;
    let seq = project["sequences"]
        .get(sid)
        .context("sequence not found")?;
    ensure!(
        array(&seq["transitions"]).is_empty(),
        "BACKEND_UNSUPPORTED: transitions"
    );
    let format = &seq["format"];
    let w = format["width"].as_u64().unwrap();
    let h = format["height"].as_u64().unwrap();
    ensure!(
        w % 2 == 0 && h % 2 == 0,
        "H.264 output width/height must be even"
    );
    ensure!(
        Time::from_json(&format["pixelAspect"])? == Time::integer(1),
        "BACKEND_UNSUPPORTED: non-square output pixels"
    );
    ensure!(
        matches!(text(&format["audioChannelLayout"]), "mono" | "stereo"),
        "BACKEND_UNSUPPORTED: channel layout"
    );
    ensure!(
        matches!(text(&format["colorSpace"]), "" | "rec709"),
        "BACKEND_UNSUPPORTED: output color space"
    );
    let fps = Time::from_json(&format["frameRate"])?;
    let frame_time = Time::integer(1).div(&fps)?.expression();
    let sr = format["audioSampleRate"].as_i64().unwrap();
    let sample_rate = Time::integer(sr);
    let duration = project::sequence_duration(seq)?;
    ensure!(
        duration > Time::zero(),
        "EMPTY_SEQUENCE: add an item or set fixed duration"
    );
    let frames = duration.mul(&fps).ceil_i64()?;
    let samples = duration.mul(&sample_rate).ceil_i64()?;
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };
    ensure!(
        overwrite || !output.exists(),
        "output exists; use --overwrite"
    );
    ensure!(
        output.extension().and_then(|s| s.to_str()) == Some(if lossless { "nut" } else { "mp4" }),
        "basic renderer outputs .mp4"
    );
    for asset in project["assets"].as_object().unwrap().values() {
        let asset_path = media::local_path(dir, text(&asset["uri"]))?;
        if output.exists() {
            ensure!(
                output.canonicalize()? != asset_path.canonicalize()?,
                "OUTPUT_IS_SOURCE: refusing to overwrite a project asset"
            );
        }
    }
    let parent = output.parent().unwrap();
    fs::create_dir_all(parent)?;
    let scratch = tempfile::tempdir_in(parent)?;
    let mut cmd = Command::new("ffmpeg");
    cmd.current_dir(scratch.path()).args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-copyts",
        "-filter_complex_threads",
        "2",
    ]);
    let mut filters=vec![format!("color=c={}:s={w}x{h}:r={},trim=end_frame={frames},setpts=PTS-STARTPTS,format=rgba[base]",color(&seq["background"],"black")?,fps.expression()),format!("anullsrc=r={sr}:cl={},atrim=end_sample={samples}[silence]",text(&format["audioChannelLayout"]))];
    let mut video = "base".to_string();
    let mut audios = vec!["[silence]".to_string()];
    let mut checked_streams = BTreeSet::new();
    let mut index = 0usize;
    let mut layer_index = 0usize;
    for kind in ["video", "audio", "subtitle"] {
        let solo = array(&seq["tracks"])
            .iter()
            .any(|t| text(&t["kind"]) == kind && enabled(t) && t["solo"] == true);
        for track in array(&seq["tracks"])
            .iter()
            .filter(|t| text(&t["kind"]) == kind)
        {
            if !enabled(track)
                || (solo && track["solo"] != true)
                || (kind == "audio" && track["muted"] == true)
            {
                continue;
            }
            for item in array(&track["items"]) {
                if !enabled(item) || (kind == "audio" && item["audio"]["muted"] == true) {
                    continue;
                }
                reject_item(item)?;
                let (start, end) = bounds(&item["timelineRange"])?;
                if start >= duration {
                    continue;
                }
                let end = end.min(duration.clone());
                let length = end.sub(&start);
                let label = format!("layer{layer_index}");
                layer_index += 1;
                if item["type"] == "adjustment" {
                    let count = integer_units(&length, &fps, "adjustment duration")?;
                    let offset = integer_units(&start, &fps, "adjustment start")?;
                    let mut graph = video_dependencies(&filters, &video);
                    graph.push(format!("[{video}]trim=start_frame={offset}:end_frame={},setpts=PTS-STARTPTS,format=rgba64le[{label}]",offset+count));
                    let geometry = visual::Geometry::new(
                        w as usize,
                        h as usize,
                        1.,
                        w as usize,
                        h as usize,
                        &item["video"]["transform"],
                    )?;
                    let pipeline =
                        visual::Pipeline::new(item, project, dir, visual::Space::Rec709)?;
                    let intermediate = scratch.path().join(format!("adjustment{layer_index}.nut"));
                    visual::prepare(
                        &cmd,
                        &graph.join(";\n"),
                        &label,
                        &geometry,
                        Some(&pipeline),
                        &fps.expression(),
                        count,
                        &intermediate,
                    )?;
                    cmd.args(["-threads", "2", "-i"]).arg(&intermediate);
                    filters.push(format!(
                        "[{index}:v:0]settb=expr={frame_time},setpts=N+{offset}[{label}]"
                    ));
                    index += 1;
                    video = overlay_layer(
                        &mut filters,
                        &video,
                        &label,
                        layer_index,
                        offset,
                        count,
                        item,
                    );
                    continue;
                }
                if kind == "subtitle" || item["generatorType"] == "builtin.text" {
                    ensure!(
                        visual::needs_stage(item)
                            || item["video"]["opacity"].as_f64().unwrap_or(1.0) == 1.0,
                        "BACKEND_UNSUPPORTED: text opacity; use text color alpha"
                    );
                    let style = if kind == "subtitle" {
                        &item["style"]
                    } else {
                        &item["parameters"]
                    };
                    for key in ["backgroundColor", "maxLines"] {
                        ensure!(style[key].is_null(), "BACKEND_UNSUPPORTED: caption {key}");
                    }
                    let content = if kind == "subtitle" {
                        text(&item["text"])
                    } else {
                        item["parameters"]["text"]
                            .as_str()
                            .context("builtin.text requires text")?
                    };
                    let file = format!("text{layer_index}.txt");
                    fs::write(scratch.path().join(&file), content)?;
                    let size = style["fontSizePixels"].as_f64().unwrap_or(48.0);
                    ensure!(size > 0.0 && size <= 4096.0, "invalid text font size");
                    let x = match text(&style["align"]) {
                        "left" => "20",
                        "right" => "w-tw-20",
                        _ => "(w-tw)/2",
                    };
                    let y = match text(&style["position"]) {
                        "top" => "20",
                        "center" => "(h-th)/2",
                        _ => "h-th-40",
                    };
                    let font = if let Some(aid) = style["fontAssetId"].as_str() {
                        let source = media::local_path(dir, text(&project["assets"][aid]["uri"]))?;
                        let filename = format!("font{layer_index}.ttf");
                        fs::copy(source, scratch.path().join(&filename))?;
                        format!(":fontfile={filename}")
                    } else {
                        ensure!(
                            style["fontFamily"].is_null(),
                            "BACKEND_UNSUPPORTED: use fontAssetId to pin a font file"
                        );
                        String::new()
                    };
                    let first = start.mul(&fps).ceil_i64()?;
                    let last = end.mul(&fps).ceil_i64()?;
                    if last <= first {
                        continue;
                    }
                    if visual::needs_stage(item) {
                        let count = last - first;
                        let graph=format!("color=c=black@0:s={w}x{h}:r={},format=rgba,trim=end_frame={count},setpts=PTS-STARTPTS,drawtext=textfile={file}:expansion=none{font}:fontsize={size}:fontcolor=white:x={x}:y={y},{},format=rgba64le[{label}]",fps.expression(),visual::text_paint(&style["color"])?);
                        let geometry = visual::Geometry::new(
                            w as usize,
                            h as usize,
                            1.,
                            w as usize,
                            h as usize,
                            &item["video"]["transform"],
                        )?;
                        let pipeline =
                            visual::Pipeline::new(item, project, dir, visual::Space::Rec709)?;
                        let intermediate = scratch.path().join(format!("text{layer_index}.nut"));
                        visual::prepare(
                            &cmd,
                            &graph,
                            &label,
                            &geometry,
                            Some(&pipeline),
                            &fps.expression(),
                            count,
                            &intermediate,
                        )?;
                        cmd.args(["-threads", "2", "-i"]).arg(&intermediate);
                        filters.push(format!(
                            "[{index}:v:0]settb=expr={frame_time},setpts=N+{first}[{label}]"
                        ));
                        index += 1;
                        video = overlay_layer(
                            &mut filters,
                            &video,
                            &label,
                            layer_index,
                            first,
                            count,
                            item,
                        );
                        continue;
                    }
                    let next = format!("textout{layer_index}");
                    filters.push(format!("[{video}]drawtext=textfile={file}:expansion=none{font}:fontsize={size}:fontcolor={}:x={x}:y={y}:enable='gte(n,{first})*lt(n,{last})'[{next}]",color(&style["color"],"white")?));
                    video = next;
                    continue;
                }
                let visual = kind == "video";
                let source_requires_conversion = item["type"] == "media"
                    && array(&project["assets"][text(&item["assetId"])]["streams"])
                        .iter()
                        .any(|s| {
                            s["id"] == item["streamId"]
                                && matches!(text(&s["video"]["colorSpace"]), "srgb" | "linear-srgb")
                        });
                let processed = visual && (visual::needs_stage(item) || source_requires_conversion);
                let mut source_size = (w as usize, h as usize, 1.0);
                let mut source_space = Some(visual::Space::Rec709);
                let count = if visual {
                    integer_units(&length, &fps, "video duration")?
                } else {
                    integer_units(&length, &sample_rate, "audio duration")?
                };
                let offset = if visual {
                    integer_units(&start, &fps, "video start")?
                } else {
                    integer_units(&start, &sample_rate, "audio start")?
                };
                if item["type"] == "generator" {
                    match text(&item["generatorType"]) {
                        "builtin.color"=>filters.push(format!("color=c={}:s={w}x{h}:r={},trim=end_frame={count},settb=expr={frame_time},setpts=N+{offset},format=rgba[{label}]",color(&item["parameters"]["color"],"black")?,fps.expression())),
                        "builtin.silence"=>continue,
                        other=>anyhow::bail!("BACKEND_UNSUPPORTED: generator {other}")
                    }
                } else {
                    let nested_asset;
                    let asset = if item["type"] == "sequence" {
                        let path = scratch
                            .path()
                            .join(format!("nested-{}.nut", text(&item["sequenceId"])));
                        if !path.exists() {
                            render_internal(
                                project,
                                dir,
                                text(&item["sequenceId"]),
                                &path,
                                false,
                                true,
                                progress,
                            )?;
                        }
                        nested_asset = media::asset(
                            &path,
                            url::Url::from_file_path(&path).unwrap().as_str(),
                            false,
                        )?;
                        &nested_asset
                    } else {
                        &project["assets"][text(&item["assetId"])]
                    };
                    let path = media::local_path(dir, text(&asset["uri"]))?;
                    let stream = array(&asset["streams"])
                        .iter()
                        .find(|s| {
                            if item["type"] == "sequence" {
                                s["kind"] == kind
                            } else {
                                s["id"] == item["streamId"]
                            }
                        })
                        .context("missing stream")?;
                    let stream_index = stream["index"].as_u64().unwrap();
                    let image = stream["kind"] == "image";
                    if visual {
                        source_size = (
                            stream["video"]["width"].as_u64().unwrap() as usize,
                            stream["video"]["height"].as_u64().unwrap() as usize,
                            stream["video"]["pixelAspect"]["num"].as_f64().unwrap()
                                / stream["video"]["pixelAspect"]["den"].as_f64().unwrap(),
                        );
                        source_space =
                            visual::Space::parse(text(&stream["video"]["colorSpace"])).ok();
                        if item["type"] == "sequence" {
                            source_space = Some(visual::Space::Rec709);
                        }
                        ensure!(stream["video"]["alphaMode"] != "premultiplied", "BACKEND_UNSUPPORTED: premultiplied source alpha; supply straight-alpha media");
                    }
                    if image {
                        cmd.args(["-loop", "1", "-framerate", &fps.expression()]);
                    }
                    if visual && !image {
                        // Seek to an integer second before the precise source boundary.
                        // -copyts preserves native PTS; the trim filter below still
                        // selects the exact rational boundary, including preroll.
                        let seek = Time::from_json(&item["sourceRange"]["start"])?
                            .0
                            .floor()
                            .to_integer();
                        cmd.args(["-ss", &seek.to_string()]);
                    }
                    cmd.args(["-threads", "2"]);
                    cmd.arg("-i").arg(&path);
                    let mut chain = format!("[{index}:{stream_index}]");
                    index += 1;
                    if image {
                        chain += &format!("trim=end_frame={count},setpts=PTS-STARTPTS");
                    } else {
                        let native = media::probe(&path)?;
                        let s = array(&native["streams"])
                            .iter()
                            .find(|s| s["index"] == stream["index"])
                            .unwrap();
                        let base: Time = text(&s["time_base"]).parse()?;
                        let origin = native_origin(&native)?;
                        let (ss, se) = bounds(&item["sourceRange"])?;
                        let source_start = ss.add(&origin);
                        let source_end = se.add(&origin);
                        if visual {
                            ensure!(
                                s["avg_frame_rate"] == s["r_frame_rate"],
                                "BACKEND_UNSUPPORTED: VFR input"
                            );
                            let source_fps: Time = text(&s["avg_frame_rate"]).parse()?;
                            if checked_streams.insert((path.clone(), stream_index)) {
                                verify_cfr(&path, stream_index, &base, &source_fps)?;
                            }
                            let native_start = s["start_pts"]
                                .as_i64()
                                .map(|n| Time::integer(n).mul(&base))
                                .unwrap_or(origin.clone());
                            ensure!(
                                source_start >= native_start,
                                "BACKEND_UNSUPPORTED: source starts before video stream"
                            );
                            integer_units(
                                &source_start.sub(&native_start),
                                &source_fps,
                                "source start",
                            )?;
                            integer_units(
                                &source_end.sub(&native_start),
                                &source_fps,
                                "source end",
                            )?;
                            let a = source_start.div(&base)?.ceil_i64()?;
                            let b = source_end.div(&base)?.ceil_i64()?;
                            chain += &format!("trim=start_pts={a}:end_pts={b},setpts=PTS-STARTPTS");
                            if item["playback"]["reverse"] == true {
                                chain += ",reverse,setpts=PTS-STARTPTS";
                            }
                            let speed = Time::from_json(&item["timelineRange"]["duration"])?
                                .div(&se.sub(&ss))?;
                            chain += &format!(
                                ",setpts=PTS*({}),fps=fps={}:round=near,trim=end_frame={count}",
                                speed.expression(),
                                fps.expression()
                            );
                        } else {
                            ensure!(
                                Time::from_json(&item["timelineRange"]["duration"])? == se.sub(&ss),
                                "BACKEND_UNSUPPORTED: audio time stretching"
                            );
                            let native_sr = Time::integer(text(&s["sample_rate"]).parse()?);
                            let native_start = s["start_pts"]
                                .as_i64()
                                .map(|n| Time::integer(n).mul(&base))
                                .unwrap_or(origin.clone());
                            ensure!(source_start>=native_start,"BACKEND_UNSUPPORTED: audio source starts before stream; trim to its shared-origin offset");
                            let a = integer_units(
                                &source_start.sub(&native_start),
                                &native_sr,
                                "audio source start",
                            )?;
                            let b = integer_units(
                                &source_end.sub(&native_start),
                                &native_sr,
                                "audio source end",
                            )?;
                            chain += &format!(
                                "atrim=start_sample={a}:end_sample={b},asetpts=PTS-STARTPTS"
                            );
                            if item["playback"]["reverse"] == true {
                                chain += ",areverse,asetpts=PTS-STARTPTS";
                            }
                        }
                    }
                    if visual {
                        if processed {
                            // YUV decode matrix follows the explicit source declaration;
                            // transfer conversion is performed in the CPU color stage.
                            if source_space.is_some() {
                                chain += ",scale=in_color_matrix=bt709:out_range=full";
                            }
                            chain+=&format!(",format=rgba64le,setsar=1,settb=expr={frame_time},setpts=N[{label}]");
                        } else {
                            chain+=&format!(",scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black@0,setsar=1,format=rgba,settb=expr={frame_time},setpts=N+{offset}[{label}]");
                        }
                    } else {
                        chain+=&format!(",aresample={sr},aformat=channel_layouts={},atrim=end_sample={count},volume={}dB,adelay={offset}S:all=1[{label}]",text(&format["audioChannelLayout"]),item["audio"]["gainDb"].as_f64().unwrap_or(0.0));
                    }
                    filters.push(chain);
                }
                if processed {
                    let graph = filters.pop().context("missing visual layer graph")?;
                    let geometry = visual::Geometry::new(
                        source_size.0,
                        source_size.1,
                        source_size.2,
                        w as usize,
                        h as usize,
                        &item["video"]["transform"],
                    )?;
                    let pipeline = if visual::has_effects(item) || source_space.is_some() {
                        Some(visual::Pipeline::new(
                            item,
                            project,
                            dir,
                            source_space.context(
                                "COLOR_SPACE_REQUIRED: import source with --color-space",
                            )?,
                        )?)
                    } else {
                        None
                    };
                    let intermediate = scratch.path().join(format!("visual{layer_index}.nut"));
                    visual::prepare(
                        &cmd,
                        &graph,
                        &label,
                        &geometry,
                        pipeline.as_ref(),
                        &fps.expression(),
                        count,
                        &intermediate,
                    )?;
                    cmd.args(["-threads", "2", "-i"]).arg(&intermediate);
                    filters.push(format!(
                        "[{index}:v:0]settb=expr={frame_time},setpts=N+{offset}[{label}]"
                    ));
                    index += 1;
                }
                if visual {
                    video = overlay_layer(
                        &mut filters,
                        &video,
                        &label,
                        layer_index,
                        offset,
                        count,
                        item,
                    );
                } else {
                    audios.push(format!("[{label}]"));
                }
            }
        }
    }
    filters.push(if lossless {
        format!("[{video}]format=gbrap16le,setparams=range=full:colorspace=gbr:color_primaries=bt709:color_trc=bt709[vout]")
    } else {
        format!("[{video}]scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709[vout]")
    });
    filters.push(format!(
        "{}amix=inputs={}:normalize=0:duration=first,atrim=end_sample={samples}[aout]",
        audios.join(""),
        audios.len()
    ));
    let graph = filters.join(";\n");
    fs::write(scratch.path().join("graph.txt"), &graph)?;
    cmd.args([
        crate::media_process::graph_file_option()?,
        "graph.txt",
        "-map",
        "[vout]",
        "-map",
        "[aout]",
        "-r",
        &fps.expression(),
        "-fps_mode",
        "cfr",
    ]);
    let filename = if lossless { "render.nut" } else { "render.mp4" };
    if lossless {
        cmd.args([
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "gbrap16le",
            "-c:a",
            "pcm_f32le",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            "pc",
        ]);
    } else {
        cmd.args([
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "18",
            "-c:a",
            "aac",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            "tv",
            "-movflags",
            "+faststart",
        ]);
    }
    cmd.arg(filename);
    progress.ffmpeg(&mut cmd, "encode", Some(frames as u64))?;
    progress.start("finalize", None, "files");
    progress.check_cancelled()?;
    let rendered = scratch.path().join(filename);
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut fs::File::open(rendered)?, &mut staged)?;
    staged.as_file().sync_all()?;
    if overwrite {
        staged.persist(&output).map_err(|e| e.error)?;
    } else {
        staged.persist_noclobber(&output).map_err(|e| e.error)?;
    }
    Ok(
        json!({"ok":true,"output":output,"sequenceId":sid,"revision":project["revision"],"frames":frames,"duration":duration.json()?,"backend":"ffmpeg-basic"}),
    )
}
pub fn preview(project: &Value, dir: &Path, sid: &str) -> Result<Value> {
    preview_with_progress(project, dir, sid, &crate::progress::Progress::default())
}
pub fn preview_with_progress(
    project: &Value,
    dir: &Path,
    sid: &str,
    progress: &crate::progress::Progress,
) -> Result<Value> {
    let hash = crate::store::hash(project)?;
    // Profile/version separates low-resolution artifacts from legacy full-size previews.
    let cache = dir.join("cache").join(format!(
        "preview-{}-640-24-v1-{}-{sid}",
        env!("CARGO_PKG_VERSION"),
        &hash[..12]
    ));
    fs::create_dir_all(&cache)?;
    let output = cache.join("preview.mp4");
    let mut result = crate::frame_render::preview(project, dir, sid, &output, progress)?;
    result["video"] = json!(output);
    progress.finish();
    Ok(result)
}

fn overlay_layer(
    filters: &mut Vec<String>,
    video: &str,
    label: &str,
    index: usize,
    offset: i64,
    count: i64,
    item: &Value,
) -> String {
    let opacity = item["video"]["opacity"].as_f64().unwrap_or(1.0);
    let alpha = format!("alpha{index}");
    filters.push(format!("[{label}]colorchannelmixer=aa={opacity}[{alpha}]"));
    let next = format!("composite{index}");
    filters.push(format!("[{video}][{alpha}]overlay=eof_action=pass:repeatlast=0:enable='gte(n,{offset})*lt(n,{})':format=auto[{next}]",offset+count));
    next
}
/// Select the connected video graph without unused audio outputs. Every graph
/// node generated by this backend has one named output as its trailing label.
fn video_dependencies(filters: &[String], output: &str) -> Vec<String> {
    let mut needed = BTreeSet::from([output.to_string()]);
    let mut selected = vec![];
    for filter in filters.iter().rev() {
        let Some((prefix, last)) = filter.rsplit_once('[') else {
            continue;
        };
        if !needed.remove(last.trim_end_matches(']')) {
            continue;
        }
        selected.push(filter.clone());
        for part in prefix.split('[').skip(1) {
            if let Some((label, _)) = part.split_once(']') {
                if !label.contains(':') {
                    needed.insert(label.to_string());
                }
            }
        }
    }
    selected.reverse();
    selected
}
