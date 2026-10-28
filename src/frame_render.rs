//! Deterministic reference renderer: evaluate each output timestamp independently.
//! Decoded media is indexed on disk, so reverse/remap do not retain whole clips in RAM.
use crate::media_process::MediaCommand;
use crate::{
    animation as anim, media,
    project::{self, array, enabled, text},
    time::{bounds, Time},
    visual::{self, Space},
};
use anyhow::{bail, ensure, Context, Result};
use num_traits::ToPrimitive;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock, Weak},
};
static WINDOW_BUILD: Mutex<()> = Mutex::new(());
static WINDOW_LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<()>>>> = OnceLock::new();
static VIDEO_WINDOW_INDEX: OnceLock<Mutex<HashMap<String, Vec<Time>>>> = OnceLock::new();
static NATIVE_CACHE: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
fn prune_windows(cache: &Path) -> Result<()> {
    let mut files = fs::read_dir(cache)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "raw" {
                return None;
            }
            let m = e.metadata().ok()?;
            Some((p, m.len(), m.modified().ok()))
        })
        .collect::<Vec<_>>();
    let mut total: u64 = files.iter().map(|(_, n, _)| n).sum();
    files.sort_by_key(|(_, _, time)| *time);
    let leases = WINDOW_LEASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    for (path, len, _) in files {
        if total <= 512 * 1024 * 1024 {
            break;
        }
        if leases.get(&path).and_then(Weak::upgrade).is_some() {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            let _ = fs::remove_file(path.with_extension("pts"));
            let _ = fs::remove_file(path.with_extension("ready"));
            total = total.saturating_sub(len);
        }
    }
    Ok(())
}
type Frame = Vec<[f64; 4]>; // linear-sRGB, premultiplied alpha
struct Source {
    _lease: Option<Arc<()>>,
    pcm: Option<Vec<f32>>,
    file: File,
    pts: Vec<Time>,
    w: usize,
    h: usize,
    sar: f64,
    space: Space,
    samples: usize,
    rate: i64,
    channels: usize,
    start: Time,
    end: Time,
}
struct Engine<'a> {
    progress: &'a crate::progress::Progress,
    project: &'a Value,
    preview: bool,
    scale: Option<f64>,
    text_scales: HashMap<String, f64>,
    dir: &'a Path,
    scratch: &'a Path,
    sources: HashMap<String, Source>,
    // Bounded per-render cache; keys fix file bytes, interpretation and dimensions.
    image_cache: VecDeque<(String, Vec<u8>)>,
    interactive: bool,
    native: HashMap<String, Value>,
}
/// Preview bounds are applied before per-pixel processing, not just at encoding.
pub(crate) fn dimensions(w: usize, h: usize, preview: bool) -> (usize, usize) {
    if !preview {
        return (w, h);
    }
    let scale = (640. / w.max(h) as f64).min(1.);
    let even = |n: usize| (((n as f64 * scale).floor() as usize / 2) * 2).max(2);
    (even(w), even(h))
}
fn scaled_dimensions(w: usize, h: usize, preview: bool, scale: Option<f64>) -> (usize, usize) {
    match scale {
        Some(scale) => {
            let even = |n: usize| ((n as f64 * scale).floor() as usize / 2 * 2).max(2);
            (even(w), even(h))
        }
        None => dimensions(w, h, preview),
    }
}
fn canvas(format: &Value, preview: bool) -> (usize, usize) {
    dimensions(
        format["width"].as_u64().unwrap() as usize,
        format["height"].as_u64().unwrap() as usize,
        preview,
    )
}
fn run(cmd: &mut Command) -> Result<()> {
    let r = cmd.media_output()?;
    ensure!(
        r.status.success(),
        "MEDIA_PROCESS_FAILED: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    Ok(())
}
fn signed(s: &str) -> Result<Time> {
    if let Some(s) = s.strip_prefix('-') {
        Ok(Time::zero().sub(&s.parse()?))
    } else {
        s.parse()
    }
}
fn stream_start(s: &Value) -> Result<Time> {
    if let Some(n) = s["start_pts"].as_i64() {
        Ok(Time::integer(n).mul(&text(&s["time_base"]).parse()?))
    } else {
        signed(s["start_time"].as_str().unwrap_or("0"))
    }
}
fn frame_size(w: usize, h: usize) -> Result<usize> {
    w.checked_mul(h)
        .and_then(|n| n.checked_mul(8))
        .context("frame size overflow")
}
fn rgba(v: &Value, default: [f64; 4]) -> [f64; 4] {
    let mut c = default;
    for (i, k) in ["r", "g", "b", "a"].iter().enumerate() {
        if let Some(n) = v[k].as_f64() {
            c[i] = n;
        }
    }
    for i in 0..3 {
        c[i] *= c[3];
    }
    c
}
fn blend(a: &Frame, b: &Frame, p: f64) -> Frame {
    a.iter()
        .zip(b)
        .map(|(a, b)| std::array::from_fn(|i| a[i] * (1. - p) + b[i] * p))
        .collect()
}
fn over(dst: &mut Frame, src: &Frame) {
    for (a, b) in dst.iter_mut().zip(src) {
        for i in 0..4 {
            a[i] = b[i] + a[i] * (1. - b[3]);
        }
    }
}
fn bytes(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.len() * 8);
    for c in frame {
        for i in 0..4 {
            let v = if i == 3 {
                c[3]
            } else if c[3] > 0. {
                Space::Rec709.encode(c[i] / c[3])
            } else {
                0.
            };
            out.extend_from_slice(&((v.clamp(0., 1.) * 65535.).round() as u16).to_le_bytes());
        }
    }
    out
}
fn linear(raw: &[u8], space: Space) -> Frame {
    static TABLES: OnceLock<[Vec<f64>; 3]> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        [Space::Linear, Space::Srgb, Space::Rec709]
            .map(|s| (0..=65535).map(|n| s.decode(n as f64 / 65535.)).collect())
    });
    let table = &tables[match space {
        Space::Linear => 0,
        Space::Srgb => 1,
        Space::Rec709 => 2,
    }];
    raw.chunks_exact(8)
        .map(|b| {
            let a = u16::from_le_bytes([b[6], b[7]]) as f64 / 65535.;
            if a == 0. {
                return [0.; 4];
            }
            [
                table[u16::from_le_bytes([b[0], b[1]]) as usize] * a,
                table[u16::from_le_bytes([b[2], b[3]]) as usize] * a,
                table[u16::from_le_bytes([b[4], b[5]]) as usize] * a,
                a,
            ]
        })
        .collect()
}

fn active(t: &Time, start: &Time, end: &Time, left: bool) -> bool {
    if left {
        t > start && t <= end
    } else {
        t >= start && t < end
    }
}
impl Engine<'_> {
    fn dimensions(&self, w: usize, h: usize) -> (usize, usize) {
        scaled_dimensions(w, h, self.preview, self.scale)
    }
    fn canvas(&self, format: &Value) -> (usize, usize) {
        self.dimensions(
            format["width"].as_u64().unwrap() as usize,
            format["height"].as_u64().unwrap() as usize,
        )
    }

    /// Interactive decoding retains source timestamps and only materializes a
    /// small window. The full renderer deliberately keeps its existing path.
    fn window_source(
        &mut self,
        item: &Value,
        audio: bool,
        rate: i64,
        channels: usize,
        at: &Time,
    ) -> Result<String> {
        self.progress.check_cancelled()?;
        let key = format!(
            "window:{}:{}:{audio}:{rate}:{channels}",
            text(&item["assetId"]),
            text(&item["streamId"])
        );
        if let Some(s) = self.sources.get(&key) {
            let upper = if audio {
                s.start
                    .add(&Time::new(s.samples.saturating_sub(2) as i64, s.rate)?)
            } else {
                s.pts.last().cloned().unwrap_or_else(Time::zero)
            };
            if at >= &s.start && (at < &upper || (upper >= s.end && at <= &s.end)) {
                return Ok(key);
            }
        }
        let asset = &self.project["assets"][text(&item["assetId"])];
        let stream = array(&asset["streams"])
            .iter()
            .find(|s| s["id"] == item["streamId"])
            .context("missing stream")?;
        let path = media::local_path(self.dir, text(&asset["uri"]))?;
        let aid = text(&item["assetId"]).to_owned();
        let source_stat = fs::metadata(&path)?;
        let source_stamp = format!("{}:{:?}", source_stat.len(), source_stat.modified().ok());
        let native_key = format!(
            "{}:{}:{source_stamp}",
            path.display(),
            text(&asset["fingerprint"]["value"])
        );
        if let Some(value) = NATIVE_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .get(&native_key)
            .cloned()
        {
            self.native.insert(aid.clone(), value);
        }
        if !self.native.contains_key(&aid) {
            self.progress.start("probe", None, "assets");
            let result = self.progress.output(
                Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-show_streams",
                        "-show_format",
                        "-of",
                        "json",
                    ])
                    .arg(&path),
            )?;
            ensure!(
                result.status.success(),
                "FFPROBE_FAILED: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            self.native
                .insert(aid.clone(), serde_json::from_slice(&result.stdout)?);
        }
        let mut native_cache = NATIVE_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        if native_cache.len() > 256 {
            native_cache.clear();
        }
        native_cache.insert(native_key, self.native[&aid].clone());
        drop(native_cache);
        let native = &self.native[&aid];
        let ns = array(&native["streams"])
            .iter()
            .find(|s| s["index"] == stream["index"])
            .context("missing native stream")?;
        let origin = array(&native["streams"])
            .iter()
            .map(stream_start)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .min()
            .unwrap_or_else(Time::zero);
        let native_start = stream_start(ns)?;
        let base: Time = text(&ns["time_base"]).parse()?;
        let duration = if let Some(ticks) = ns["duration_ts"].as_i64() {
            Time::integer(ticks).mul(&base)
        } else {
            Time::from_json(&stream["duration"])?
        };
        let end = native_start.add(&duration).sub(&origin);
        let bucket = Time::integer(
            at.mul(&Time::integer(2))
                .0
                .floor()
                .to_integer()
                .to_i64()
                .context("window time overflow")?,
        )
        .div(&Time::integer(2))?;
        let native_at = bucket.add(&origin);
        let seek = native_at
            .sub(&Time::integer(if audio { 5 } else { 1 }))
            .max(native_start.clone());
        let until = native_at
            .add(&Time::integer(2))
            .min(native_start.add(&duration));
        let index = self
            .sources
            .keys()
            .filter(|k| k.starts_with("window:"))
            .count();
        let _build = WINDOW_BUILD.lock().unwrap();
        let cache = self.dir.join("cache").join("decode-windows-v1");
        fs::create_dir_all(&cache)?;
        // Index timestamps before choosing a byte-bounded pixel window. Input
        // seeking may decode a long GOP; those preroll frames must not consume
        // the pixel budget before we reach the requested timestamp.
        let mut output_start = seek.clone();
        let mut video_limit = 0usize;
        let mut indexed_last = None;
        if !audio {
            let (w, h) = self.dimensions(
                stream["video"]["width"].as_u64().context("source width")? as usize,
                stream["video"]["height"]
                    .as_u64()
                    .context("source height")? as usize,
            );
            video_limit = (256 * 1024 * 1024 / frame_size(w, h)?).max(3);
            let index_key = crate::store::hash(&json!({
                "path":path,"fingerprint":asset["fingerprint"],"stamp":source_stamp,
                "stream":stream["index"],"seek":seek.json()?,"until":until.json()?
            }))?;
            let cached_index = VIDEO_WINDOW_INDEX
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap()
                .get(&index_key)
                .cloned();
            let timestamps = if let Some(index) = cached_index {
                index
            } else {
                let result = self.progress.output(
                    Command::new("ffprobe")
                        .args([
                            "-v",
                            "error",
                            "-threads",
                            "2",
                            "-select_streams",
                            &stream["index"].to_string(),
                            "-read_intervals",
                            &format!("{}%{}", anim::float(&seek)?, anim::float(&until)?),
                            "-show_frames",
                            "-show_entries",
                            "frame=best_effort_timestamp",
                            "-of",
                            "json",
                        ])
                        .arg(&path),
                )?;
                ensure!(
                    result.status.success(),
                    "MEDIA_PROCESS_FAILED: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let index: Value = serde_json::from_slice(&result.stdout)?;
                let timestamps = array(&index["frames"])
                    .iter()
                    .map(|frame| {
                        Ok(Time::integer(
                            frame["best_effort_timestamp"]
                                .as_i64()
                                .context("SOURCE_INDEX_INVALID: missing timestamp")?,
                        )
                        .mul(&base))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|pts| pts < &until)
                    .collect::<Vec<_>>();
                ensure!(
                    !timestamps.is_empty() && timestamps.windows(2).all(|pair| pair[0] < pair[1]),
                    "SOURCE_INDEX_INVALID: empty or nonincreasing window timestamps"
                );
                let mut cache = VIDEO_WINDOW_INDEX.get().unwrap().lock().unwrap();
                if cache.len() >= 64 {
                    cache.clear();
                }
                cache.insert(index_key, timestamps.clone());
                timestamps
            };
            let target = at.add(&origin);
            let selected = timestamps
                .partition_point(|pts| pts <= &target)
                .saturating_sub(1);
            // Keep a predecessor for exclusive-upper sampling, plus a successor
            // for frame blending. Further frames amortize sequential playback.
            let stride = video_limit - 2;
            let first = (selected / stride * stride).saturating_sub(1);
            output_start = timestamps[first].clone();
            indexed_last = timestamps.last().cloned();
        }
        let cache_key = crate::store::hash(
            &json!({"path":path,"fingerprint":asset["fingerprint"],"sourceStamp":source_stamp,"stream":stream,"at":bucket.json()?,"outputStart":output_start.json()?,"audio":audio,"rate":rate,"channels":channels,"scale":if audio {None} else {self.scale},"decoder":"window-v4","renderer":env!("CARGO_PKG_VERSION")}),
        )?;
        let cached_raw = cache.join(format!("{cache_key}.raw"));
        let cached_meta = cache.join(format!("{cache_key}.pts"));
        let ready = cache.join(format!("{cache_key}.ready"));
        // Decode into a private staging directory, publish the marker last.
        let staging = tempfile::tempdir_in(&cache)?;
        // Stable names allow old windows to be replaced instead of filling disk.
        // Stream IDs are domain identifiers, not safe FFmpeg muxer paths.
        let stem = "window";
        let raw_path = staging.path().join(format!("{stem}.raw"));
        let meta_path = staging.path().join(format!("{stem}.pts"));
        self.sources.remove(&key);
        let mut cmd = Command::new("ffmpeg");
        cmd.current_dir(staging.path())
            .args([
                "-nostdin",
                "-v",
                "error",
                "-y",
                "-copyts",
                "-seek_timestamp",
                "1",
                "-ss",
                &format!("{}us", seek.mul(&Time::integer(1_000_000)).ceil_i64()?),
                "-noaccurate_seek",
                "-threads",
                "2",
                "-noautorotate",
                "-i",
            ])
            .arg(&path)
            .args([
                "-map",
                &format!("0:{}", stream["index"]),
                "-threads",
                "2",
                "-to",
                &format!("{}us", until.mul(&Time::integer(1_000_000)).ceil_i64()?),
            ]);
        let (mut w, mut h, mut sar, mut space) = (1, 1, 1., Space::Rec709);
        let mut audio_origin = None;
        if audio {
            let native_rate = text(&ns["sample_rate"]).parse::<i64>()?;
            let (mut a, mut b) = (native_rate, rate);
            while b != 0 {
                let rem = a % b;
                a = b;
                b = rem;
            }
            let period = native_rate / a;
            let aligned = seek
                .sub(&native_start)
                .mul(&Time::integer(native_rate))
                .div(&Time::integer(period))?
                .ceil_i64()?
                * period;
            let aligned_time = native_start.add(&Time::new(aligned, native_rate)?);
            let input_pts = aligned_time.mul(&Time::integer(native_rate));
            ensure!(input_pts.0.is_integer(), "AUDIO_WINDOW_CLOCK_UNSUPPORTED");
            let filter = format!(
                "atrim=start_pts={},asetpts=PTS-STARTPTS,aresample={rate}",
                input_pts.ceil_i64()?
            );
            audio_origin = Some(aligned_time.sub(&origin));
            cmd.args([
                "-vn",
                "-ar",
                &rate.to_string(),
                "-ac",
                &channels.to_string(),
                "-c:a",
                "pcm_f32le",
                "-af",
                &filter,
                "-t",
                &format!(
                    "{}us",
                    until
                        .sub(&aligned_time)
                        .mul(&Time::integer(1_000_000))
                        .ceil_i64()?
                ),
            ]);
        } else {
            ensure!(
                stream["video"]["alphaMode"] != "premultiplied",
                "premultiplied source media unsupported"
            );
            let ow = stream["video"]["width"].as_u64().context("source width")? as usize;
            let oh = stream["video"]["height"]
                .as_u64()
                .context("source height")? as usize;
            ensure!(
                ns["width"].as_u64() == Some(ow as u64) && ns["height"].as_u64() == Some(oh as u64),
                "MEDIA_DIMENSION_MISMATCH"
            );
            (w, h) = self.dimensions(ow, oh);
            sar = anim::float(&Time::from_json(&stream["video"]["pixelAspect"])?)?
                * (ow as f64 / oh as f64)
                / (w as f64 / h as f64);
            space = Space::parse(text(&stream["video"]["colorSpace"]))?;
            let rgb = ["rgb", "bgr", "gbr"]
                .iter()
                .any(|p| text(&ns["pix_fmt"]).starts_with(p));
            let filter = if rgb {
                format!(
                    "trim=start_pts={}:end_pts={},scale={w}:{h}:flags=fast_bilinear,format=rgba64le",
                    output_start.div(&base)?.ceil_i64()?,
                    until.div(&base)?.ceil_i64()?
                )
            } else {
                format!("trim=start_pts={}:end_pts={},scale={w}:{h}:flags=fast_bilinear:in_color_matrix=bt709:out_range=full,format=rgba64le",output_start.div(&base)?.ceil_i64()?,until.div(&base)?.ceil_i64()?)
            };
            cmd.args([
                "-an",
                "-vf",
                &filter,
                "-fps_mode",
                "passthrough",
                "-enc_time_base",
                &base.expression(),
                "-frames:v",
                &video_limit.to_string(),
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                "rgba64le",
            ]);
        }
        cmd.args([
            "-f",
            "tee",
            &format!(
                "[f={}]{}.raw|[f=framehash:hash=adler32]{}.pts",
                if audio { "f32le" } else { "rawvideo" },
                stem,
                stem
            ),
        ]);
        self.progress.start(
            if audio {
                "decode_audio"
            } else {
                "decode_video"
            },
            None,
            "windows",
        );
        if !ready.is_file() || !cached_raw.is_file() || !cached_meta.is_file() {
            let result = self.progress.output(&mut cmd)?;
            ensure!(
                result.status.success(),
                "MEDIA_PROCESS_FAILED: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            self.progress.check_cancelled()?;
            fs::rename(&raw_path, &cached_raw)?;
            fs::rename(&meta_path, &cached_meta)?;
            fs::write(&ready, b"ready")?;
        }
        let metadata = fs::read_to_string(&cached_meta)?;
        let mut pts = Vec::new();
        let mut tb = None;
        let mut expected_bytes = 0usize;
        for line in metadata.lines() {
            if let Some(value) = line.strip_prefix("#tb 0: ") {
                tb = Some(value.trim().parse::<Time>()?);
            }
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            ensure!(
                fields.len() >= 6,
                "SOURCE_INDEX_INVALID: malformed window metadata"
            );
            let packet_time = Time::integer(fields[2].parse()?)
                .mul(tb.as_ref().context("source time base missing")?);
            pts.push(if let Some(start) = &audio_origin {
                start.add(&packet_time)
            } else {
                packet_time.sub(&origin)
            });
            expected_bytes = expected_bytes
                .checked_add(fields[4].parse::<usize>()?)
                .context("window size overflow")?;
        }
        let file = File::open(&cached_raw)?;
        let len = file.metadata()?.len() as usize;
        ensure!(
            !pts.is_empty() && len == expected_bytes,
            "SOURCE_INDEX_INVALID: empty or incomplete window"
        );
        ensure!(
            pts.windows(2).all(|p| p[0] < p[1]),
            "SOURCE_INDEX_INVALID: nonincreasing timestamps"
        );
        let start = pts[0].clone();
        let samples = if audio {
            len / (channels * 4)
        } else {
            ensure!(
                len == pts.len() * frame_size(w, h)?,
                "SOURCE_INDEX_INVALID: frame sizes"
            );
            0
        };
        if !audio {
            let full_tail = until >= native_start.add(&duration)
                && indexed_last
                    .as_ref()
                    .is_some_and(|last| pts.last().unwrap().add(&origin) >= *last);
            ensure!(
                at >= &start && (at <= pts.last().unwrap() || full_tail),
                "PREVIEW_DECODE_WINDOW_LIMIT: source requires a smaller decode window"
            );
        }
        let pcm = if audio {
            Some(
                fs::read(&cached_raw)?
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect(),
            )
        } else {
            None
        };
        // A session may visit thousands of clips. Retain only a bounded set of
        // source windows; the shared disk cache can reopen evicted windows.
        loop {
            let windows = self
                .sources
                .iter()
                .filter(|(key, _)| key.starts_with("window:"))
                .collect::<Vec<_>>();
            let held: u64 = windows
                .iter()
                .map(|(_, s)| s.file.metadata().map_or(0, |m| m.len()))
                .sum();
            if windows.len() < 4 && held + len as u64 <= 256 * 1024 * 1024 {
                break;
            }
            let Some(expired) = windows.first().map(|(key, _)| (*key).clone()) else {
                break;
            };
            self.sources.remove(&expired);
        }
        let mut leases = WINDOW_LEASES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        let lease = leases
            .get(&cached_raw)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(()));
        leases.retain(|_, lease| lease.strong_count() > 0);
        leases.insert(cached_raw.clone(), Arc::downgrade(&lease));
        drop(leases);
        prune_windows(&cache)?;
        self.sources.insert(
            key.clone(),
            Source {
                _lease: Some(lease),
                pcm,
                file,
                pts,
                w,
                h,
                sar,
                space,
                samples,
                rate,
                channels,
                start,
                end,
            },
        );
        self.progress.advance((index + 1) as u64);
        Ok(key)
    }
    fn source(&mut self, item: &Value, audio: bool, rate: i64, channels: usize) -> Result<String> {
        let key = format!(
            "{}:{}:{audio}:{rate}:{channels}",
            text(&item["assetId"]),
            text(&item["streamId"])
        );
        if self.sources.contains_key(&key) {
            return Ok(key);
        }
        let _restore = self.progress.preserve();
        self.progress.start("probe", None, "assets");
        let asset = &self.project["assets"][text(&item["assetId"])];
        let stream = array(&asset["streams"])
            .iter()
            .find(|s| s["id"] == item["streamId"])
            .context("missing stream")?;
        let path = media::local_path(self.dir, text(&asset["uri"]))?;
        let native = media::probe(&path)?;
        let ns = array(&native["streams"])
            .iter()
            .find(|s| s["index"] == stream["index"])
            .context("missing native stream")?;
        let origin = array(&native["streams"])
            .iter()
            .map(stream_start)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .min()
            .unwrap_or(Time::zero());
        let start = stream_start(ns)?.sub(&origin);
        let mut end = Time::zero();
        let output = self
            .scratch
            .join(format!("source-{}.raw", self.sources.len()));
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-threads",
            "2",
            "-noautorotate",
            "-i",
        ])
        .arg(&path)
        .args(["-map", &format!("0:{}", stream["index"]), "-threads", "2"]);
        let (mut pts, mut w, mut h, mut sar, mut space) = (vec![], 1, 1, 1., Space::Rec709);
        if audio {
            cmd.args([
                "-vn",
                "-ar",
                &rate.to_string(),
                "-ac",
                &channels.to_string(),
                "-c:a",
                "pcm_f32le",
                "-f",
                "f32le",
            ]);
        } else {
            ensure!(
                stream["video"]["alphaMode"] != "premultiplied",
                "premultiplied source media unsupported"
            );
            w = stream["video"]["width"].as_u64().context("source width")? as usize;
            h = stream["video"]["height"]
                .as_u64()
                .context("source height")? as usize;
            ensure!(
                ns["width"].as_u64() == Some(w as u64) && ns["height"].as_u64() == Some(h as u64),
                "MEDIA_DIMENSION_MISMATCH"
            );
            sar = anim::float(&Time::from_json(&stream["video"]["pixelAspect"])?)?;
            space = Space::parse(text(&stream["video"]["colorSpace"]))?;
            let (dw, dh) = self.dimensions(w, h);
            sar *= (w as f64 / h as f64) / (dw as f64 / dh as f64);
            w = dw;
            h = dh;
            let rgb = ["rgb", "bgr", "gbr"]
                .iter()
                .any(|p| text(&ns["pix_fmt"]).starts_with(p));
            let filter = if rgb && !self.preview {
                "format=rgba64le".to_string()
            } else if rgb {
                format!("scale={w}:{h}:flags=fast_bilinear,format=rgba64le")
            } else {
                format!(
                    "scale={w}:{h}:flags={}:in_color_matrix=bt709:out_range=full,format=rgba64le",
                    if self.preview {
                        "fast_bilinear"
                    } else {
                        "bicubic"
                    }
                )
            };
            cmd.args([
                "-an",
                "-vf",
                &filter,
                "-pix_fmt",
                "rgba64le",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
            ]);
            if stream["kind"] == "image" {
                pts.push(Time::zero());
                cmd.args(["-frames:v", "1"]);
            } else {
                let result = Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        &stream["index"].to_string(),
                        "-show_frames",
                        "-show_entries",
                        "frame=best_effort_timestamp,duration,pkt_duration",
                        "-of",
                        "json",
                    ])
                    .arg(&path)
                    .media_output()?;
                ensure!(result.status.success(), "timestamp indexing failed");
                let index: Value = serde_json::from_slice(&result.stdout)?;
                let base: Time = text(&ns["time_base"]).parse()?;
                for frame in array(&index["frames"]) {
                    pts.push(
                        Time::integer(
                            frame["best_effort_timestamp"]
                                .as_i64()
                                .context("missing frame timestamp")?,
                        )
                        .mul(&base)
                        .sub(&origin),
                    );
                }
                if let Some(last) = array(&index["frames"]).last() {
                    let step = if let Some(n) = last["duration"]
                        .as_i64()
                        .or_else(|| last["pkt_duration"].as_i64())
                        .filter(|n| *n > 0)
                    {
                        Time::integer(n).mul(&base)
                    } else {
                        {
                            let limit = if let Some(n) = ns["duration_ts"].as_i64() {
                                start.add(&Time::integer(n).mul(&base))
                            } else if let Some(d) = ns["duration"].as_str() {
                                start.add(&d.parse()?)
                            } else {
                                anyhow::bail!("SOURCE_INDEX_INVALID: last frame has no duration")
                            };
                            let step = limit.sub(pts.last().context("empty frame index")?);
                            ensure!(step > Time::zero(), "invalid final frame duration");
                            step
                        }
                    };
                    end = pts.last().context("empty frame index")?.add(&step);
                }
                ensure!(
                    pts.windows(2).all(|p| p[0] < p[1]),
                    "nonincreasing frame timestamps"
                );
            }
        }
        cmd.arg(&output);
        self.progress.ffmpeg(
            &mut cmd,
            if audio {
                "decode_audio"
            } else {
                "decode_video"
            },
            if audio { None } else { Some(pts.len() as u64) },
        )?;
        let file = File::open(output)?;
        let len = file.metadata()?.len() as usize;
        let samples = if audio {
            len / (channels * 4)
        } else {
            ensure!(
                !pts.is_empty() && len == pts.len() * frame_size(w, h)?,
                "decoded frames and timestamp index disagree"
            );
            0
        };
        self.sources.insert(
            key.clone(),
            Source {
                _lease: None,
                pcm: None,
                file,
                pts,
                w,
                h,
                sar,
                space,
                samples,
                rate,
                channels,
                start,
                end,
            },
        );
        Ok(key)
    }
    fn raw_frame(s: &mut Source, index: usize) -> Result<Vec<u8>> {
        let size = frame_size(s.w, s.h)?;
        s.file.seek(SeekFrom::Start((index * size) as u64))?;
        let mut raw = vec![0; size];
        s.file.read_exact(&mut raw)?;
        Ok(raw)
    }
    fn media_frame(
        &mut self,
        item: &Value,
        t: &Time,
        left: bool,
        handle: bool,
    ) -> Result<(Vec<u8>, usize, usize, f64, Space)> {
        let asset = &self.project["assets"][text(&item["assetId"])];
        if crate::image_sequence::is_sequence(asset) {
            let v = &asset["streams"][0]["video"];
            let ow = v["width"].as_u64().unwrap() as usize;
            let oh = v["height"].as_u64().unwrap() as usize;
            let (w, h) = self.dimensions(ow, oh);
            let sar = anim::float(&Time::from_json(&v["pixelAspect"])?)? * (ow as f64 / oh as f64)
                / (w as f64 / h as f64);
            let space = Space::parse(text(&v["colorSpace"]))?;
            let chosen = crate::image_sequence::selection(asset, item, t, left, handle)?;
            let mut output: Option<Frame> = None;
            for (index, weight) in &chosen {
                let f = &asset["source"]["frames"][*index];
                let key = format!(
                    "{}:{w}:{h}:{}:straight:bilinear-v1",
                    text(&f["fingerprint"]["value"]),
                    text(&v["colorSpace"])
                );
                let raw = if let Some(pos) = self.image_cache.iter().position(|(k, _)| k == &key) {
                    let entry = self.image_cache.remove(pos).unwrap();
                    let raw = entry.1.clone();
                    self.image_cache.push_back(entry);
                    raw
                } else {
                    let path = crate::image_sequence::frame_path(self.dir, asset, f)?;
                    ensure!(
                        media::fingerprint(&path)?
                            .eq_ignore_ascii_case(text(&f["fingerprint"]["value"])),
                        "FINGERPRINT_MISMATCH: {}",
                        path.display()
                    );
                    let raw = crate::image_sequence::decode(&path, w, h)?;
                    const BUDGET: usize = 64 * 1024 * 1024;
                    while !self.image_cache.is_empty()
                        && self.image_cache.iter().map(|(_, b)| b.len()).sum::<usize>() + raw.len()
                            > BUDGET
                    {
                        self.image_cache.pop_front();
                    }
                    if raw.len() <= BUDGET {
                        self.image_cache.push_back((key, raw.clone()));
                    }
                    raw
                };
                if chosen.len() == 1 {
                    return Ok((raw, w, h, sar, space));
                }
                let frame = linear(&raw, space);
                let weight = anim::float(weight)?;
                let out = output.get_or_insert_with(|| vec![[0.; 4]; w * h]);
                for (out, pixel) in out.iter_mut().zip(frame) {
                    for c in 0..4 {
                        out[c] += pixel[c] * weight;
                    }
                }
            }
            return Ok((
                bytes(&output.context("empty image sample")?),
                w,
                h,
                sar,
                Space::Rec709,
            ));
        }
        let source_time = if item["sourceRange"].is_null() {
            Time::zero()
        } else {
            anim::mapping(item, t, left)?.0
        };
        let stream = array(&asset["streams"])
            .iter()
            .find(|s| s["id"] == item["streamId"])
            .context("missing stream")?;
        let key = if self.interactive && stream["kind"] == "video" {
            self.window_source(item, false, 0, 0, &source_time)?
        } else {
            self.source(item, false, 0, 0)?
        };
        let s = self.sources.get_mut(&key).unwrap();
        if item["sourceRange"].is_null() {
            return Ok((Self::raw_frame(s, 0)?, s.w, s.h, s.sar, s.space));
        }
        let (source, upper, _) = anim::mapping(item, t, left)?;
        let (ss, se) = bounds(&item["sourceRange"])?;
        let lo = if handle {
            0
        } else {
            s.pts.partition_point(|p| p <= &ss).saturating_sub(1)
        };
        let hi = if handle {
            s.pts.len() - 1
        } else {
            s.pts.partition_point(|p| p < &se).saturating_sub(1)
        };
        ensure!(
            source >= s.pts[0],
            "SOURCE_OUT_OF_BOUNDS: sample precedes first video frame"
        );
        ensure!(
            source < s.end || (upper && source == s.end),
            "SOURCE_OUT_OF_BOUNDS: sample exceeds decoded video handle"
        );
        let idx = s
            .pts
            .partition_point(|p| if upper { p < &source } else { p <= &source })
            .saturating_sub(1)
            .clamp(lo, hi);
        let method = text(&item["playback"]["videoSampling"]["method"]);
        let raw = if method == "frame-blend" {
            let a = s
                .pts
                .partition_point(|p| p <= &source)
                .saturating_sub(1)
                .clamp(lo, hi);
            let b = (a + 1).min(hi);
            if a == b {
                Self::raw_frame(s, a)?
            } else {
                let p = anim::float(&source.sub(&s.pts[a]).div(&s.pts[b].sub(&s.pts[a]))?)?
                    .clamp(0., 1.);
                let fa = linear(&Self::raw_frame(s, a)?, s.space);
                let fb = linear(&Self::raw_frame(s, b)?, s.space);
                return Ok((bytes(&blend(&fa, &fb, p)), s.w, s.h, s.sar, Space::Rec709));
            }
        } else {
            ensure!(
                matches!(method, "" | "frame-hold"),
                "PROCESSOR_UNAVAILABLE: optical-flow"
            );
            Self::raw_frame(s, idx)?
        };
        Ok((raw, s.w, s.h, s.sar, s.space))
    }
    fn text_frame(&mut self, item: &Value, w: usize, h: usize) -> Result<Vec<u8>> {
        let key = format!("text:{}:{w}:{h}", text(&item["id"]));
        if let Some(s) = self.sources.get_mut(&key) {
            return Self::raw_frame(s, 0);
        }
        let style = if item["type"] == "caption" {
            &item["style"]
        } else {
            &item["parameters"]
        };
        for k in ["backgroundColor", "maxLines", "fontFamily"] {
            ensure!(
                style[k].is_null(),
                "BACKEND_UNSUPPORTED: text {k}; pin fontAssetId"
            );
        }
        let content = if item["type"] == "caption" {
            text(&item["text"])
        } else {
            text(&style["text"])
        };
        let n = self.sources.len();
        let txt = format!("text{n}.txt");
        fs::write(self.scratch.join(&txt), content)?;
        let scale = self
            .text_scales
            .get(text(&item["id"]))
            .copied()
            .unwrap_or(1.);
        let text_cache = if self.interactive {
            let directory = self.dir.join("cache").join("interactive-text-v1");
            fs::create_dir_all(&directory)?;
            let font = style["fontAssetId"]
                .as_str()
                .map(|id| &self.project["assets"][id]);
            let hash = crate::store::hash(
                &json!({"style":style,"content":content,"font":font,"width":w,"height":h,"scale":scale,"version":env!("CARGO_PKG_VERSION")}),
            )?;
            Some(directory.join(format!("{hash}.rgba")))
        } else {
            None
        };
        if let Some(path) = text_cache.as_ref().filter(|path| path.is_file()) {
            let raw = fs::read(path)?;
            ensure!(raw.len() == frame_size(w, h)?, "TEXT_CACHE_INVALID");
            return Ok(raw);
        }
        let margin = 20. * scale;
        let x = match text(&style["align"]) {
            "left" => margin.to_string(),
            "right" => format!("w-tw-{margin}"),
            _ => "(w-tw)/2".into(),
        };
        let y = match text(&style["position"]) {
            "top" => margin.to_string(),
            "center" => "(h-th)/2".into(),
            _ => format!("h-th-{}", 40. * scale),
        };
        let font = if let Some(id) = style["fontAssetId"].as_str() {
            let name = format!("font{n}.ttf");
            fs::copy(
                media::local_path(self.dir, text(&self.project["assets"][id]["uri"]))?,
                self.scratch.join(&name),
            )?;
            format!(":fontfile={name}")
        } else {
            String::new()
        };
        let out = self.scratch.join(format!("text{n}.raw"));
        let graph=format!("color=black@0:s={w}x{h},format=rgba,drawtext=textfile={txt}:expansion=none{font}:fontsize={}:fontcolor=white:x={x}:y={y},{},format=rgba64le",style["fontSizePixels"].as_f64().unwrap_or(48.)*scale, visual::text_paint(&style["color"])?);
        run(Command::new("ffmpeg")
            .current_dir(self.scratch)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &graph,
                "-frames:v",
                "1",
                "-threads",
                "2",
                "-pix_fmt",
                "rgba64le",
                "-f",
                "rawvideo",
            ])
            .arg(&out))?;
        let mut s = Source {
            _lease: None,
            pcm: None,
            file: File::open(out)?,
            pts: vec![Time::zero()],
            w,
            h,
            sar: 1.,
            space: Space::Rec709,
            samples: 0,
            rate: 0,
            channels: 0,
            start: Time::zero(),
            end: Time::zero(),
        };
        let raw = Self::raw_frame(&mut s, 0)?;
        if let Some(path) = text_cache {
            let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
            staged.write_all(&raw)?;
            staged.persist(&path).map_err(|e| e.error)?;
            crate::preview::prune_artifacts(path.parent().unwrap(), &path)?;
        }
        self.sources.insert(key, s);
        Ok(raw)
    }
    #[allow(clippy::too_many_arguments)]
    fn item_frame(
        &mut self,
        item: &Value,
        t: &Time,
        left: bool,
        handle: bool,
        w: usize,
        h: usize,
        base: Option<&Frame>,
        depth: usize,
    ) -> Result<Frame> {
        let evaluated = anim::evaluated(item, t)?;
        let (raw, sw, sh, sar, space) = match text(&item["type"]) {
            "media" => self.media_frame(item, t, left, handle)?,
            "sequence" => {
                let (src, upper, _) = anim::mapping(item, t, left)?;
                let child = &self.project["sequences"][text(&item["sequenceId"])];
                let (cw, ch) = self.canvas(&child["format"]);
                let original_aspect = child["format"]["width"].as_f64().unwrap()
                    / child["format"]["height"].as_f64().unwrap();
                let sar = original_aspect / (cw as f64 / ch as f64);
                let f = self.sequence_frame(text(&item["sequenceId"]), &src, upper, depth + 1)?;
                (bytes(&f), cw, ch, sar, Space::Rec709)
            }
            "adjustment" => (
                bytes(base.context("adjustment needs underlying frame")?),
                w,
                h,
                1.,
                Space::Rec709,
            ),
            "caption" => (self.text_frame(item, w, h)?, w, h, 1., Space::Rec709),
            "generator" => {
                crate::schema::validate_generator(item)?;
                match text(&item["generatorType"]) {
                    "builtin.color" => {
                        let c = rgba(&item["parameters"]["color"], [0., 0., 0., 1.]); // generator colors retain existing Rec709 interpretation
                        let mut raw = Vec::with_capacity(w * h * 8);
                        let c = if c[3] > 0. {
                            [c[0] / c[3], c[1] / c[3], c[2] / c[3], c[3]]
                        } else {
                            [0.; 4]
                        };
                        for _ in 0..w * h {
                            for x in c {
                                raw.extend_from_slice(&((x * 65535.).round() as u16).to_le_bytes());
                            }
                        }
                        (raw, w, h, 1., Space::Rec709)
                    }
                    "builtin.text" => (self.text_frame(item, w, h)?, w, h, 1., Space::Rec709),
                    other => bail!("PLUGIN_UNAVAILABLE: {other}"),
                }
            }
            other => bail!("unsupported video item {other}"),
        };
        let geometry = visual::Geometry::new(sw, sh, sar, w, h, &evaluated["video"]["transform"])?;
        let mut pipeline = visual::Pipeline::new(&evaluated, self.project, self.dir, space)?;
        if self.preview && pipeline.spatial() {
            let (ow, oh) = match text(&item["type"]) {
                "media" => {
                    let stream = array(&self.project["assets"][text(&item["assetId"])]["streams"])
                        .iter()
                        .find(|s| s["id"] == item["streamId"])
                        .context("missing effect stream")?;
                    (
                        stream["video"]["width"].as_f64().context("video width")?,
                        stream["video"]["height"].as_f64().context("video height")?,
                    )
                }
                "sequence" => {
                    let format = &self.project["sequences"][text(&item["sequenceId"])]["format"];
                    (
                        format["width"].as_f64().unwrap(),
                        format["height"].as_f64().unwrap(),
                    )
                }
                _ => {
                    let format = self.project["sequences"]
                        .as_object()
                        .unwrap()
                        .values()
                        .find(|s| {
                            array(&s["tracks"])
                                .iter()
                                .any(|tr| array(&tr["items"]).iter().any(|i| i["id"] == item["id"]))
                        })
                        .map(|s| &s["format"])
                        .context("effect owner sequence")?;
                    (
                        format["width"].as_f64().unwrap(),
                        format["height"].as_f64().unwrap(),
                    )
                }
            };
            pipeline = pipeline.source_scale(sw as f64 / ow, sh as f64 / oh);
        }
        // Identity geometry and a Rec.709 input need neither resampling nor
        // a decode/encode/quantize round trip before linear compositing.
        let mut f = if geometry.identity() && pipeline.passthrough() {
            linear(&raw, Space::Rec709)
        } else {
            let mut out = vec![0; frame_size(w, h)?];
            geometry.frame(&raw, Some(&pipeline), &mut out);
            linear(&out, Space::Rec709)
        };
        let opacity = evaluated["video"]["opacity"].as_f64().unwrap_or(1.);
        if opacity != 1. {
            for c in &mut f {
                for v in c {
                    *v *= opacity;
                }
            }
        }
        Ok(f)
    }
    fn sequence_frame(&mut self, sid: &str, t: &Time, left: bool, depth: usize) -> Result<Frame> {
        ensure!(depth < 64, "sequence depth exceeds 64");
        let seq = &self.project["sequences"][sid];
        let (w, h) = self.canvas(&seq["format"]);
        let mut background = rgba(&seq["background"], [0., 0., 0., 1.]);
        if background[3] > 0. {
            for i in 0..3 {
                background[i] = Space::Rec709.decode(background[i] / background[3]) * background[3];
            }
        }
        let mut frame = vec![background; w * h];
        let duration = project::sequence_duration(seq)?;
        if !active(t, &Time::zero(), &duration, left) {
            return Ok(vec![[0.; 4]; w * h]);
        }
        for kind in ["video", "subtitle"] {
            let solo = array(&seq["tracks"])
                .iter()
                .any(|tr| tr["kind"] == kind && enabled(tr) && tr["solo"] == true);
            for track in array(&seq["tracks"])
                .iter()
                .filter(|tr| tr["kind"] == kind && enabled(tr) && (!solo || tr["solo"] == true))
            {
                let mut consumed = vec![];
                let mut transition_frames = HashMap::new();
                for tr in array(&seq["transitions"]) {
                    let Some(a) = array(&track["items"])
                        .iter()
                        .find(|i| i["id"] == tr["fromItemId"])
                    else {
                        continue;
                    };
                    let b = array(&track["items"])
                        .iter()
                        .find(|i| i["id"] == tr["toItemId"])
                        .context("transition target")?;
                    let (ws, we) = anim::transition_window(tr, a)?;
                    if !active(t, &ws, &we, left) {
                        continue;
                    }
                    let fa = self.item_frame(
                        a,
                        &t.sub(&bounds(&a["timelineRange"])?.0),
                        left,
                        true,
                        w,
                        h,
                        None,
                        depth,
                    )?;
                    let fb = self.item_frame(
                        b,
                        &t.sub(&bounds(&b["timelineRange"])?.0),
                        left,
                        true,
                        w,
                        h,
                        None,
                        depth,
                    )?;
                    let p = if tr["progress"].is_null() {
                        anim::float(&t.sub(&ws).div(&we.sub(&ws))?)?
                    } else {
                        anim::evaluate(&tr["progress"], &t.sub(&ws))?
                            .as_f64()
                            .context("progress")?
                    };
                    let result = match text(&tr["transitionType"]) {
                        "builtin.crossfade" => blend(&fa, &fb, p),
                        "builtin.dip-to-color" => {
                            let k = vec![rgba(&tr["parameters"]["color"], [0., 0., 0., 1.]); w * h];
                            if p <= 0.5 {
                                blend(&fa, &k, 2. * p)
                            } else {
                                blend(&k, &fb, 2. * p - 1.)
                            }
                        }
                        other => bail!("PLUGIN_UNAVAILABLE: {other}"),
                    };
                    transition_frames.insert(text(&a["id"]), result);
                    consumed.push(text(&a["id"]));
                    consumed.push(text(&b["id"]));
                }
                for item in array(&track["items"]) {
                    if let Some(result) = transition_frames.remove(text(&item["id"])) {
                        over(&mut frame, &result);
                        continue;
                    }
                    let (start, end) = bounds(&item["timelineRange"])?;
                    if !enabled(item)
                        || consumed.contains(&text(&item["id"]))
                        || !active(t, &start, &end, left)
                    {
                        continue;
                    }
                    let f = self.item_frame(
                        item,
                        &t.sub(&start),
                        left,
                        false,
                        w,
                        h,
                        Some(&frame),
                        depth,
                    )?;
                    if item["type"] == "adjustment" {
                        let evaluated = anim::evaluated(item, &t.sub(&start))?;
                        let opacity = evaluated["video"]["opacity"].as_f64().unwrap_or(1.);
                        for (a, b) in frame.iter_mut().zip(f) {
                            for i in 0..4 {
                                a[i] = a[i] * (1. - opacity) + b[i];
                            }
                        }
                    } else {
                        over(&mut frame, &f);
                    }
                }
            }
        }
        Ok(frame)
    }
    /// Compile ordinary forward, unity-rate clips once per block. Exact rational
    /// anchors determine sample indices and fractional phase; the inner loop
    /// only interpolates PCM and mixes gains. Other mappings use the evaluator.
    fn audio_block(
        &mut self,
        sid: &str,
        start: &Time,
        count: usize,
        rate: i64,
        channels: usize,
    ) -> Result<Option<Vec<u8>>> {
        let seq = &self.project["sequences"][sid];
        let end = project::sequence_duration(seq)?;
        let solo = array(&seq["tracks"])
            .iter()
            .any(|tr| tr["kind"] == "audio" && enabled(tr) && tr["solo"] == true);
        let mut clips = Vec::new();
        for track in array(&seq["tracks"]).iter().filter(|tr| {
            tr["kind"] == "audio"
                && enabled(tr)
                && tr["muted"] != true
                && (!solo || tr["solo"] == true)
        }) {
            for item in array(&track["items"]) {
                if !enabled(item)
                    || item["audio"]["muted"] == true
                    || item["playback"]["audioPolicy"]["mode"] == "mute"
                {
                    continue;
                }
                let (a, b) = bounds(&item["timelineRange"])?;
                let first = a
                    .sub(start)
                    .mul(&Time::integer(rate))
                    .ceil_i64()?
                    .clamp(0, count as i64) as usize;
                let last = b
                    .min(end.clone())
                    .sub(start)
                    .mul(&Time::integer(rate))
                    .ceil_i64()?
                    .clamp(0, count as i64) as usize;
                if first >= last {
                    continue;
                }
                if item["type"] == "generator" && item["generatorType"] == "builtin.silence" {
                    continue;
                }
                if item["type"] != "media"
                    || item["playback"]["reverse"] == true
                    || matches!(text(&item["playback"]["mode"]), "remap" | "loop" | "hold")
                    || Time::from_json(&item["sourceRange"]["duration"])?
                        != Time::from_json(&item["timelineRange"]["duration"])?
                    || ["gainDb", "pan"]
                        .iter()
                        .any(|k| !item["audio"][k].is_null() && !item["audio"][k].is_number())
                    || ["fadeIn", "fadeOut"]
                        .iter()
                        .any(|k| item["audio"][k].is_object() && item["audio"][k]["num"] != 0)
                {
                    return Ok(None);
                }
                clips.push((item, a, first, last));
            }
        }
        let mut mixed = vec![0f64; count * channels];
        for (item, clip_start, mut offset, last) in clips {
            let gain = 10f64.powf(item["audio"]["gainDb"].as_f64().unwrap_or(0.) / 20.);
            let pan = item["audio"]["pan"].as_f64().unwrap_or(0.);
            let (source_start, source_end) = bounds(&item["sourceRange"])?;
            while offset < last {
                self.progress.check_cancelled()?;
                let src = source_start
                    .add(&start.sub(&clip_start))
                    .add(&Time::new(offset as i64, rate)?);
                let key = self.window_source(item, true, rate, channels, &src)?;
                let source = &self.sources[&key];
                let index = src.sub(&source.start).mul(&Time::integer(rate));
                let n = index
                    .0
                    .floor()
                    .to_integer()
                    .to_i64()
                    .context("audio index overflow")?;
                let frac = anim::float(&index.sub(&Time::integer(n)))?;
                ensure!(
                    n >= 0 && (n as usize) < source.samples,
                    "audio source sample outside decoded data"
                );
                let final_sample = source_end
                    .sub(&source.start)
                    .mul(&Time::integer(rate))
                    .ceil_i64()?
                    .saturating_sub(1);
                ensure!(final_sample >= n, "audio sample exceeds source window");
                // Keep one interpolation neighbor; at the true source end clamp.
                let available = source.samples.saturating_sub(n as usize + 1).max(1);
                let take = (last - offset)
                    .min(available)
                    .min((final_sample - n + 1) as usize);
                let pcm = source.pcm.as_ref().context("interactive PCM missing")?;
                for j in 0..take {
                    let a = n as usize + j;
                    let b = (a + 1).min(final_sample as usize).min(source.samples - 1);
                    for c in 0..channels {
                        let x = pcm[a * channels + c] as f64;
                        let y = pcm[b * channels + c] as f64;
                        let amplitude = gain
                            * if channels != 2 {
                                1.
                            } else if c == 0 {
                                (1. - pan).min(1.)
                            } else {
                                (1. + pan).min(1.)
                            };
                        mixed[(offset + j) * channels + c] += (x + (y - x) * frac) * amplitude;
                    }
                }
                offset += take;
            }
        }
        Ok(Some(
            mixed
                .into_iter()
                .flat_map(|v| (v as f32).to_le_bytes())
                .collect(),
        ))
    }
    fn audio_sample(
        &mut self,
        sid: &str,
        t: &Time,
        left: bool,
        rate: i64,
        channels: usize,
        depth: usize,
    ) -> Result<Vec<f64>> {
        ensure!(depth < 64, "sequence depth exceeds 64");
        let seq = &self.project["sequences"][sid];
        let mut out = vec![0.; channels];
        if !active(t, &Time::zero(), &project::sequence_duration(seq)?, left) {
            return Ok(out);
        }
        let solo = array(&seq["tracks"])
            .iter()
            .any(|tr| tr["kind"] == "audio" && enabled(tr) && tr["solo"] == true);
        for track in array(&seq["tracks"]).iter().filter(|tr| {
            tr["kind"] == "audio"
                && enabled(tr)
                && tr["muted"] != true
                && (!solo || tr["solo"] == true)
        }) {
            for item in array(&track["items"]) {
                let (start, end) = bounds(&item["timelineRange"])?;
                if !enabled(item)
                    || item["audio"]["muted"] == true
                    || item["playback"]["audioPolicy"]["mode"] == "mute"
                    || !active(t, &start, &end, left)
                {
                    continue;
                }
                if item["type"] == "generator" {
                    ensure!(
                        item["generatorType"] == "builtin.silence",
                        "unsupported audio generator"
                    );
                    continue;
                }
                let local = t.sub(&start);
                let (src, upper, frozen) = anim::mapping(item, &local, left)?;
                if frozen {
                    continue;
                }
                let mut sample = if item["type"] == "sequence" {
                    self.audio_sample(
                        text(&item["sequenceId"]),
                        &src,
                        upper,
                        rate,
                        channels,
                        depth + 1,
                    )?
                } else {
                    let key = if self.interactive {
                        self.window_source(item, true, rate, channels, &src)?
                    } else {
                        self.source(item, true, rate, channels)?
                    };
                    let s = self.sources.get_mut(&key).unwrap();
                    let index = src.sub(&s.start).mul(&Time::integer(s.rate));
                    let mut n = index
                        .0
                        .floor()
                        .to_integer()
                        .to_i64()
                        .context("audio index overflow")?;
                    let frac = if upper && index.0.is_integer() {
                        n -= 1;
                        0.
                    } else {
                        anim::float(&index.sub(&Time::integer(n)))?
                    };
                    ensure!(
                        n >= 0 && (n as usize) < s.samples,
                        "audio source sample outside decoded data"
                    );
                    let read = |s: &mut Source, n: usize| -> Result<Vec<f64>> {
                        if let Some(pcm) = &s.pcm {
                            return Ok(pcm[n * s.channels..(n + 1) * s.channels]
                                .iter()
                                .map(|s| *s as f64)
                                .collect());
                        }
                        s.file.seek(SeekFrom::Start((n * s.channels * 4) as u64))?;
                        let mut buf = vec![0; s.channels * 4];
                        s.file.read_exact(&mut buf)?;
                        Ok(buf
                            .chunks_exact(4)
                            .map(|v| f32::from_le_bytes(v.try_into().unwrap()) as f64)
                            .collect())
                    };
                    let a = read(s, n as usize)?;
                    let (_, end) = bounds(&item["sourceRange"])?;
                    let last = end
                        .sub(&s.start)
                        .mul(&Time::integer(s.rate))
                        .ceil_i64()?
                        .saturating_sub(1);
                    ensure!(last >= n, "audio sample exceeds source window");
                    let b = read(s, ((n + 1).min(last) as usize).min(s.samples - 1))?;
                    a.iter().zip(b).map(|(a, b)| a + (b - a) * frac).collect()
                };
                let gain = anim::evaluate(&item["audio"]["gainDb"], &local)?
                    .as_f64()
                    .unwrap_or(0.);
                let pan = anim::evaluate(&item["audio"]["pan"], &local)?
                    .as_f64()
                    .unwrap_or(0.);
                let mut amp = 10f64.powf(gain / 20.);
                for k in ["fadeIn", "fadeOut"] {
                    if let Some(v) = item["audio"].get(k) {
                        let d = Time::from_json(v)?;
                        if d > Time::zero() {
                            let x = if k == "fadeIn" {
                                local.clone()
                            } else {
                                end.sub(t)
                            };
                            amp *= anim::float(&x.div(&d)?)?.clamp(0., 1.);
                        }
                    }
                }
                for (c, v) in sample.iter_mut().enumerate() {
                    *v *= amp;
                    if channels == 2 {
                        *v *= if c == 0 {
                            (1. - pan).min(1.)
                        } else {
                            (1. + pan).min(1.)
                        };
                    }
                    out[c] += *v;
                }
            }
        }
        Ok(out)
    }
}
pub fn required(project: &Value) -> bool {
    if project["specVersion"] != "0.3.0" {
        return true;
    }
    project["sequences"].as_object().unwrap().values().any(|s| {
        !array(&s["transitions"]).is_empty()
            || array(&s["tracks"]).iter().any(|t| {
                array(&t["items"]).iter().any(|i| {
                    anim::animated(i)
                        || matches!(text(&i["playback"]["mode"]), "remap" | "hold" | "loop")
                        || !i["playback"]["videoSampling"].is_null()
                        || !i["playback"]["audioPolicy"].is_null()
                        || i["audio"]["pan"].as_f64().unwrap_or(0.) != 0.
                })
            })
    })
}
pub(crate) fn preflight(project: &Value) -> Result<()> {
    for seq in project["sequences"].as_object().unwrap().values() {
        ensure!(
            Time::from_json(&seq["format"]["pixelAspect"])? == Time::integer(1),
            "non-square output pixels unsupported"
        );
        ensure!(
            matches!(text(&seq["format"]["colorSpace"]), "" | "rec709"),
            "output color space unsupported"
        );
        for tr in array(&seq["transitions"]) {
            ensure!(
                matches!(
                    text(&tr["transitionType"]),
                    "builtin.crossfade" | "builtin.dip-to-color"
                ) && tr["version"] == "2.0.0",
                "transition processor/version unavailable"
            );
        }
        for track in array(&seq["tracks"]) {
            for item in array(&track["items"]) {
                ensure!(
                    track["kind"] != "audio" || item["type"] != "adjustment",
                    "BACKEND_UNSUPPORTED: audio adjustment"
                );
                if item["type"] == "media" && track["kind"] == "video" {
                    let stream = array(&project["assets"][text(&item["assetId"])]["streams"])
                        .iter()
                        .find(|s| s["id"] == item["streamId"])
                        .context("missing video stream")?;
                    Space::parse(text(&stream["video"]["colorSpace"]))?;
                }
                ensure!(
                    matches!(text(&item["video"]["blendMode"]), "" | "normal"),
                    "BACKEND_UNSUPPORTED: blend mode"
                );
                ensure!(
                    item["playback"]["videoSampling"]["method"] != "optical-flow",
                    "PROCESSOR_UNAVAILABLE: optical-flow"
                );
                let policy = &item["playback"]["audioPolicy"];
                if !policy.is_null() {
                    match text(&policy["mode"]){"mute"=>{},"resample"=>ensure!(policy["processor"]["id"]=="cutcraft.linear-resample"&&policy["processor"]["version"]=="1.0.0"&&(policy["processor"]["parameters"].is_null()||policy["processor"]["parameters"]==json!({})),"PROCESSOR_UNAVAILABLE: use cutcraft.linear-resample 1.0.0 with empty parameters"),_=>bail!("PROCESSOR_UNAVAILABLE: preserve-pitch")}
                }
            }
        }
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
pub(crate) fn render_with_progress(
    project: &Value,
    dir: &Path,
    sid: &str,
    output: &Path,
    overwrite: bool,
    progress: &crate::progress::Progress,
) -> Result<Value> {
    render_profile(project, dir, sid, output, overwrite, false, progress)
}
pub(crate) fn preview(
    project: &Value,
    dir: &Path,
    sid: &str,
    output: &Path,
    progress: &crate::progress::Progress,
) -> Result<Value> {
    render_profile(project, dir, sid, output, true, true, progress)
}

/// The interactive facade shares the reference evaluator and color pipeline.
pub(crate) struct PreviewEngine<'a> {
    engine: Engine<'a>,
    sid: &'a str,
    pub width: usize,
    pub height: usize,
    pub fps: Time,
    pub rate: i64,
    pub channels: usize,
    pub duration: Time,
}
impl<'a> PreviewEngine<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        project: &'a Value,
        dir: &'a Path,
        sid: &'a str,
        scratch: &'a Path,
        progress: &'a crate::progress::Progress,
    ) -> Result<Self> {
        Self::with_scale(project, dir, sid, scratch, progress, None)
    }
    pub(crate) fn with_scale(
        project: &'a Value,
        dir: &'a Path,
        sid: &'a str,
        scratch: &'a Path,
        progress: &'a crate::progress::Progress,
        scale: Option<f64>,
    ) -> Result<Self> {
        crate::preview::validate_scale(scale)?;
        project::validate(project)?;
        preflight(project)?;
        let seq = project["sequences"]
            .get(sid)
            .context("sequence not found")?;
        let format = &seq["format"];
        let (width, height) = scaled_dimensions(
            format["width"].as_u64().unwrap() as usize,
            format["height"].as_u64().unwrap() as usize,
            true,
            scale,
        );
        ensure!(width <= 4096 && height <= 4096 && width * height <= 4096 * 2304 && width.div_ceil(16) * height.div_ceil(16) <= 36864, "PREVIEW_RESOLUTION_TOO_LARGE: reduce scale (maximum 4096 on either axis and 9437184 pixels)");
        let mut text_scales = HashMap::new();
        for seq in project["sequences"].as_object().unwrap().values() {
            let (w, h) = scaled_dimensions(
                seq["format"]["width"].as_u64().unwrap() as usize,
                seq["format"]["height"].as_u64().unwrap() as usize,
                true,
                scale,
            );
            let scale = (w as f64 / seq["format"]["width"].as_f64().unwrap())
                .min(h as f64 / seq["format"]["height"].as_f64().unwrap());
            for track in array(&seq["tracks"]) {
                for item in array(&track["items"]) {
                    text_scales.insert(text(&item["id"]).to_owned(), scale);
                }
            }
        }
        Ok(Self {
            engine: Engine {
                project,
                dir,
                scratch,
                progress,
                preview: true,
                scale,
                text_scales,
                sources: HashMap::new(),
                image_cache: VecDeque::new(),
                interactive: true,
                native: HashMap::new(),
            },
            sid,
            width,
            height,
            fps: Time::from_json(&format["frameRate"])?.min(Time::integer(24)),
            rate: format["audioSampleRate"]
                .as_i64()
                .context("audio sample rate")?
                .min(24000),
            channels: if format["audioChannelLayout"] == "mono" {
                1
            } else {
                2
            },
            duration: project::sequence_duration(seq)?,
        })
    }
    pub(crate) fn video(&mut self, at: &Time, srgb: bool) -> Result<Vec<u8>> {
        self.engine.progress.check_cancelled()?;
        let frame = self.engine.sequence_frame(self.sid, at, false, 0)?;
        if !srgb {
            return Ok(bytes(&frame));
        }
        let mut out = Vec::with_capacity(frame.len() * 8);
        for c in frame {
            for i in 0..4 {
                let value = if i == 3 {
                    c[3]
                } else if c[3] > 0. {
                    Space::Srgb.encode(c[i] / c[3])
                } else {
                    0.
                };
                out.extend_from_slice(
                    &((value.clamp(0., 1.) * 65535.).round() as u16).to_le_bytes(),
                );
            }
        }
        Ok(out)
    }
    pub(crate) fn audio_at(&mut self, start: &Time, count: usize) -> Result<Vec<u8>> {
        if self.engine.interactive {
            if let Some(out) =
                self.engine
                    .audio_block(self.sid, start, count, self.rate, self.channels)?
            {
                return Ok(out);
            }
        }
        let mut out = Vec::with_capacity(count * self.channels * 4);
        for n in 0..count as i64 {
            if n % 2048 == 0 {
                self.engine.progress.check_cancelled()?;
            }
            for value in self.engine.audio_sample(
                self.sid,
                &start.add(&Time::new(n, self.rate)?),
                false,
                self.rate,
                self.channels,
                0,
            )? {
                out.extend_from_slice(&(value as f32).to_le_bytes());
            }
        }
        Ok(out)
    }
}
fn render_profile(
    project: &Value,
    dir: &Path,
    sid: &str,
    output: &Path,
    overwrite: bool,
    preview: bool,
    progress: &crate::progress::Progress,
) -> Result<Value> {
    progress.start("validate", None, "assets");
    media::validate(project, dir)?;
    preflight(project)?;
    let seq = project["sequences"]
        .get(sid)
        .context("sequence not found")?;
    let format = &seq["format"];
    let (w, h) = canvas(format, preview);
    frame_size(w, h)?;
    ensure!(w % 2 == 0 && h % 2 == 0, "H264 requires even dimensions");
    let original_fps = Time::from_json(&format["frameRate"])?;
    let fps = if preview {
        original_fps.min(Time::integer(24))
    } else {
        original_fps
    };
    let duration = project::sequence_duration(seq)?;
    ensure!(duration > Time::zero(), "EMPTY_SEQUENCE");
    let frames = duration.mul(&fps).ceil_i64()?;
    let rate = format["audioSampleRate"].as_i64().unwrap();
    let rate = if preview { rate.min(24000) } else { rate };
    let channels = match text(&format["audioChannelLayout"]) {
        "mono" => 1,
        "stereo" => 2,
        _ => bail!("unsupported audio layout"),
    };
    let output: PathBuf = if output.is_absolute() {
        output.into()
    } else {
        std::env::current_dir()?.join(output)
    };
    ensure!(
        output.extension().and_then(|s| s.to_str()) == Some("mp4"),
        "output must be .mp4"
    );
    ensure!(
        overwrite || !output.exists(),
        "output exists; use --overwrite"
    );
    if output.exists() {
        for asset in project["assets"].as_object().unwrap().values() {
            if crate::image_sequence::is_sequence(asset) {
                for frame in array(&asset["source"]["frames"]) {
                    ensure!(
                        output.canonicalize()?
                            != crate::image_sequence::frame_path(dir, asset, frame)?
                                .canonicalize()?,
                        "OUTPUT_IS_SOURCE"
                    );
                }
                continue;
            }
            ensure!(
                output.canonicalize()?
                    != media::local_path(dir, text(&asset["uri"]))?.canonicalize()?,
                "OUTPUT_IS_SOURCE"
            );
        }
    }
    let parent = output.parent().context("output parent")?;
    fs::create_dir_all(parent)?;
    let scratch = tempfile::tempdir_in(parent)?;
    let mut text_scales = HashMap::new();
    if preview {
        for seq in project["sequences"].as_object().unwrap().values() {
            let (w, h) = canvas(&seq["format"], true);
            let scale = (w as f64 / seq["format"]["width"].as_f64().unwrap())
                .min(h as f64 / seq["format"]["height"].as_f64().unwrap());
            for track in array(&seq["tracks"]) {
                for item in array(&track["items"]) {
                    text_scales.insert(text(&item["id"]).to_string(), scale);
                }
            }
        }
    }
    let mut engine = Engine {
        progress,
        project,
        preview,
        scale: None,
        text_scales,
        dir,
        scratch: scratch.path(),
        sources: HashMap::new(),
        image_cache: VecDeque::new(),
        interactive: false,
        native: HashMap::new(),
    };
    let video = scratch.path().join("video.rgba");
    let mut writer = BufWriter::new(File::create(&video)?);
    progress.start("video", Some(frames as u64), "frames");
    for n in 0..frames {
        progress.check_cancelled()?;
        let t = Time::integer(n).div(&fps)?;
        let frame = engine.sequence_frame(sid, &t, false, 0)?;
        writer.write_all(&bytes(&frame))?;
        progress.advance((n + 1) as u64);
    }
    writer.flush()?;
    drop(writer);
    let audio = scratch.path().join("audio.f32");
    let mut writer = BufWriter::new(File::create(&audio)?);
    let samples = duration.mul(&Time::integer(rate)).ceil_i64()?;
    let has_audio = project["sequences"].as_object().unwrap().values().any(|s| {
        array(&s["tracks"])
            .iter()
            .any(|t| t["kind"] == "audio" && enabled(t) && !array(&t["items"]).is_empty())
    });
    progress.start("audio", Some(samples as u64), "samples");
    for n in 0..samples {
        if n % 2048 == 0 {
            progress.check_cancelled()?;
        }
        let sample = if has_audio {
            engine.audio_sample(sid, &Time::new(n, rate)?, false, rate, channels, 0)?
        } else {
            vec![0.; channels]
        };
        for s in sample {
            writer.write_all(&(s as f32).to_le_bytes())?;
        }
        if n % 2048 == 0 || n + 1 == samples {
            progress.advance((n + 1) as u64);
        }
    }
    writer.flush()?;
    drop(writer);
    let rendered = scratch.path().join("render.mp4");
    progress.ffmpeg(Command::new("ffmpeg").args(["-nostdin","-v","error","-y","-threads","2","-f","rawvideo","-pixel_format","rgba64le","-video_size",&format!("{w}x{h}"),"-framerate",&fps.expression(),"-i"]).arg(video).args(["-f","f32le","-ar",&rate.to_string(),"-ac",&channels.to_string(),"-i"]).arg(audio).args(["-filter_threads","2","-vf","scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709","-c:v","libx264","-threads","2","-preset",if preview{"ultrafast"}else{"fast"},"-crf",if preview{"28"}else{"18"},"-c:a","aac","-movflags","+faststart"]).arg(&rendered), "encode", Some(frames as u64))?;
    progress.start("finalize", None, "files");
    progress.check_cancelled()?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut File::open(rendered)?, &mut staged)?;
    staged.as_file().sync_all()?;
    if overwrite {
        staged.persist(&output).map_err(|e| e.error)?;
    } else {
        staged.persist_noclobber(&output).map_err(|e| e.error)?;
    }
    Ok(
        json!({"ok":true,"output":output,"sequenceId":sid,"revision":project["revision"],"frames":frames,"duration":duration.json()?,"backend":"cutcraft-reference-0.5","width":w,"height":h,"frameRate":fps.json()?,"preview":preview}),
    )
}

#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::{id, progress::Progress};
    #[test]
    #[ignore = "manual debug preview profiling; set CUTCRAFT_PROFILE_PROJECT"]
    fn profile_preview() {
        let dir = std::path::PathBuf::from(std::env::var("CUTCRAFT_PROFILE_PROJECT").unwrap());
        let p: Value =
            serde_json::from_slice(&fs::read(dir.join("cutcraft.json")).unwrap()).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let scale = std::env::var("CUTCRAFT_PROFILE_SCALE")
            .ok()
            .map(|s| s.parse().unwrap());
        let mut e = PreviewEngine::with_scale(
            &p,
            &dir,
            text(&p["rootSequenceId"]),
            scratch.path(),
            &progress,
            scale,
        )
        .unwrap();
        for pass in 0..2 {
            let now = std::time::Instant::now();
            for n in 0..12 {
                e.video(&Time::new(n, 24).unwrap(), false).unwrap();
            }
            eprintln!("pass {pass}: 12 video frames {:?}", now.elapsed());
            let now = std::time::Instant::now();
            e.audio_at(&Time::zero(), 12000).unwrap();
            eprintln!("pass {pass}: 0.5s audio {:?}", now.elapsed());
        }
    }
    fn fixture(dir: &Path, native_rate: i64) -> (Value, String) {
        let source = dir.join("source.mp4");
        let result = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=12:duration=12",
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=427:sample_rate={native_rate}:duration=12"),
                "-vf",
                "select=not(eq(mod(n\\,3)\\,1))",
                "-fps_mode",
                "vfr",
                "-c:v",
                "libx264",
                "-g",
                "12",
                "-bf",
                "2",
                "-c:a",
                "aac",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut p = project::new("window");
        let sid = text(&p["rootSequenceId"]).to_owned();
        let aid = id("asset");
        p["assets"][&aid] = media::asset(&source, "source.mp4", false).unwrap();
        p["assets"][&aid]["streams"][0]["video"]["colorSpace"] = json!("rec709");
        let sequence = &mut p["sequences"][&sid];
        sequence["format"]["width"] = json!(64);
        sequence["format"]["height"] = json!(48);
        sequence["format"]["frameRate"] = Time::integer(12).json().unwrap();
        let range = json!({"start":{"num":0,"den":1},"duration":{"num":11,"den":1}});
        sequence["tracks"] = json!([
            {"id":id("track"),"kind":"video","name":"Video","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"media","assetId":aid,"streamId":"stream_v0","timelineRange":range,"sourceRange":range}]},
            {"id":id("track"),"kind":"audio","name":"Audio","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"media","assetId":aid,"streamId":"stream_a1","timelineRange":range,"sourceRange":range}]}
        ]);
        (p, sid)
    }
    #[test]
    fn block_audio_matches_sample_evaluator_across_clip_and_window_boundaries() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            assert!(std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut p, sid) = fixture(dir.path(), 44100);
        let clip = &mut p["sequences"][&sid]["tracks"][1]["items"][0];
        clip["timelineRange"]["start"] = Time::new(1, 23).unwrap().json().unwrap();
        clip["sourceRange"]["start"] = Time::new(1, 7).unwrap().json().unwrap();
        clip["audio"] = json!({"gainDb":-6.,"pan":0.3});
        let mut track = p["sequences"][&sid]["tracks"][1].clone();
        track["id"] = json!(id("track"));
        track["items"][0]["id"] = json!(id("item"));
        track["items"][0]["audio"] = json!({"gainDb":-3.,"pan":-0.7});
        p["sequences"][&sid]["tracks"]
            .as_array_mut()
            .unwrap()
            .push(track);
        let scratch = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let mut fast = PreviewEngine::new(&p, dir.path(), &sid, scratch.path(), &progress).unwrap();
        let mut slow = PreviewEngine::new(&p, dir.path(), &sid, other.path(), &progress).unwrap();
        slow.engine.interactive = false;
        for (start, count) in [
            (Time::zero(), 1500),
            (Time::new(3, 2).unwrap(), 14000),
            (Time::integer(11), 1500),
        ] {
            let a = fast.audio_at(&start, count).unwrap();
            let b = slow.audio_at(&start, count).unwrap();
            let difference = a
                .chunks_exact(4)
                .zip(b.chunks_exact(4))
                .map(|(a, b)| {
                    (f32::from_le_bytes(a.try_into().unwrap())
                        - f32::from_le_bytes(b.try_into().unwrap()))
                    .abs()
                })
                .fold(0f32, f32::max);
            assert!(
                difference < 0.0001,
                "block differs at {start}: {difference}"
            );
        }
        // Reverse/fades remain on the full evaluator; no silent approximation.
        drop(fast);
        for change in [
            json!({"playback":{"mode":"fit","reverse":true,"audioPolicy":{"mode":"resample","processor":{"id":"cutcraft.linear-resample","version":"1.0.0","parameters":{}}}}}),
            json!({"audio":{"fadeIn":{"num":1,"den":1}}}),
        ] {
            let mut changed = p.clone();
            for (k, v) in change.as_object().unwrap() {
                changed["sequences"][&sid]["tracks"][1]["items"][0][k] = v.clone();
            }
            let mut e =
                PreviewEngine::new(&changed, dir.path(), &sid, scratch.path(), &progress).unwrap();
            assert!(e
                .engine
                .audio_block(&sid, &Time::integer(1), 100, e.rate, e.channels)
                .unwrap()
                .is_none());
        }
    }
    #[test]
    fn windows_match_vfr_reference_frames_and_pcm() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            assert!(std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
            return;
        }
        for native_rate in [44100, 48000] {
            let dir = tempfile::tempdir().unwrap();
            let (p, sid) = fixture(dir.path(), native_rate);
            let original = tempfile::tempdir().unwrap();
            let window = tempfile::tempdir().unwrap();
            let progress = Progress::default();
            let mut reference =
                PreviewEngine::new(&p, dir.path(), &sid, original.path(), &progress).unwrap();
            reference.engine.interactive = false;
            let mut interactive =
                PreviewEngine::new(&p, dir.path(), &sid, window.path(), &progress).unwrap();
            for at in [
                Time::new(7, 1).unwrap(),
                Time::new(91, 12).unwrap(),
                Time::new(19, 3).unwrap(),
            ] {
                assert_eq!(
                    interactive.video(&at, false).unwrap(),
                    reference.video(&at, false).unwrap(),
                    "frame {at}"
                );
            }
            for at in [
                Time::zero(),
                Time::integer(7),
                Time::new(162, 23).unwrap(),
                Time::integer(9),
            ] {
                let a = interactive.audio_at(&at, 512).unwrap();
                let b = reference.audio_at(&at, 512).unwrap();
                let max = a
                    .chunks_exact(4)
                    .zip(b.chunks_exact(4))
                    .map(|(a, b)| {
                        (f32::from_le_bytes(a.try_into().unwrap())
                            - f32::from_le_bytes(b.try_into().unwrap()))
                        .abs()
                    })
                    .fold(0f32, f32::max);
                assert!(
                    max < 0.0001,
                    "PCM window differs at {at} ({native_rate}Hz): {max}"
                );
            }
            // A new request/Engine must reuse the same source window artifacts.
            let before = fs::read_dir(dir.path().join("cache/decode-windows-v1"))
                .unwrap()
                .count();
            let fresh = tempfile::tempdir().unwrap();
            let mut another =
                PreviewEngine::new(&p, dir.path(), &sid, fresh.path(), &progress).unwrap();
            another.video(&Time::integer(7), false).unwrap();
            let after = fs::read_dir(dir.path().join("cache/decode-windows-v1"))
                .unwrap()
                .count();
            assert_eq!(
                before, after,
                "new seek repeated source decode instead of sharing cache"
            );
            let windows = fs::read_dir(dir.path().join("cache/decode-windows-v1"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|e| e == "raw"))
                .collect::<Vec<_>>();
            assert!(!windows.is_empty());
            for file in windows {
                assert!(
                    fs::metadata(file).unwrap().len() < 64 * 48 * 8 * 12 * 6,
                    "decoded full source instead of a window"
                );
            }
        }
    }
}
