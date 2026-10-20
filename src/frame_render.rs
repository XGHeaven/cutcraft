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
};
type Frame = Vec<[f64; 4]>; // linear-sRGB, premultiplied alpha
struct Source {
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
    text_scales: HashMap<String, f64>,
    dir: &'a Path,
    scratch: &'a Path,
    sources: HashMap<String, Source>,
    // Bounded per-render cache; keys fix file bytes, interpretation and dimensions.
    image_cache: VecDeque<(String, Vec<u8>)>,
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
    raw.chunks_exact(8)
        .map(|b| {
            let c: [f64; 4] = std::array::from_fn(|i| {
                u16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as f64 / 65535.
            });
            [
                space.decode(c[0]) * c[3],
                space.decode(c[1]) * c[3],
                space.decode(c[2]) * c[3],
                c[3],
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
            let (dw, dh) = dimensions(w, h, self.preview);
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
            let (w, h) = dimensions(ow, oh, self.preview);
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
        let key = self.source(item, false, 0, 0)?;
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
                let (cw, ch) = canvas(&child["format"], self.preview);
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
        let pipeline = visual::Pipeline::new(&evaluated, self.project, self.dir, space)?;
        let mut out = vec![0; frame_size(w, h)?];
        geometry.frame(&raw, Some(&pipeline), &mut out);
        let mut f = linear(&out, Space::Rec709);
        let opacity = evaluated["video"]["opacity"].as_f64().unwrap_or(1.);
        for c in &mut f {
            for v in c {
                *v *= opacity;
            }
        }
        Ok(f)
    }
    fn sequence_frame(&mut self, sid: &str, t: &Time, left: bool, depth: usize) -> Result<Frame> {
        ensure!(depth < 64, "sequence depth exceeds 64");
        let seq = &self.project["sequences"][sid];
        let (w, h) = canvas(&seq["format"], self.preview);
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
                    let key = self.source(item, true, rate, channels)?;
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
        text_scales,
        dir,
        scratch: scratch.path(),
        sources: HashMap::new(),
        image_cache: VecDeque::new(),
    };
    let video = scratch.path().join("video.rgba");
    let mut writer = BufWriter::new(File::create(&video)?);
    progress.start("video", Some(frames as u64), "frames");
    for n in 0..frames {
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
