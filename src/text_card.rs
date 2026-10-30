//! Optional text-card layout and linear-light decoration. Existing text stays unchanged.
use crate::{
    capabilities::{self, Capability},
    media_process::MediaCommand,
    project::text,
    visual::Space,
};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn number(v: &Value, key: &str, default: f64) -> f64 {
    v[key].as_f64().unwrap_or(default)
}
pub fn validate(version: &str, style: &Value, content: &str) -> Result<()> {
    let Some(card) = style.get("card") else {
        return Ok(());
    };
    ensure!(
        capabilities::supports(version, Capability::TextCards),
        "TEXT_CARD_UNSUPPORTED_VERSION: requires Spec 0.1.0"
    );
    crate::schema::validate("text-card", card)?;
    let edge = number(card, "widthPixels", 0.).min(number(card, "heightPixels", 0.));
    ensure!(
        number(card, "paddingPixels", 16.) * 2. < edge,
        "TEXT_CARD_INVALID: padding consumes content box"
    );
    ensure!(
        number(card, "radiusPixels", 0.) <= edge / 2.,
        "TEXT_CARD_INVALID: radius exceeds half short edge"
    );
    ensure!(
        number(&card["border"], "widthPixels", 0.) <= edge / 2.,
        "TEXT_CARD_INVALID: border exceeds half short edge"
    );
    ensure!(
        number(style, "fontSizePixels", 48.) <= 4096.,
        "TEXT_CARD_INVALID: font size exceeds 4096"
    );
    ensure!(
        style.get("backgroundColor").is_none() && style.get("maxLines").is_none(),
        "TEXT_CARD_INVALID: card conflicts with backgroundColor/maxLines"
    );
    let content = content.replace("\r\n", "\n");
    ensure!(
        content.split('\n').count() <= 128,
        "TEXT_CARD_INVALID: at most 128 explicit lines"
    );
    ensure!(
        !content.chars().any(|c| c.is_control() && c != '\n'),
        "TEXT_CARD_INVALID: only LF/CRLF control characters are supported"
    );
    Ok(())
}
fn color(v: &Value) -> [f64; 4] {
    if v.is_null() {
        return [0.; 4];
    }
    let a = number(v, "a", 1.);
    [
        number(v, "r", 0.) * a,
        number(v, "g", 0.) * a,
        number(v, "b", 0.) * a,
        a,
    ]
}
fn over(dst: &mut [f64; 4], src: [f64; 4], coverage: f64) {
    let a = src[3] * coverage;
    for i in 0..4 {
        dst[i] = src[i] * coverage + dst[i] * (1. - a);
    }
}
fn rounded(x: f64, y: f64, w: f64, h: f64, r: f64) -> f64 {
    if w <= 0. || h <= 0. {
        return 0.;
    }
    let qx = (x - w / 2.).abs() - (w / 2. - r);
    let qy = (y - h / 2.).abs() - (h / 2. - r);
    let distance = qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - r;
    (0.5 - distance).clamp(0., 1.)
}
fn blur(mask: &mut [f64], w: usize, h: usize, sigma: f64) -> Result<()> {
    if sigma <= 0. {
        return Ok(());
    }
    let radius = (sigma * 3.).ceil() as isize;
    let mut kernel: Vec<f64> = (-radius..=radius)
        .map(|k| (-(k as f64).powi(2) / (2. * sigma * sigma)).exp())
        .collect();
    let total: f64 = kernel.iter().sum();
    kernel.iter_mut().for_each(|v| *v /= total);
    let mut temp = vec![0.; mask.len()];
    for axis in 0..2 {
        for y in 0..h {
            crate::media_process::check_cancellation()?;
            for x in 0..w {
                let mut sum = 0.;
                for (k, weight) in (-radius..=radius).zip(&kernel) {
                    let (xx, yy) = if axis == 0 {
                        (x as isize + k, y as isize)
                    } else {
                        (x as isize, y as isize + k)
                    };
                    if xx >= 0 && yy >= 0 && xx < w as isize && yy < h as isize {
                        sum += mask[yy as usize * w + xx as usize] * weight;
                    }
                }
                temp[y * w + x] = sum;
            }
        }
        mask.copy_from_slice(&temp);
    }
    Ok(())
}

/// Card colors are linear sRGB; returned bytes are Rec.709 straight RGBA16.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    style: &Value,
    content: &str,
    w: usize,
    h: usize,
    scale: f64,
    font: Option<&crate::fonts::ResolvedFont>,
    scratch: &Path,
) -> Result<Vec<u8>> {
    crate::media_process::check_cancellation()?;
    let card = &style["card"];
    let cw = (number(card, "widthPixels", 0.) * scale).max(1.);
    let ch = (number(card, "heightPixels", 0.) * scale).max(1.);
    let radius = (number(card, "radiusPixels", 0.) * scale).min(cw.min(ch) / 2.);
    let pad = number(card, "paddingPixels", 16.) * scale;
    let left = match text(&style["align"]) {
        "left" => 20. * scale,
        "right" => w as f64 - cw - 20. * scale,
        _ => (w as f64 - cw) / 2.,
    };
    let top = match text(&style["position"]) {
        "top" => 20. * scale,
        "center" => (h as f64 - ch) / 2.,
        _ => h as f64 - ch - 40. * scale,
    };
    let mut pixels = vec![[0.; 4]; w * h];
    if let Some(shadow) = card.get("shadow") {
        let sigma = number(shadow, "sigmaPixels", 0.) * scale;
        // Keep a transparent halo so shadows just outside the canvas can blur in.
        let halo = (sigma * 3.).ceil() as usize + 1;
        let sw = w + 2 * halo;
        let sh = h + 2 * halo;
        ensure!(
            sw.checked_mul(sh).is_some_and(|n| n <= 32_000_000),
            "TEXT_CARD_TOO_LARGE: shadow canvas"
        );
        let mut mask = vec![0.; sw * sh];
        let dx = number(shadow, "offsetXPixels", 0.) * scale;
        let dy = number(shadow, "offsetYPixels", 0.) * scale;
        for y in 0..sh {
            crate::media_process::check_cancellation()?;
            for x in 0..sw {
                mask[y * sw + x] = rounded(
                    x as f64 - halo as f64 + 0.5 - left - dx,
                    y as f64 - halo as f64 + 0.5 - top - dy,
                    cw,
                    ch,
                    radius,
                );
            }
        }
        blur(&mut mask, sw, sh, sigma)?;
        let ink = color(&shadow["color"]);
        for y in 0..h {
            for x in 0..w {
                over(
                    &mut pixels[y * w + x],
                    ink,
                    mask[(y + halo) * sw + x + halo],
                );
            }
        }
    }
    let background = &card["background"];
    let gradient = background["type"] == "linear-gradient";
    let first = color(if gradient {
        &background["startColor"]
    } else {
        &background["color"]
    });
    let last = color(&background["endColor"]);
    let border = &card["border"];
    let bw = number(border, "widthPixels", 0.) * scale;
    let border_color = color(&border["color"]);
    for y in 0..h {
        crate::media_process::check_cancellation()?;
        for x in 0..w {
            let xx = x as f64 + 0.5 - left;
            let yy = y as f64 + 0.5 - top;
            let outer = rounded(xx, yy, cw, ch, radius);
            let t = if background["direction"] == "vertical" {
                yy / ch
            } else {
                xx / cw
            };
            let mut fill = if gradient {
                std::array::from_fn(|i| first[i] + (last[i] - first[i]) * t.clamp(0., 1.))
            } else {
                first
            };
            if bw > 0. && outer > 0. {
                let inner = rounded(
                    xx - bw,
                    yy - bw,
                    cw - 2. * bw,
                    ch - 2. * bw,
                    (radius - bw).max(0.),
                );
                // Both paints share the outer geometric coverage. Applying it
                // twice would make translucent rounded edges too opaque.
                over(
                    &mut fill,
                    border_color,
                    ((outer - inner) / outer).clamp(0., 1.),
                );
            }
            over(&mut pixels[y * w + x], fill, outer);
        }
    }
    let iw = (cw - 2. * pad).floor().max(1.) as usize;
    let ih = (ch - 2. * pad).floor().max(1.) as usize;
    let temp = tempfile::tempdir_in(scratch)?;
    let font = font
        .map(|f| f.prepare(temp.path(), "font.ttf", content))
        .transpose()?
        .unwrap_or_default();
    let content = content.replace("\r\n", "\n");
    let lines: Vec<_> = content.split('\n').collect();
    let size = number(style, "fontSizePixels", 48.) * scale;
    let line_height = size * number(card, "lineHeight", 1.2);
    let height = line_height * lines.len() as f64;
    let y = match text(&card["verticalAlign"]) {
        "center" => (ih as f64 - height) / 2.,
        "bottom" => ih as f64 - height,
        _ => 0.,
    };
    for (i, line) in lines.iter().enumerate() {
        fs::write(temp.path().join(format!("line{i}.txt")), line)?;
    }
    let mask = |stroke: f64| -> Result<Vec<u8>> {
        let mut graph = format!("color=black@0:s={iw}x{ih},format=rgba");
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let x = match text(&style["align"]) {
                "left" => "0",
                "right" => "w-tw",
                _ => "(w-tw)/2",
            };
            graph.push_str(&format!(",drawtext=textfile=line{i}.txt:expansion=none{font}:fontsize={size}:fontcolor=white:bordercolor=white:borderw={}:x={x}:y={}:y_align=font",stroke.round(),y+i as f64*line_height));
        }
        graph.push_str(",format=rgba[out]");
        fs::write(temp.path().join("graph.txt"), graph)?;
        let out = Command::new("ffmpeg")
            .current_dir(temp.path())
            .args([
                "-nostdin",
                "-v",
                "error",
                "-filter_complex_threads",
                "2",
                crate::media_process::graph_file_option()?,
                "graph.txt",
                "-map",
                "[out]",
                "-frames:v",
                "1",
                "-threads",
                "2",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .media_output()?;
        ensure!(
            out.status.success(),
            "TEXT_CARD_RENDER_FAILED: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        ensure!(
            out.stdout.len() == iw * ih * 4,
            "TEXT_CARD_RENDER_FAILED: invalid mask dimensions"
        );
        Ok(out.stdout.chunks_exact(4).map(|p| p[3]).collect())
    };
    let glyphs = mask(0.)?;
    let stroke = card.get("textStroke");
    let outlined = stroke
        .map(|s| mask(number(s, "widthPixels", 0.) * scale))
        .transpose()?;
    let ink = if style["color"].is_null() {
        [1.; 4]
    } else {
        color(&style["color"])
    };
    for y in 0..ih {
        for x in 0..iw {
            let xx = (left + pad).floor() as isize + x as isize;
            let yy = (top + pad).floor() as isize + y as isize;
            if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                continue;
            }
            let dst = &mut pixels[yy as usize * w + xx as usize];
            let a = glyphs[y * iw + x] as f64 / 255.;
            let mut paint = ink.map(|v| v * a);
            if let (Some(stroke), Some(outlined)) = (stroke, &outlined) {
                let ring = (outlined[y * iw + x] as f64 / 255. - a).max(0.);
                let stroke = color(&stroke["color"]);
                for i in 0..4 {
                    paint[i] += stroke[i] * ring;
                }
            }
            over(dst, paint, 1.);
        }
    }
    let mut out = Vec::with_capacity(w * h * 8);
    for pixel in pixels {
        for i in 0..4 {
            let v = if i == 3 {
                pixel[3]
            } else if pixel[3] > 0. {
                Space::Rec709.encode(pixel[i] / pixel[3])
            } else {
                0.
            };
            out.extend_from_slice(&((v.clamp(0., 1.) * 65535.).round() as u16).to_le_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn border_and_background_share_antialiased_outline() {
        let scratch = tempfile::tempdir().unwrap();
        let mut style = json!({"position":"center","card":{"widthPixels":48,"heightPixels":48,"paddingPixels":8,"radiusPixels":16,"background":{"type":"solid","color":{"r":1,"g":0,"b":0,"a":1}}}});
        let plain = render(&style, " ", 64, 64, 1., None, scratch.path()).unwrap();
        style["card"]["border"] = json!({"widthPixels":4,"color":{"r":0,"g":1,"b":0,"a":1}});
        let bordered = render(&style, " ", 64, 64, 1., None, scratch.path()).unwrap();
        let mut fractional = false;
        for (a, b) in plain.chunks_exact(8).zip(bordered.chunks_exact(8)) {
            assert_eq!(
                &a[6..8],
                &b[6..8],
                "opaque border must preserve the same geometric alpha"
            );
            let alpha = u16::from_le_bytes([a[6], a[7]]);
            fractional |= alpha > 0 && alpha < 65535;
        }
        assert!(
            fractional,
            "fixture must exercise antialiased corner pixels"
        );
    }
}
