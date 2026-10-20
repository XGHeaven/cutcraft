//! Deterministic CPU color/geometry stage. FFmpeg handles codecs and timing;
//! effect math operates in linear-sRGB with straight alpha at f64 precision.
use crate::media_process::MediaCommand;
use crate::{
    media,
    project::{array, enabled, text},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// Draw an opaque white coverage mask first, then assign straight RGB and scale
/// alpha once. drawtext's blending onto transparent RGBA otherwise darkens edges
/// and applies translucent font alpha twice.
pub(crate) fn text_paint(color: &Value) -> Result<String> {
    let component = |key: &str| -> Result<f64> {
        let v = if color.is_null() {
            1.
        } else {
            color[key].as_f64().context("invalid text color")?
        };
        ensure!((0.0..=1.0).contains(&v), "invalid text color");
        Ok(v)
    };
    Ok(format!(
        "lutrgb=r={}:g={}:b={}:a=val*{}",
        component("r")? * 255.,
        component("g")? * 255.,
        component("b")? * 255.,
        component("a")?
    ))
}

#[derive(Clone, Debug)]
pub struct Cube {
    pub size: usize,
    pub three_d: bool,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub data: Vec<[f64; 3]>,
}
impl Cube {
    pub fn read(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).context("CUBE_INVALID: expected UTF-8 .cube")?)
    }
    pub fn parse(input: &str) -> Result<Self> {
        let mut cube = Self {
            size: 0,
            three_d: false,
            min: [0.; 3],
            max: [1.; 3],
            data: vec![],
        };
        let (mut title, mut min, mut max) = (false, false, false);
        for (line_no, line) in input.trim_start_matches('\u{feff}').lines().enumerate() {
            // A '#' inside a quoted TITLE is literal, outside it starts a comment.
            let mut quoted = false;
            let end = line
                .char_indices()
                .find_map(|(i, c)| {
                    if c == '"' {
                        quoted = !quoted;
                    }
                    (c == '#' && !quoted).then_some(i)
                })
                .unwrap_or(line.len());
            let line = line[..end].trim();
            if line.is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            let header = matches!(
                fields[0],
                "TITLE" | "LUT_1D_SIZE" | "LUT_3D_SIZE" | "DOMAIN_MIN" | "DOMAIN_MAX"
            );
            ensure!(
                !header || cube.data.is_empty(),
                "CUBE_INVALID line {}: header after data",
                line_no + 1
            );
            match fields[0] {
                "TITLE" => {
                    let value = line[5..].trim();
                    ensure!(
                        !title
                            && value.starts_with('"')
                            && value.ends_with('"')
                            && value.len() >= 2
                            && !value[1..value.len() - 1].contains('"'),
                        "CUBE_INVALID: TITLE"
                    );
                    title = true;
                }
                "LUT_1D_SIZE" | "LUT_3D_SIZE" => {
                    ensure!(
                        cube.size == 0 && fields.len() == 2,
                        "CUBE_INVALID: duplicate/combined LUT size"
                    );
                    cube.size = fields[1].parse().context("CUBE_INVALID: size")?;
                    cube.three_d = fields[0] == "LUT_3D_SIZE";
                    ensure!(
                        (2..=if cube.three_d { 256 } else { 65536 }).contains(&cube.size),
                        "CUBE_INVALID: size out of range"
                    );
                }
                "DOMAIN_MIN" | "DOMAIN_MAX" => {
                    ensure!(fields.len() == 4, "CUBE_INVALID: domain");
                    let values = triple(&fields[1..])?;
                    if fields[0] == "DOMAIN_MIN" {
                        ensure!(!min, "CUBE_INVALID: duplicate DOMAIN_MIN");
                        cube.min = values;
                        min = true;
                    } else {
                        ensure!(!max, "CUBE_INVALID: duplicate DOMAIN_MAX");
                        cube.max = values;
                        max = true;
                    }
                }
                _ => {
                    ensure!(
                        cube.size > 0,
                        "CUBE_INVALID: data before size or unknown directive"
                    );
                    let expected = if cube.three_d {
                        cube.size.pow(3)
                    } else {
                        cube.size
                    };
                    ensure!(cube.data.len() < expected, "CUBE_INVALID: excess rows");
                    cube.data.push(triple(&fields)?);
                }
            }
        }
        ensure!(
            cube.size > 0
                && cube.data.len()
                    == if cube.three_d {
                        cube.size.pow(3)
                    } else {
                        cube.size
                    },
            "CUBE_INVALID: missing size/data rows"
        );
        ensure!(
            (0..3).all(|i| cube.max[i] > cube.min[i] && (cube.max[i] - cube.min[i]).is_finite()),
            "CUBE_INVALID: invalid domain"
        );
        Ok(cube)
    }
    pub fn check_interpolation(&self, method: &str) -> Result<()> {
        ensure!(
            if self.three_d {
                matches!(method, "trilinear" | "tetrahedral")
            } else {
                method == "linear"
            },
            "LUT_INTERPOLATION_MISMATCH"
        );
        Ok(())
    }
    pub fn sample(&self, rgb: [f64; 3], tetrahedral: bool) -> [f64; 3] {
        let mut low = [0; 3];
        let mut f = [0.; 3];
        for i in 0..3 {
            let u = ((rgb[i] - self.min[i]) / (self.max[i] - self.min[i])).clamp(0., 1.)
                * (self.size - 1) as f64;
            low[i] = (u.floor() as usize).min(self.size - 2);
            f[i] = u - low[i] as f64;
        }
        if !self.three_d {
            return std::array::from_fn(|i| {
                self.data[low[i]][i] * (1. - f[i]) + self.data[low[i] + 1][i] * f[i]
            });
        }
        let get = |d: [usize; 3]| {
            self.data[(low[0] + d[0]) + self.size * ((low[1] + d[1]) + self.size * (low[2] + d[2]))]
        };
        let mut out = [0.; 3];
        if tetrahedral {
            let mut axes = [0, 1, 2];
            axes.sort_by(|a, b| f[*b].total_cmp(&f[*a]).then(a.cmp(b)));
            let weights = [
                1. - f[axes[0]],
                f[axes[0]] - f[axes[1]],
                f[axes[1]] - f[axes[2]],
                f[axes[2]],
            ];
            let mut d = [0; 3];
            for step in 0..4 {
                let v = get(d);
                for i in 0..3 {
                    out[i] += weights[step] * v[i];
                }
                if step < 3 {
                    d[axes[step]] = 1;
                }
            }
        } else {
            for r in 0..2 {
                for g in 0..2 {
                    for b in 0..2 {
                        let d = [r, g, b];
                        let weight = (0..3)
                            .map(|i| if d[i] == 0 { 1. - f[i] } else { f[i] })
                            .product::<f64>();
                        let v = get(d);
                        for i in 0..3 {
                            out[i] += weight * v[i];
                        }
                    }
                }
            }
        }
        out
    }
}
fn triple(fields: &[&str]) -> Result<[f64; 3]> {
    ensure!(
        fields.len() == 3,
        "CUBE_INVALID: expected three finite decimal values"
    );
    let mut values = [0.0_f64; 3];
    for i in 0..3 {
        values[i] = fields[i]
            .parse()
            .context("CUBE_INVALID: number/unknown directive")?;
        ensure!(values[i].is_finite(), "CUBE_INVALID: non-finite value");
    }
    Ok(values)
}
#[derive(Clone, Copy, Debug)]
pub enum Space {
    Linear,
    Srgb,
    Rec709,
}
impl Space {
    pub fn parse(s: &str) -> Result<Self> {
        match s {"linear-srgb"=>Ok(Self::Linear),"srgb"=>Ok(Self::Srgb),"rec709"=>Ok(Self::Rec709),_=>bail!("COLOR_SPACE_REQUIRED: unsupported or unknown color space {s:?}; import with --color-space")}
    }
    pub fn decode(self, x: f64) -> f64 {
        match self {
            Self::Linear => x,
            Self::Srgb => {
                if x <= 0.04045 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                }
            }
            Self::Rec709 => {
                if x < 0.081 {
                    x / 4.5
                } else {
                    ((x + 0.099) / 1.099).powf(1. / 0.45)
                }
            }
        }
    }
    pub fn encode(self, x: f64) -> f64 {
        match self {
            Self::Linear => x,
            Self::Srgb => {
                if x <= 0.0031308 {
                    12.92 * x
                } else {
                    1.055 * x.powf(1. / 2.4) - 0.055
                }
            }
            Self::Rec709 => {
                if x < 0.018 {
                    4.5 * x
                } else {
                    1.099 * x.powf(0.45) - 0.099
                }
            }
        }
    }
}
#[derive(Clone)]
enum Effect {
    Grade {
        gain: [f64; 3],
        contrast: f64,
        pivot: f64,
        saturation: f64,
    },
    Curves([Vec<[f64; 2]>; 4]),
    Lut {
        cube: Cube,
        strength: f64,
        input: Space,
        output: Space,
        tetrahedral: bool,
    },
}
pub struct Pipeline {
    effects: Vec<Effect>,
    input: Space,
}
pub fn has_effects(item: &Value) -> bool {
    array(&item["effects"]).iter().any(enabled)
}
pub fn needs_stage(item: &Value) -> bool {
    has_effects(item)
        || (!item["video"]["transform"].is_null()
            && item["video"]["transform"] != serde_json::json!({}))
}
pub fn number(v: &Value, key: &str, default: f64) -> f64 {
    v[key].as_f64().unwrap_or(default)
}
impl Pipeline {
    pub fn new(item: &Value, project: &Value, dir: &Path, input: Space) -> Result<Self> {
        ensure!(
            !crate::animation::animated(item),
            "ANIMATION_NOT_EVALUATED: evaluate registered slots before compiling visual pipeline"
        );
        let mut effects = vec![];
        for e in array(&item["effects"]).iter().filter(|e| enabled(e)) {
            ensure!(
                matches!(text(&e["version"]), "1.0.0" | "2.0.0")
                    && array(&e["automation"]).is_empty(),
                "EFFECT_UNSUPPORTED: version/automation"
            );
            let p = &e["parameters"];
            effects.push(match text(&e["effectType"]) {
                "builtin.color-grade" => {
                    let exposure = number(p, "exposureEv", 0.);
                    let temp = number(p, "temperature", 0.);
                    let tint = number(p, "tint", 0.);
                    Effect::Grade {
                        gain: [
                            (exposure + temp).exp2(),
                            (exposure + tint).exp2(),
                            (exposure - temp).exp2(),
                        ],
                        contrast: number(p, "contrast", 1.),
                        pivot: number(p, "pivot", 0.18),
                        saturation: number(p, "saturation", 1.),
                    }
                }
                "builtin.curves" => Effect::Curves(["rgb", "r", "g", "b"].map(|key| {
                    array(&p[key])
                        .iter()
                        .map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap()])
                        .collect()
                })),
                "builtin.lut" => {
                    let asset = &project["assets"][text(&p["assetId"])];
                    let path = media::local_path(dir, text(&asset["uri"]))?;
                    let fingerprint = text(&asset["fingerprint"]["value"]);
                    let bytes = fs::read(&path)?;
                    ensure!(
                        !fingerprint.is_empty()
                            && format!("{:x}", Sha256::digest(&bytes))
                                .eq_ignore_ascii_case(fingerprint),
                        "LUT_FINGERPRINT_MISMATCH_OR_MISSING"
                    );
                    let cube = Cube::parse(
                        std::str::from_utf8(&bytes).context("CUBE_INVALID: UTF-8 required")?,
                    )?;
                    cube.check_interpolation(text(&p["interpolation"]))?;
                    Effect::Lut {
                        cube,
                        strength: number(p, "strength", 1.),
                        input: Space::parse(text(&p["inputColorSpace"]))?,
                        output: Space::parse(text(&p["outputColorSpace"]))?,
                        tetrahedral: p["interpolation"] == "tetrahedral",
                    }
                }
                other => bail!("PLUGIN_UNAVAILABLE: {other}"),
            });
        }
        Ok(Self { effects, input })
    }
    pub fn pixel(&self, mut rgb: [f64; 3]) -> [f64; 3] {
        rgb = rgb.map(|v| self.input.decode(v));
        for e in &self.effects {
            match e {
                Effect::Grade {
                    gain,
                    contrast,
                    pivot,
                    saturation,
                } => {
                    for i in 0..3 {
                        rgb[i] = (rgb[i] * gain[i] - pivot) * contrast + pivot;
                    }
                    let y = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
                    rgb = rgb.map(|c| (y + (c - y) * saturation).clamp(0., 1.));
                }
                Effect::Curves(curves) => {
                    rgb = std::array::from_fn(|i| {
                        curve(&curves[i + 1], curve(&curves[0], rgb[i].clamp(0., 1.)))
                    });
                }
                Effect::Lut {
                    cube,
                    strength,
                    input,
                    output,
                    tetrahedral,
                } => {
                    let look = cube
                        .sample(rgb.map(|c| input.encode(c)), *tetrahedral)
                        .map(|c| output.decode(c));
                    rgb = std::array::from_fn(|i| (1. - strength) * rgb[i] + strength * look[i]);
                }
            }
        }
        rgb.map(|c| Space::Rec709.encode(c).clamp(0., 1.))
    }
}
fn curve(points: &[[f64; 2]], x: f64) -> f64 {
    if points.is_empty() {
        return x;
    }
    let i = points
        .partition_point(|p| p[0] <= x)
        .clamp(1, points.len() - 1);
    let (a, b) = (points[i - 1], points[i]);
    let t = (x - a[0]) / (b[0] - a[0]);
    a[1] + t * (b[1] - a[1])
}

/// Inverse affine map, evaluated at pixel centers. Bilinear interpolation uses
/// premultiplied RGBA to avoid dark fringes around transparent material.
pub struct Geometry {
    pub sw: usize,
    pub sh: usize,
    pub w: usize,
    pub h: usize,
    matrix: [f64; 6],
    crop: [f64; 4],
    empty: bool,
}
impl Geometry {
    pub fn new(sw: usize, sh: usize, sar: f64, w: usize, h: usize, t: &Value) -> Result<Self> {
        let left = number(&t["crop"], "left", 0.) * sw as f64;
        let top = number(&t["crop"], "top", 0.) * sh as f64;
        let right = (1. - number(&t["crop"], "right", 0.)) * sw as f64;
        let bottom = (1. - number(&t["crop"], "bottom", 0.)) * sh as f64;
        ensure!(
            right > left && bottom > top,
            "TRANSFORM_INVALID: crop removes entire image"
        );
        let cw = (right - left) * sar;
        let ch = bottom - top;
        let fit = (w as f64 / cw).min(h as f64 / ch);
        let sx = number(&t["scale"], "x", 1.) * fit;
        let sy = number(&t["scale"], "y", 1.) * fit;
        let angle = number(t, "rotationDegrees", 0.)
            .rem_euclid(360.)
            .to_radians();
        let (sin, cos) = angle.sin_cos();
        let px = number(&t["position"], "x", 0.5) * w as f64;
        let py = number(&t["position"], "y", 0.5) * h as f64;
        let ax = number(&t["anchor"], "x", 0.5) * cw;
        let ay = number(&t["anchor"], "y", 0.5) * ch;
        let empty = sx == 0. || sy == 0.;
        let matrix = if empty {
            [0.; 6]
        } else {
            [
                cos / (sx * sar),
                sin / (sx * sar),
                (-cos * px - sin * py) / sx / sar + ax / sar + left,
                -sin / sy,
                cos / sy,
                (sin * px - cos * py) / sy + ay + top,
            ]
        };
        ensure!(
            matrix.iter().all(|v| v.is_finite()) && sw > 0 && sh > 0 && w > 0 && h > 0,
            "TRANSFORM_INVALID: non-finite/overflow geometry"
        );
        Ok(Self {
            sw,
            sh,
            w,
            h,
            matrix,
            crop: [left, top, right, bottom],
            empty,
        })
    }
    pub fn frame(&self, source: &[u8], pipeline: Option<&Pipeline>, output: &mut [u8]) {
        output.fill(0);
        if self.empty {
            return;
        }
        let mut pixels = vec![[0.; 4]; self.sw * self.sh];
        for (pixel, bytes) in pixels.iter_mut().zip(source.chunks_exact(8)) {
            for i in 0..4 {
                pixel[i] = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]) as f64 / 65535.;
            }
            if let Some(p) = pipeline {
                let rgb = p.pixel([pixel[0], pixel[1], pixel[2]]);
                pixel[..3].copy_from_slice(&rgb);
            }
            for i in 0..3 {
                pixel[i] *= pixel[3];
            }
        }
        let m = self.matrix;
        for y in 0..self.h {
            for x in 0..self.w {
                let sx = m[0] * (x as f64 + 0.5) + m[1] * (y as f64 + 0.5) + m[2];
                let sy = m[3] * (x as f64 + 0.5) + m[4] * (y as f64 + 0.5) + m[5];
                if sx < self.crop[0]
                    || sx >= self.crop[2]
                    || sy < self.crop[1]
                    || sy >= self.crop[3]
                {
                    continue;
                }
                let ix = (sx - 0.5).floor();
                let iy = (sy - 0.5).floor();
                let fx = sx - 0.5 - ix;
                let fy = sy - 0.5 - iy;
                let mut rgba = [0.; 4];
                for dy in 0..2 {
                    for dx in 0..2 {
                        // Clamp reconstruction to the retained source region;
                        // cropped-away pixels must not bleed across the edge.
                        let xmin =
                            (self.crop[0] - 0.5).ceil().clamp(0., (self.sw - 1) as f64) as i64;
                        let xmax = (self.crop[2] - 0.5)
                            .floor()
                            .clamp(xmin as f64, (self.sw - 1) as f64)
                            as i64;
                        let ymin =
                            (self.crop[1] - 0.5).ceil().clamp(0., (self.sh - 1) as f64) as i64;
                        let ymax = (self.crop[3] - 0.5)
                            .floor()
                            .clamp(ymin as f64, (self.sh - 1) as f64)
                            as i64;
                        let xx = (ix as i64 + dx).clamp(xmin, xmax) as usize;
                        let yy = (iy as i64 + dy).clamp(ymin, ymax) as usize;
                        let weight =
                            if dx == 0 { 1. - fx } else { fx } * if dy == 0 { 1. - fy } else { fy };
                        for (i, value) in rgba.iter_mut().enumerate() {
                            *value += pixels[yy * self.sw + xx][i] * weight;
                        }
                    }
                }
                if rgba[3] > 0. {
                    for i in 0..3 {
                        rgba[i] /= rgba[3];
                    }
                }
                let base = (y * self.w + x) * 8;
                for i in 0..4 {
                    output[base + 2 * i..base + 2 * i + 2].copy_from_slice(
                        &((rgba[i].clamp(0., 1.) * 65535.).round() as u16).to_le_bytes(),
                    );
                }
            }
        }
    }
}

/// Stream one trimmed layer through the CPU stage into a lossless alpha-capable
/// intermediate. No whole-clip pixel buffering and no shell/filter path quoting.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    command: &std::process::Command,
    graph: &str,
    label: &str,
    geometry: &Geometry,
    pipeline: Option<&Pipeline>,
    fps: &str,
    count: i64,
    output: &Path,
) -> Result<()> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let parent = output.parent().context("missing intermediate directory")?;
    let mut decoder = Command::new(command.get_program());
    decoder.args(command.get_args());
    if let Some(dir) = command.get_current_dir() {
        decoder.current_dir(dir);
    }
    let graph_path = output.with_extension("graph");
    fs::write(&graph_path, graph)?;
    let decode_log = output.with_extension("decode.log");
    let encode_log = output.with_extension("encode.log");
    let mut dec = decoder
        .arg(crate::media_process::graph_file_option()?)
        .arg(graph_path)
        .args([
            "-map",
            &format!("[{label}]"),
            "-an",
            "-fps_mode",
            "passthrough",
            "-threads",
            "1",
            "-pix_fmt",
            "rgba64le",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&decode_log)?)
        .media_spawn()?;
    let encoder = Command::new("ffmpeg")
        .current_dir(parent)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-threads",
            "1",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba64le",
            "-video_size",
            &format!("{}x{}", geometry.w, geometry.h),
            "-framerate",
            fps,
            "-i",
            "pipe:0",
            "-an",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-threads",
            "2",
            "-pix_fmt",
            "gbrap16le",
        ])
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&encode_log)?)
        .media_spawn();
    let mut enc = match encoder {
        Ok(p) => p,
        Err(e) => {
            let _ = dec.kill();
            let _ = dec.wait();
            return Err(e);
        }
    };
    let result = (|| -> Result<()> {
        let mut source = vec![
            0u8;
            geometry
                .sw
                .checked_mul(geometry.sh)
                .and_then(|n| n.checked_mul(8))
                .context("frame too large")?
        ];
        let mut target = vec![
            0u8;
            geometry
                .w
                .checked_mul(geometry.h)
                .and_then(|n| n.checked_mul(8))
                .context("frame too large")?
        ];
        let mut input = dec.stdout.take().unwrap();
        let mut writer = enc.stdin.take().unwrap();
        for _ in 0..count {
            input
                .read_exact(&mut source)
                .context("VISUAL_DECODE: incomplete frame/clip")?;
            geometry.frame(&source, pipeline, &mut target);
            writer.write_all(&target).context("VISUAL_ENCODE")?;
        }
        let mut extra = [0u8; 1];
        ensure!(
            input.read(&mut extra)? == 0,
            "VISUAL_DECODE: unexpected extra frames"
        );
        drop(writer);
        Ok(())
    })();
    if result.is_err() {
        let _ = dec.kill();
        let _ = enc.kill();
    }
    let decoded = dec.wait()?;
    let encoded = enc.wait()?;
    result.with_context(|| {
        format!(
            "{} {}",
            fs::read_to_string(&decode_log).unwrap_or_default(),
            fs::read_to_string(&encode_log).unwrap_or_default()
        )
    })?;
    ensure!(
        decoded.success() && encoded.success(),
        "VISUAL_STAGE_FAILED: {} {}",
        fs::read_to_string(decode_log)?,
        fs::read_to_string(encode_log)?
    );
    Ok(())
}
