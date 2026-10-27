//! Registered property animations and exact (non-integrating) source time maps.
use crate::{
    project::{array, text},
    time::{bounds, Time},
};
use anyhow::{ensure, Context, Result};
use num_traits::ToPrimitive;
use serde_json::{json, Value};

pub fn float(t: &Time) -> Result<f64> {
    t.0.to_f64()
        .context("rational exceeds floating point range")
}
fn rat(v: &Value) -> Result<Time> {
    let t = Time::from_json(v)?;
    ensure!(t.json()? == *v, "NON_CANONICAL: animation rational");
    Ok(t)
}
pub fn is_animation(v: &Value) -> bool {
    v.get("$animation").is_some()
}
pub fn validate(v: &Value, duration: &Time) -> Result<()> {
    if !is_animation(v) {
        return Ok(());
    }
    crate::schema::validate("animation", v)?;
    let a = &v["$animation"];
    let keys = array(&a["keyframes"]);
    let mut previous = None;
    for (i, k) in keys.iter().enumerate() {
        let t = rat(&k["offset"])?;
        ensure!(
            t.nonnegative() && &t <= duration && previous.as_ref().is_none_or(|p| p < &t),
            "ANIMATION_INVALID: key offsets must increase within owner duration"
        );
        previous = Some(t);
        let e = &k["easing"];
        ensure!(
            i + 1 < keys.len() || e.is_null(),
            "ANIMATION_INVALID: last key must omit easing"
        );
        if e["type"] == "cubic-bezier" {
            ensure!(
                e["x1"].as_f64() <= e["x2"].as_f64(),
                "ANIMATION_INVALID: x1 > x2"
            );
        }
        if a["valueType"] == "time" {
            rat(&k["value"])?;
            if e["type"] == "cubic-hermite" {
                rat(&e["outRate"])?;
                rat(&e["inRate"])?;
            }
        }
    }
    Ok(())
}
/// Polynomial coefficients in normalized segment time, all exact rationals.
fn coefficients(a: &Value, b: &Value) -> Result<([Time; 4], Time)> {
    let d = rat(&b["offset"])?.sub(&rat(&a["offset"])?);
    let s0 = rat(&a["value"])?;
    let s1 = rat(&b["value"])?;
    let (m0, m1) = if a["easing"]["type"] == "cubic-hermite" {
        (rat(&a["easing"]["outRate"])?, rat(&a["easing"]["inRate"])?)
    } else {
        let r = s1.sub(&s0).div(&d)?;
        (r.clone(), r)
    };
    Ok((
        [
            s0.sub(&s1).mul(&Time::integer(2)).add(&d.mul(&m0.add(&m1))),
            s1.sub(&s0)
                .mul(&Time::integer(3))
                .sub(&d.mul(&m0.mul(&Time::integer(2)).add(&m1))),
            d.mul(&m0),
            s0,
        ],
        d,
    ))
}
/// Return position, velocity, and first nonzero derivative's direction.
pub fn time_value(v: &Value, t: &Time, left: bool) -> Result<(Time, Time, i8)> {
    let keys = array(&v["$animation"]["keyframes"]);
    ensure!(keys.len() >= 2, "timeMap requires two keys");
    let mut pair = &keys[keys.len() - 2..];
    for p in keys.windows(2) {
        let end = rat(&p[1]["offset"])?;
        if t < &end || (left && t == &end) {
            pair = p;
            break;
        }
    }
    let (c, d) = coefficients(&pair[0], &pair[1])?;
    let u = t.sub(&rat(&pair[0]["offset"])?).div(&d)?;
    let position = c[0]
        .mul(&u)
        .add(&c[1])
        .mul(&u)
        .add(&c[2])
        .mul(&u)
        .add(&c[3]);
    let first = c[0]
        .mul(&u)
        .mul(&Time::integer(3))
        .add(&c[1].mul(&Time::integer(2)))
        .mul(&u)
        .add(&c[2]);
    let second = c[0]
        .mul(&u)
        .mul(&Time::integer(6))
        .add(&c[1].mul(&Time::integer(2)));
    let direction = [
        first.clone(),
        if left {
            Time::zero().sub(&second)
        } else {
            second
        },
        c[0].clone(),
    ]
    .iter()
    .find(|v| **v != Time::zero())
    .map(|v| if v.nonnegative() { 1 } else { -1 })
    .unwrap_or(0);
    Ok((position, first.div(&d)?, direction))
}
fn bezier(u: f64, e: &Value) -> f64 {
    let curve = |q: f64, a: f64, b: f64| {
        3. * (1. - q).powi(2) * q * a + 3. * (1. - q) * q * q * b + q * q * q
    };
    let (mut lo, mut hi) = (0., 1.);
    let mut q = u;
    for _ in 0..80 {
        let x = curve(q, e["x1"].as_f64().unwrap(), e["x2"].as_f64().unwrap());
        if (x - u).abs() <= 1e-12 {
            break;
        }
        if x < u {
            lo = q
        } else {
            hi = q
        }
        q = (lo + hi) / 2.;
    }
    curve(q, e["y1"].as_f64().unwrap(), e["y2"].as_f64().unwrap())
}
fn canonical_color(mut v: Value) -> Value {
    if v["a"].as_f64() == Some(0.) {
        for k in ["r", "g", "b"] {
            v[k] = json!(0.);
        }
    }
    v
}
pub fn evaluate(v: &Value, t: &Time) -> Result<Value> {
    if !is_animation(v) {
        return Ok(v.clone());
    }
    let a = &v["$animation"];
    let keys = array(&a["keyframes"]);
    ensure!(!keys.is_empty(), "empty animation");
    let value = if t <= &rat(&keys[0]["offset"])? {
        keys[0]["value"].clone()
    } else if t >= &rat(&keys.last().unwrap()["offset"])? {
        keys.last().unwrap()["value"].clone()
    } else {
        let mut result = Value::Null;
        for pair in keys.windows(2) {
            let a0 = rat(&pair[0]["offset"])?;
            let a1 = rat(&pair[1]["offset"])?;
            if t >= &a0 && t < &a1 {
                let v0 = &pair[0]["value"];
                let v1 = &pair[1]["value"];
                let e = &pair[0]["easing"];
                if a["valueType"] == "time" {
                    result = time_value(v, t, false)?.0.json()?;
                    break;
                }
                if e["type"] == "hold" || matches!(text(&a["valueType"]), "boolean" | "string") {
                    result = v0.clone();
                    break;
                }
                let u = float(&t.sub(&a0).div(&a1.sub(&a0))?)?;
                let s = if e["type"] == "cubic-bezier" {
                    bezier(u, e)
                } else {
                    u
                };
                let mix = |x: f64, y: f64| x + (y - x) * s;
                result = match text(&a["valueType"]) {
                    "number" => json!(mix(v0.as_f64().unwrap(), v1.as_f64().unwrap())),
                    "vector2" => {
                        json!({"x":mix(v0["x"].as_f64().unwrap(),v1["x"].as_f64().unwrap()),"y":mix(v0["y"].as_f64().unwrap(),v1["y"].as_f64().unwrap())})
                    }
                    "color" => {
                        let aa = v0["a"].as_f64().unwrap();
                        let ab = v1["a"].as_f64().unwrap();
                        let alpha = mix(aa, ab);
                        let mut c = json!({"a":alpha});
                        for k in ["r", "g", "b"] {
                            c[k] = json!(if alpha == 0. {
                                0.
                            } else {
                                mix(v0[k].as_f64().unwrap() * aa, v1[k].as_f64().unwrap() * ab)
                                    / alpha
                            });
                        }
                        c
                    }
                    other => anyhow::bail!("unsupported animation type {other}"),
                };
                break;
            }
        }
        result
    };
    Ok(if a["valueType"] == "color" {
        canonical_color(value)
    } else {
        value
    })
}
/// Visit only registered slots; never interpret opaque plugin JSON.
pub fn slots(item: &mut Value, mut f: impl FnMut(&mut Value) -> Result<()>) -> Result<()> {
    if let Some(video) = item.get_mut("video") {
        if let Some(v) = video.get_mut("opacity") {
            f(v)?;
        }
        if let Some(tr) = video.get_mut("transform") {
            for k in ["position", "anchor", "scale"] {
                if let Some(v) = tr.get_mut(k) {
                    if is_animation(v) {
                        f(v)?;
                    } else {
                        for c in ["x", "y"] {
                            if let Some(v) = v.get_mut(c) {
                                f(v)?;
                            }
                        }
                    }
                }
            }
            if let Some(v) = tr.get_mut("rotationDegrees") {
                f(v)?;
            }
            if let Some(crop) = tr.get_mut("crop") {
                for k in ["left", "right", "top", "bottom"] {
                    if let Some(v) = crop.get_mut(k) {
                        f(v)?;
                    }
                }
            }
        }
    }
    if let Some(audio) = item.get_mut("audio") {
        for k in ["gainDb", "pan"] {
            if let Some(v) = audio.get_mut(k) {
                f(v)?;
            }
        }
    }
    for e in item
        .get_mut("effects")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let fields: &[&str] = match (text(&e["effectType"]), text(&e["version"])) {
            ("builtin.color-grade", "2.0.0") => &[
                "exposureEv",
                "temperature",
                "tint",
                "contrast",
                "pivot",
                "saturation",
            ],
            ("builtin.lut", "2.0.0") => &["strength"],
            ("builtin.gaussian-blur", "1.0.0") => &["sigma"],
            _ => &[],
        };
        for k in fields {
            if let Some(v) = e["parameters"].get_mut(*k) {
                f(v)?;
            }
        }
    }
    Ok(())
}
pub fn evaluated(item: &Value, t: &Time) -> Result<Value> {
    let mut out = item.clone();
    slots(&mut out, |v| {
        *v = evaluate(v, t)?;
        Ok(())
    })?;
    Ok(out)
}
pub fn animated(item: &Value) -> bool {
    let mut found = false;
    let _ = slots(&mut item.clone(), |v| {
        found |= is_animation(v);
        Ok(())
    });
    found
}
pub fn validate_item(item: &Value, kind: &str) -> Result<()> {
    let duration = rat(&item["timelineRange"]["duration"])?;
    slots(&mut item.clone(), |v| validate(v, &duration))?;
    let p = &item["playback"];
    let mode = text(&p["mode"]);
    ensure!(
        kind != "audio" || p["videoSampling"].is_null(),
        "videoSampling on audio"
    );
    ensure!(
        kind == "audio" || p["audioPolicy"].is_null(),
        "audioPolicy on video"
    );
    let mut identity = mode == "loop" && p["reverse"] != true;
    let mut negative = p["reverse"] == true;
    if !item["sourceRange"].is_null() && matches!(mode, "" | "fit") {
        identity = rat(&item["sourceRange"]["duration"])? == duration && !negative;
    }
    if mode == "remap" {
        ensure!(
            !item["sourceRange"].is_null() && matches!(text(&item["type"]), "media" | "sequence"),
            "remap needs timed source"
        );
        let length = rat(&item["sourceRange"]["duration"])?;
        let v = &p["timeMap"];
        validate(v, &duration)?;
        let keys = array(&v["$animation"]["keyframes"]);
        ensure!(
            keys.len() >= 2
                && rat(&keys[0]["offset"])? == Time::zero()
                && rat(&keys.last().unwrap()["offset"])? == duration,
            "timeMap must cover entire duration"
        );
        identity = true;
        negative = false;
        for pair in keys.windows(2) {
            let (c, d) = coefficients(&pair[0], &pair[1])?;
            let s0 = rat(&pair[0]["value"])?;
            let s1 = rat(&pair[1]["value"])?;
            let end_derivative = c[0]
                .mul(&Time::integer(3))
                .add(&c[1].mul(&Time::integer(2)))
                .add(&c[2]);
            for q in [
                s0.clone(),
                s0.add(&c[2].div(&Time::integer(3))?),
                s1.sub(&end_derivative.div(&Time::integer(3))?),
                s1,
            ] {
                ensure!(
                    q.nonnegative() && q <= length,
                    "timeMap control hull exceeds sourceRange"
                );
            }
            identity &= c[0] == Time::zero() && c[1] == Time::zero() && c[2] == d;
            negative |= !c[2].nonnegative() || !end_derivative.nonnegative();
            if c[0] > Time::zero() {
                let u = Time::zero().sub(&c[1]).div(&c[0].mul(&Time::integer(3)))?;
                if u > Time::zero() && u < Time::integer(1) {
                    negative |= !c[0]
                        .mul(&u)
                        .mul(&Time::integer(3))
                        .add(&c[1].mul(&Time::integer(2)))
                        .mul(&u)
                        .add(&c[2])
                        .nonnegative();
                }
            }
        }
    }
    if kind == "audio" && !item["sourceRange"].is_null() {
        ensure!(
            identity || !p["audioPolicy"].is_null(),
            "retimed audio requires explicit audioPolicy"
        );
        ensure!(
            !(negative && p["audioPolicy"]["mode"] == "preserve-pitch"),
            "preserve-pitch cannot reverse"
        );
    }
    Ok(())
}
/// Handles extend linearly from endpoint position and velocity. No accumulated rate.
pub fn mapping(item: &Value, t: &Time, incoming_left: bool) -> Result<(Time, bool, bool)> {
    let (ss, se) = bounds(&item["sourceRange"])?;
    let length = se.sub(&ss);
    let duration = rat(&item["timelineRange"]["duration"])?;
    let p = &item["playback"];
    let mode = text(&p["mode"]);
    let (pos, dir, frozen) = if mode == "remap" {
        let clamped = t.clone().max(Time::zero()).min(duration.clone());
        let (s, r, dir) = time_value(
            &p["timeMap"],
            &clamped,
            incoming_left || clamped == duration,
        )?;
        let direction = if t != &clamped {
            if r == Time::zero() {
                0
            } else if r.nonnegative() {
                1
            } else {
                -1
            }
        } else {
            dir
        };
        (s.add(&t.sub(&clamped).mul(&r)), direction, direction == 0)
    } else if mode == "hold" {
        (Time::zero(), 0, true)
    } else {
        let delta = if mode == "loop" {
            let mut v = Time(&t.0 % &length.0);
            if !v.nonnegative() {
                v = v.add(&length);
            }
            if incoming_left && v == Time::zero() {
                length.clone()
            } else {
                v
            }
        } else {
            t.mul(&length.div(&duration)?)
        };
        if p["reverse"] == true {
            (length.sub(&delta), -1, false)
        } else {
            (delta, 1, false)
        }
    };
    let upper = if pos == Time::zero() {
        false
    } else if pos == length {
        true
    } else {
        (dir < 0) ^ (incoming_left && !frozen)
    };
    Ok((ss.add(&pos), upper, frozen))
}
pub fn transition_window(tr: &Value, from: &Value) -> Result<(Time, Time)> {
    let cut = bounds(&from["timelineRange"])?.1;
    let d = rat(&tr["duration"])?;
    let before = match text(&tr["alignment"]) {
        "before" => d.clone(),
        "center" => d.div(&Time::integer(2))?,
        _ => Time::zero(),
    };
    let start = cut.sub(&before);
    Ok((start.clone(), start.add(&d)))
}

/// Restrict a curve to [start,end], preserving its function and local clock.
pub fn slice(value: &Value, start: &Time, end: &Time) -> Result<Value> {
    if !is_animation(value) {
        return Ok(value.clone());
    }
    ensure!(end > start, "empty animation slice");
    let keys = array(&value["$animation"]["keyframes"]);
    let is_time = value["$animation"]["valueType"] == "time";
    let mut offsets = vec![start.clone()];
    for key in keys {
        let t = rat(&key["offset"])?;
        if &t > start && &t < end {
            offsets.push(t);
        }
    }
    offsets.push(end.clone());
    let mut output = vec![];
    for (index, t) in offsets.iter().enumerate() {
        let mut key = json!({"offset":t.sub(start).json()?,"value":evaluate(value,t)?});
        if index + 1 < offsets.len() {
            let finish = &offsets[index + 1];
            let segment = keys.windows(2).find(|pair| {
                rat(&pair[0]["offset"]).is_ok_and(|a| a <= *t)
                    && rat(&pair[1]["offset"]).is_ok_and(|b| *t < b)
            });
            if let Some(pair) = segment {
                let e = &pair[0]["easing"];
                if is_time && e["type"] == "cubic-hermite" {
                    key["easing"] = json!({"type":"cubic-hermite","outRate":time_value(value,t,false)?.1.json()?,"inRate":time_value(value,finish,true)?.1.json()?});
                } else if e["type"] == "cubic-bezier" {
                    let a = rat(&pair[0]["offset"])?;
                    let d = rat(&pair[1]["offset"])?.sub(&a);
                    let u0 = float(&t.sub(&a).div(&d)?)?;
                    let u1 = float(&finish.sub(&a).div(&d)?)?;
                    key["easing"] = restrict_bezier(e, u0, u1);
                } else if !e.is_null() {
                    key["easing"] = e.clone();
                }
            } else {
                key["easing"] = json!({"type":if is_time{"linear"}else{"hold"}});
            }
        }
        output.push(key);
    }
    let mut result = value.clone();
    result["$animation"]["keyframes"] = json!(output);
    Ok(result)
}
fn restrict_bezier(e: &Value, u0: f64, u1: f64) -> Value {
    let x1 = e["x1"].as_f64().unwrap();
    let x2 = e["x2"].as_f64().unwrap();
    let y1 = e["y1"].as_f64().unwrap();
    let y2 = e["y2"].as_f64().unwrap();
    let c = |q: f64, a: f64, b: f64| {
        3. * (1. - q).powi(2) * q * a + 3. * (1. - q) * q * q * b + q.powi(3)
    };
    let derivative = |q: f64, a: f64, b: f64| {
        3. * (1. - q).powi(2) * a + 6. * (1. - q) * q * (b - a) + 3. * q * q * (1. - b)
    };
    let solve = |u: f64| {
        if u == 0. || u == 1. {
            return u;
        }
        let (mut lo, mut hi) = (0., 1.);
        for _ in 0..80 {
            let q = (lo + hi) / 2.;
            if c(q, x1, x2) < u {
                lo = q
            } else {
                hi = q
            }
        }
        (lo + hi) / 2.
    };
    let q0 = solve(u0);
    let q1 = solve(u1);
    let width = q1 - q0;
    let control = |a: f64, b: f64| {
        let begin = c(q0, a, b);
        let end = c(q1, a, b);
        let span = end - begin;
        if span == 0. {
            return [0., 1.];
        }
        [
            (width * derivative(q0, a, b) / 3. / span).clamp(0., 1.),
            (1. - width * derivative(q1, a, b) / 3. / span).clamp(0., 1.),
        ]
    };
    let x = control(x1, x2);
    let y = control(y1, y2);
    json!({"type":"cubic-bezier","x1":x[0],"x2":x[1].max(x[0]),"y1":y[0],"y2":y[1]})
}
