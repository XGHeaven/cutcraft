//! Interactive preview artifacts. Canonical edits and render/export remain separate.
use crate::{
    frame_render::PreviewEngine,
    progress::{CancellationToken, Progress},
    project::{self, array, text},
    time::Time,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

#[derive(Debug)]
pub struct PreviewFrame {
    pub bytes: Vec<u8>,
    pub metadata: Value,
}

/// Runtime resolution multiplier, relative to sequence/media dimensions.
/// Omission preserves the legacy 640px preview bound. Never changes the project.
pub fn validate_scale(scale: Option<f64>) -> Result<()> {
    ensure!(
        scale.is_none_or(|v| v.is_finite() && v > 0. && v <= 1.),
        "INVALID_PREVIEW_SCALE: expected a finite number in (0, 1]"
    );
    Ok(())
}
type Verified = (u64, Option<SystemTime>, String);
static VERIFIED: OnceLock<Mutex<HashMap<PathBuf, Verified>>> = OnceLock::new();
fn validate(project: &Value, dir: &Path, progress: &Progress) -> Result<()> {
    progress.check_cancelled()?;
    ensure!(
        crate::capabilities::supports(
            text(&project["specVersion"]),
            crate::capabilities::Capability::InteractivePreview
        ),
        "INTERACTIVE_PREVIEW_UNSUPPORTED_VERSION"
    );
    project::validate(project)?;
    for asset in project["assets"].as_object().context("assets")?.values() {
        if crate::image_sequence::is_sequence(asset) {
            continue;
        }
        let path = crate::media::local_path(dir, text(&asset["uri"]))?;
        let stat =
            fs::metadata(&path).with_context(|| format!("MEDIA_MISSING: {}", path.display()))?;
        ensure!(stat.is_file(), "MEDIA_MISSING: {}", path.display());
        let Some(expected) = asset["fingerprint"]["value"].as_str() else {
            continue;
        };
        let signature = (stat.len(), stat.modified().ok(), expected.to_owned());
        let cache = VERIFIED.get_or_init(|| Mutex::new(HashMap::new()));
        if cache.lock().unwrap().get(&path) == Some(&signature) {
            continue;
        }
        use sha2::{Digest, Sha256};
        let mut source = fs::File::open(&path)?;
        let mut hash = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            progress.check_cancelled()?;
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        ensure!(
            format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected),
            "FINGERPRINT_MISMATCH: {}",
            path.display()
        );
        let after = source.metadata()?;
        ensure!(
            after.len() == signature.0 && after.modified().ok() == signature.1,
            "MEDIA_CHANGED_DURING_READ"
        );
        let mut cached = cache.lock().unwrap();
        if cached.len() >= 256 {
            cached.clear();
        }
        cached.insert(path, signature);
    }
    Ok(())
}
fn frame_key(project: &Value, sid: &str, at: &Time, scale: Option<f64>) -> Result<String> {
    crate::store::hash(
        &json!({"project":project,"sequenceId":sid,"at":at.json()?,"scale":scale,"profile":"interactive-v3","renderer":env!("CARGO_PKG_VERSION")}),
    )
}
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::store::atomic_write(path, bytes)
}
pub(crate) fn prune_artifacts(cache: &Path, keep: &Path) -> Result<()> {
    let mut entries = fs::read_dir(cache)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (e.path(), m.len(), m.modified().ok()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|(_, _, time)| *time);
    let mut count = entries.len();
    let mut total: u64 = entries.iter().map(|(_, n, _)| n).sum();
    for (path, len, _) in entries {
        if count <= 256 && total <= 64 * 1024 * 1024 {
            break;
        }
        if path == keep {
            continue;
        }
        if fs::remove_file(path).is_ok() {
            count -= 1;
            total = total.saturating_sub(len);
        }
    }
    Ok(())
}
/// Return a final composed PNG at an exact timeline time. PNG pixels use sRGB.
pub fn frame(
    project: &Value,
    dir: &Path,
    sid: &str,
    at: &Time,
    progress: &Progress,
) -> Result<PreviewFrame> {
    frame_scaled(project, dir, sid, at, progress, None)
}
pub fn frame_scaled(
    project: &Value,
    dir: &Path,
    sid: &str,
    at: &Time,
    progress: &Progress,
    scale: Option<f64>,
) -> Result<PreviewFrame> {
    validate_scale(scale)?;
    ensure!(at.nonnegative(), "preview time must be non-negative");
    validate(project, dir, progress)?;
    let cache = dir.join("cache").join("interactive-v1");
    fs::create_dir_all(&cache)?;
    let output = cache.join(format!("{}.png", frame_key(project, sid, at, scale)?));
    let scratch = tempfile::tempdir_in(&cache)?;
    let mut engine = PreviewEngine::with_scale(project, dir, sid, scratch.path(), progress, scale)?;
    let metadata = json!({"revision":project["revision"],"sequenceId":sid,"time":at.json()?,"width":engine.width,"height":engine.height,"scale":scale,"mime":"image/png"});
    let reusable = project["assets"]
        .as_object()
        .unwrap()
        .values()
        .all(|asset| {
            !crate::image_sequence::is_sequence(asset) && asset["fingerprint"]["value"].is_string()
        });
    if reusable && output.is_file() {
        let bytes = fs::read(output)?;
        progress.finish();
        return Ok(PreviewFrame { bytes, metadata });
    }
    let raw = engine.video(at, true)?;
    let raw_path = scratch.path().join("frame.rgba");
    fs::write(&raw_path, raw)?;
    progress.start("encode_frame", Some(1), "frames");
    let result = progress.output(
        Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba64le",
                "-video_size",
                &format!("{}x{}", engine.width, engine.height),
                "-i",
            ])
            .arg(&raw_path)
            .args([
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
            ]),
    )?;
    ensure!(
        result.status.success(),
        "MEDIA_PROCESS_FAILED: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    progress.check_cancelled()?;
    publish(&output, &result.stdout)?;
    prune_artifacts(&cache, &output)?;
    progress.finish();
    Ok(PreviewFrame {
        bytes: result.stdout,
        metadata,
    })
}

#[derive(Default)]
pub struct ExportOptions {
    pub at: Option<Time>,
    pub start: Option<Time>,
    pub duration: Option<Time>,
    pub scale: Option<f64>,
}

/// CLI artifacts reuse the interactive evaluator and bounded source caches.
/// Sampling is anchored at the exact requested start, without editing the project.
pub fn export(
    project: &Value,
    dir: &Path,
    sid: &str,
    options: &ExportOptions,
    progress: &Progress,
) -> Result<Value> {
    validate_scale(options.scale)?;
    ensure!(
        options.at.is_none() || (options.start.is_none() && options.duration.is_none()),
        "PREVIEW_OPTIONS_CONFLICT: --at cannot be combined with --start/--duration"
    );
    let seq = project["sequences"]
        .get(sid)
        .context("sequence not found")?;
    let end = project::sequence_duration(seq)?;
    if let Some(at) = &options.at {
        ensure!(at.nonnegative() && at < &end, "PREVIEW_TIME_OUT_OF_BOUNDS");
        let mut result = frame_scaled(project, dir, sid, at, progress, options.scale)?.metadata;
        let output = dir.join("cache/interactive-v1").join(format!(
            "{}.png",
            frame_key(project, sid, at, options.scale)?
        ));
        result["ok"] = json!(true);
        result["output"] = json!(output);
        result["image"] = result["output"].clone();
        result["preview"] = json!(true);
        return Ok(result);
    }
    let start = options.start.clone().unwrap_or_else(Time::zero);
    let duration = options.duration.clone().unwrap_or_else(|| end.sub(&start));
    ensure!(
        start.nonnegative()
            && start < end
            && duration > Time::zero()
            && start.add(&duration) <= end,
        "PREVIEW_RANGE_OUT_OF_BOUNDS: require 0 <= start < start+duration <= sequence duration"
    );
    validate(project, dir, progress)?;
    let cache = dir.join("cache").join("preview-ranges-v1");
    fs::create_dir_all(&cache)?;
    let key = crate::store::hash(
        &json!({"project":project,"sequenceId":sid,"start":start.json()?,"duration":duration.json()?,"scale":options.scale,"profile":"preview-range-v2","renderer":env!("CARGO_PKG_VERSION")}),
    )?;
    let output = cache.join(format!("{key}.mp4"));
    let scratch = tempfile::tempdir_in(&cache)?;
    let mut engine =
        PreviewEngine::with_scale(project, dir, sid, scratch.path(), progress, options.scale)?;
    let frames = duration.mul(&engine.fps).ceil_i64()?;
    let samples = duration.mul(&Time::integer(engine.rate)).ceil_i64()?;
    let video = scratch.path().join("video.rgba");
    let audio = scratch.path().join("audio.f32");
    let mut writer = std::io::BufWriter::new(fs::File::create(&video)?);
    progress.start("video", Some(frames as u64), "frames");
    for n in 0..frames {
        writer.write_all(&engine.video(&start.add(&Time::integer(n).div(&engine.fps)?), false)?)?;
        progress.advance_phase("video", Some(frames as u64), "frames", (n + 1) as u64);
    }
    writer.flush()?;
    drop(writer);
    let mut writer = std::io::BufWriter::new(fs::File::create(&audio)?);
    progress.start("audio", Some(samples as u64), "samples");
    for n in (0..samples).step_by(4096) {
        let count = (samples - n).min(4096) as usize;
        writer.write_all(&engine.audio_at(&start.add(&Time::new(n, engine.rate)?), count)?)?;
        progress.advance_phase(
            "audio",
            Some(samples as u64),
            "samples",
            n as u64 + count as u64,
        );
    }
    writer.flush()?;
    drop(writer);
    let rendered = scratch.path().join("preview.mp4");
    progress.ffmpeg(Command::new("ffmpeg")
        .args(["-nostdin","-v","error","-y","-f","rawvideo","-pixel_format","rgba64le","-video_size",&format!("{}x{}",engine.width,engine.height),"-framerate",&engine.fps.expression(),"-i"]).arg(&video)
        .args(["-f","f32le","-ar",&engine.rate.to_string(),"-ac",&engine.channels.to_string(),"-i"]).arg(&audio)
        .args(["-filter_threads","2","-vf","scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709","-c:v","libx264","-threads","2","-preset","ultrafast","-crf","28","-c:a","aac","-movflags","+faststart"]).arg(&rendered), "encode", Some(frames as u64))?;
    progress.start("finalize", None, "files");
    progress.check_cancelled()?;
    let mut staged = tempfile::NamedTempFile::new_in(&cache)?;
    std::io::copy(&mut fs::File::open(&rendered)?, &mut staged)?;
    staged.as_file().sync_all()?;
    staged.persist(&output).map_err(|e| e.error)?;
    crate::store::sync_dir(&cache)?;
    progress.finish();
    Ok(
        json!({"ok":true,"output":output,"video":output,"preview":true,"revision":project["revision"],"sequenceId":sid,"startTime":start.json()?,"duration":duration.json()?,"frames":frames,"audioSamples":samples,"width":engine.width,"height":engine.height,"scale":options.scale,"frameRate":engine.fps.json()?,"audioSampleRate":engine.rate,"mime":"video/mp4"}),
    )
}

type StreamInput = (u16, SyncSender<Vec<u8>>, thread::JoinHandle<Result<()>>);
fn stream_input(cancel: CancellationToken, halt: Arc<AtomicBool>) -> Result<StreamInput> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let (send, receive) = mpsc::sync_channel::<Vec<u8>>(4);
    let handle = thread::spawn(move || -> Result<()> {
        let mut socket = loop {
            cancel.check()?;
            if halt.load(Ordering::Acquire) {
                return Ok(());
            }
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error.into()),
            }
        };
        socket.set_write_timeout(Some(Duration::from_millis(100)))?;
        loop {
            cancel.check()?;
            if halt.load(Ordering::Acquire) {
                return Ok(());
            }
            match receive.recv_timeout(Duration::from_millis(20)) {
                Ok(data) => {
                    let mut offset = 0;
                    while offset < data.len() {
                        cancel.check()?;
                        if halt.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        match socket.write(&data[offset..]) {
                            Ok(0) => anyhow::bail!("ENCODER_INPUT_CLOSED"),
                            Ok(n) => offset += n,
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    });
    Ok((port, send, handle))
}
fn send(
    send: &SyncSender<Vec<u8>>,
    mut bytes: Vec<u8>,
    cancel: &CancellationToken,
    halt: &AtomicBool,
) -> Result<()> {
    loop {
        cancel.check()?;
        ensure!(!halt.load(Ordering::Acquire), "ENCODER_STOPPED");
        match send.try_send(bytes) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(data)) => {
                bytes = data;
                thread::sleep(Duration::from_millis(10));
            }
            Err(TrySendError::Disconnected(_)) => anyhow::bail!("ENCODER_INPUT_CLOSED"),
        }
    }
}
/// A fixed-revision, single-encoder session. Completed HLS fragments can be read
/// while this call runs; cancellation closes both input streams and reaps FFmpeg.
pub fn playback(
    project: &Value,
    dir: &Path,
    sid: &str,
    start: &Time,
    out_dir: &Path,
    progress: &Progress,
) -> Result<Value> {
    playback_scaled(project, dir, sid, start, out_dir, progress, None)
}
pub fn playback_scaled(
    project: &Value,
    dir: &Path,
    sid: &str,
    start: &Time,
    out_dir: &Path,
    progress: &Progress,
    scale: Option<f64>,
) -> Result<Value> {
    validate_scale(scale)?;
    validate(project, dir, progress)?;
    ensure!(start.nonnegative(), "preview time must be non-negative");
    fs::create_dir_all(out_dir)?;
    ensure!(
        !out_dir.join("stream.m3u8").exists(),
        "PREVIEW_SESSION_EXISTS"
    );
    let scratch = tempfile::tempdir_in(out_dir)?;
    let mut engine = PreviewEngine::with_scale(project, dir, sid, scratch.path(), progress, scale)?;
    ensure!(start < &engine.duration, "PREVIEW_TIME_OUT_OF_BOUNDS");
    let first = start.mul(&engine.fps).ceil_i64()?;
    let origin = Time::integer(first).div(&engine.fps)?;
    ensure!(origin < engine.duration, "PREVIEW_TIME_OUT_OF_BOUNDS");
    let frames = engine.duration.mul(&engine.fps).ceil_i64()? - first;
    let sample_start = 0;
    let sample_end = engine
        .duration
        .sub(&origin)
        .mul(&Time::integer(engine.rate))
        .ceil_i64()?;
    // Both output clocks start at the exact seek origin, without rounding it.
    let audio_offset = Time::zero();
    let macroblocks = engine.width.div_ceil(16) * engine.height.div_ceil(16);
    let extent = engine.width.max(engine.height);
    let (level, codec) = if macroblocks <= 1620 && extent <= 720 {
        ("3.0", "avc1.42c01e")
    } else if macroblocks <= 3600 && extent <= 1280 {
        ("3.1", "avc1.42c01f")
    } else if macroblocks <= 8192 && extent <= 1920 {
        ("4.0", "avc1.42c028")
    } else {
        ("5.1", "avc1.42c033")
    };
    let mime = format!("video/mp4; codecs=\"{codec},mp4a.40.2\"");
    let metadata = json!({"revision":project["revision"],"sequenceId":sid,"startTime":origin.json()?,"requestedTime":start.json()?,"duration":engine.duration.json()?,"width":engine.width,"height":engine.height,"scale":scale,"frameRate":engine.fps.json()?,"audioSampleRate":engine.rate,"audioOffset":audio_offset.json()?,"mime":mime,"playlist":"stream.m3u8","playlistWindowSegments":24,"segmentTargetDuration":{"num":1,"den":2},"expiredPlayback":"restart-at-current-time"});
    publish(
        &out_dir.join("metadata.json"),
        &serde_json::to_vec(&metadata)?,
    )?;
    let cancel = progress.cancellation();
    let encoder_control = cancel.child();
    let halt = Arc::new(AtomicBool::new(false));
    let (vp, video, vwriter) = stream_input(cancel.clone(), halt.clone())?;
    let (ap, audio, awriter) = stream_input(cancel.clone(), halt.clone())?;
    let gop = engine.fps.div(&Time::integer(2))?.ceil_i64()?.max(1);
    let mut command = Command::new("ffmpeg");
    command.args(["-nostdin","-v","error","-y","-probesize","32","-analyzeduration","0","-fpsprobesize","0","-f","rawvideo","-pixel_format","rgba64le","-video_size",&format!("{}x{}",engine.width,engine.height),"-framerate",&engine.fps.expression(),"-i",&format!("tcp://127.0.0.1:{vp}"),"-probesize","32","-analyzeduration","0","-f","f32le","-ar",&engine.rate.to_string(),"-ac",&engine.channels.to_string(),"-i",&format!("tcp://127.0.0.1:{ap}"),"-filter_threads","2","-vf","scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709","-c:v","libx264","-threads","2","-profile:v","baseline","-level:v",level,"-preset","ultrafast","-tune","zerolatency","-crf","28","-g",&gop.to_string(),"-keyint_min",&gop.to_string(),"-sc_threshold","0","-bf","0","-c:a","aac","-avoid_negative_ts","disabled","-f","hls","-hls_segment_type","fmp4","-hls_time","0.5","-hls_list_size","24","-hls_delete_threshold","2","-hls_flags","independent_segments+temp_file+delete_segments","-hls_fmp4_init_filename","init.mp4","-hls_segment_filename"]).arg(out_dir.join("part-%06d.m4s")).arg(out_dir.join("stream.m3u8"));
    // Preserve the video origin at zero. AAC priming retains negative PTS
    // instead of shifting every video frame by one codec packet.
    let encoder_cancel = encoder_control.clone();
    let encoder_halt = halt.clone();
    let encoder = thread::spawn(move || {
        let result =
            crate::media_process::output_cancellable(&mut command, &encoder_cancel, |_| {});
        encoder_halt.store(true, Ordering::Release);
        result
    });
    let began = Instant::now();
    progress.start("preview", Some(frames as u64), "frames");
    let generated = (|| -> Result<()> {
        let mut next_sample = sample_start;
        for offset in 0..frames {
            cancel.check()?;
            let at = Time::integer(first + offset).div(&engine.fps)?;
            while crate::animation::float(&at.sub(&origin))? > began.elapsed().as_secs_f64() + 2. {
                cancel.check()?;
                ensure!(!halt.load(Ordering::Acquire), "ENCODER_STOPPED");
                thread::sleep(Duration::from_millis(20));
            }
            send(&video, engine.video(&at, false)?, &cancel, &halt)?;
            let next = Time::integer(offset + 1)
                .div(&engine.fps)?
                .mul(&Time::integer(engine.rate))
                .ceil_i64()?
                .min(sample_end);
            if next > next_sample {
                send(
                    &audio,
                    engine.audio_at(
                        &origin.add(&Time::new(next_sample, engine.rate)?),
                        (next - next_sample) as usize,
                    )?,
                    &cancel,
                    &halt,
                )?;
                next_sample = next;
            }
            progress.advance_phase(
                "preview",
                Some(frames as u64),
                "frames",
                (offset + 1) as u64,
            );
        }
        Ok(())
    })();
    drop(video);
    drop(audio);
    if generated.is_err() {
        halt.store(true, Ordering::Release);
        encoder_control.cancel();
    }
    let video_result = vwriter
        .join()
        .map_err(|_| anyhow::anyhow!("PREVIEW_WRITER_FAILED"))?;
    let audio_result = awriter
        .join()
        .map_err(|_| anyhow::anyhow!("PREVIEW_WRITER_FAILED"))?;
    let encoded = encoder
        .join()
        .map_err(|_| anyhow::anyhow!("PREVIEW_ENCODER_FAILED"))?;
    if generated.as_ref().err().is_some_and(|error| {
        matches!(
            error.to_string().as_str(),
            "ENCODER_STOPPED" | "ENCODER_INPUT_CLOSED"
        )
    }) {
        match encoded {
            Err(error) if error.to_string().contains("JOB_CANCELLED") => {}
            Err(error) => return Err(error),
            Ok(output) => ensure!(
                output.status.success(),
                "MEDIA_PROCESS_FAILED: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
        return Err(generated.unwrap_err());
    }
    generated?;
    video_result?;
    audio_result?;
    let output = encoded?;
    ensure!(
        output.status.success(),
        "MEDIA_PROCESS_FAILED: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    progress.check_cancelled()?;
    progress.finish();
    Ok(metadata)
}

/// Asset thumbnail, composed by the same evaluator as a one-frame sequence.
pub fn thumbnail(
    project: &Value,
    dir: &Path,
    asset_id: &str,
    stream_id: &str,
    progress: &Progress,
) -> Result<PreviewFrame> {
    validate(project, dir, progress)?;
    let asset = project["assets"].get(asset_id).context("ASSET_NOT_FOUND")?;
    let stream = array(&asset["streams"])
        .iter()
        .find(|s| s["id"] == stream_id)
        .context("STREAM_NOT_FOUND")?;
    ensure!(
        matches!(text(&stream["kind"]), "image" | "video"),
        "THUMBNAIL_REQUIRES_VIDEO"
    );
    let cache = dir.join("cache").join("interactive-v1");
    fs::create_dir_all(&cache)?;
    let root = text(&project["rootSequenceId"]);
    let thumb_key = crate::store::hash(
        &json!({"asset":asset,"streamId":stream_id,"format":project["sequences"][root]["format"],"profile":"thumbnail-v2"}),
    )?;
    let thumbnail_path = cache.join(format!("thumbnail-{thumb_key}.png"));
    let (width, height) = crate::frame_render::dimensions(
        project["sequences"][root]["format"]["width"]
            .as_u64()
            .unwrap() as usize,
        project["sequences"][root]["format"]["height"]
            .as_u64()
            .unwrap() as usize,
        true,
    );
    if asset["fingerprint"]["value"].is_string() && thumbnail_path.is_file() {
        return Ok(PreviewFrame {
            bytes: fs::read(&thumbnail_path)?,
            metadata: json!({"assetId":asset_id,"streamId":stream_id,"revision":project["revision"],"mime":"image/png","width":width,"height":height}),
        });
    }
    let mut p = project.clone();
    let sid = text(&p["rootSequenceId"]).to_owned();
    let mut item = json!({"id":crate::id("item"),"type":"media","assetId":asset_id,"streamId":stream_id,"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}});
    if crate::image_sequence::is_sequence(asset) {
        let duration = Time::from_json(&stream["duration"])?.min(Time::integer(1));
        item["sourceRange"] = crate::time::range(&Time::zero(), &duration)?;
    } else if stream["kind"] == "video" {
        let native = crate::media::probe(&crate::media::local_path(dir, text(&asset["uri"]))?)?;
        let starts = array(&native["streams"])
            .iter()
            .map(|s| {
                let base: Time = text(&s["time_base"]).parse()?;
                Ok(Time::integer(s["start_pts"].as_i64().unwrap_or(0)).mul(&base))
            })
            .collect::<Result<Vec<_>>>()?;
        let origin = starts.iter().min().cloned().unwrap_or_else(Time::zero);
        let index = array(&native["streams"])
            .iter()
            .position(|s| s["index"] == stream["index"])
            .context("native stream missing")?;
        let start = starts[index].sub(&origin);
        let duration = Time::from_json(&stream["duration"])?.min(Time::integer(1));
        item["sourceRange"] = crate::time::range(&start, &duration)?;
    }
    let seq = &mut p["sequences"][&sid];
    seq["durationMode"] = json!("fixed");
    seq["duration"] = Time::integer(1).json()?;
    seq["transitions"] = json!([]);
    seq["links"] = json!([]);
    seq["tracks"] = json!([{"id":crate::id("track"),"kind":"video","name":"Thumbnail","overlapPolicy":"forbid","items":[item]}]);
    let mut result = frame(&p, dir, &sid, &Time::zero(), progress)?;
    result.metadata["assetId"] = json!(asset_id);
    result.metadata["streamId"] = json!(stream_id);
    publish(&thumbnail_path, &result.bytes)?;
    prune_artifacts(&cache, &thumbnail_path)?;
    Ok(result)
}

/// Peak envelopes are asset-time data, never a second timeline audio renderer.
pub fn waveform(
    project: &Value,
    dir: &Path,
    asset_id: &str,
    stream_id: &str,
    progress: &Progress,
) -> Result<Value> {
    validate(project, dir, progress)?;
    let asset = project["assets"].get(asset_id).context("ASSET_NOT_FOUND")?;
    let stream = array(&asset["streams"])
        .iter()
        .find(|s| s["id"] == stream_id)
        .context("STREAM_NOT_FOUND")?;
    ensure!(stream["kind"] == "audio", "WAVEFORM_REQUIRES_AUDIO");
    let duration = Time::from_json(&stream["duration"])?;
    let source_path = crate::media::local_path(dir, text(&asset["uri"]))?;
    let source_stat = fs::metadata(&source_path)?;
    let key = crate::store::hash(
        &json!({"path":source_path,"sourceSize":source_stat.len(),"sourceModified":format!("{:?}",source_stat.modified().ok()),"fingerprint":asset["fingerprint"],"stream":stream_id,"profile":"peaks-1024-native-v1"}),
    )?;
    let cache = dir.join("cache").join("interactive-v1");
    fs::create_dir_all(&cache)?;
    let output = cache.join(format!("waveform-{key}.json"));
    if output.is_file() {
        let mut result: Value = serde_json::from_slice(&fs::read(output)?)?;
        result["assetId"] = json!(asset_id);
        result["streamId"] = json!(stream_id);
        return Ok(result);
    }
    // Keep all channels so opposite-phase stereo does not disappear in a mono mix.
    let channels = stream["audio"]["channels"]
        .as_u64()
        .context("audio channels")? as usize;
    ensure!(
        channels > 0 && channels <= 64,
        "WAVEFORM_CHANNEL_LAYOUT_UNSUPPORTED"
    );
    let rate = stream["audio"]["sampleRate"]
        .as_i64()
        .context("audio sample rate")?;
    let samples = duration.mul(&Time::integer(rate)).ceil_i64()?.max(1) as usize;
    let bins = 1024usize.min(samples);
    let scratch = tempfile::tempdir_in(&cache)?;
    let raw = scratch.path().join("audio.f32");
    progress.ffmpeg(
        Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(crate::media::local_path(dir, text(&asset["uri"]))?)
            .args([
                "-map",
                &format!("0:{}", stream["index"]),
                "-vn",
                "-c:a",
                "pcm_f32le",
                "-f",
                "f32le",
            ])
            .arg(&raw),
        "waveform",
        None,
    )?;
    let mut file = std::io::BufReader::new(fs::File::open(&raw)?);
    let mut bytes = vec![0; channels * 4];
    let mut peaks = vec![0f64; bins];
    let mut n = 0usize;
    loop {
        if n % 4096 == 0 {
            progress.check_cancelled()?;
        }
        match file.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        let peak = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()).abs() as f64)
            .fold(0f64, f64::max);
        let bucket = (n.saturating_mul(bins) / samples).min(bins - 1);
        peaks[bucket] = peaks[bucket].max(peak.min(1.));
        n += 1;
    }
    let result = json!({"assetId":asset_id,"streamId":stream_id,"duration":duration.json()?,"peaks":peaks,"sampleCount":n,"sampleRate":rate});
    progress.check_cancelled()?;
    publish(&output, &serde_json::to_vec(&result)?)?;
    prune_artifacts(&cache, &output)?;
    progress.finish();
    Ok(result)
}
