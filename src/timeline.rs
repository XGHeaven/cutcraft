//! CLI edits compile to ordinary atomic domain commands, including linked A/V.
use crate::{
    animation as anim,
    project::{array, text},
    time::{bounds, Time},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

pub fn animate(item: &mut Value, property: &str, effect: Option<&str>, value: Value) -> Result<()> {
    ensure!(
        anim::is_animation(&value),
        "expected an animation envelope with a $animation member"
    );
    let allowed = [
        "video.opacity",
        "video.transform.position",
        "video.transform.position.x",
        "video.transform.position.y",
        "video.transform.anchor",
        "video.transform.anchor.x",
        "video.transform.anchor.y",
        "video.transform.scale",
        "video.transform.scale.x",
        "video.transform.scale.y",
        "video.transform.rotationDegrees",
        "video.transform.crop.left",
        "video.transform.crop.right",
        "video.transform.crop.top",
        "video.transform.crop.bottom",
        "audio.gainDb",
        "audio.pan",
    ];
    if let Some(id) = effect {
        let e = item
            .get_mut("effects")
            .and_then(Value::as_array_mut)
            .and_then(|a| a.iter_mut().find(|e| e["id"] == id))
            .context("effect not found")?;
        let fields: &[&str] = match text(&e["effectType"]) {
            "builtin.color-grade" => &[
                "exposureEv",
                "temperature",
                "tint",
                "contrast",
                "pivot",
                "saturation",
            ],
            "builtin.lut" => &["strength"],
            _ => &[],
        };
        ensure!(
            fields.contains(&property),
            "unregistered effect animation property"
        );
        e["version"] = json!("2.0.0");
        e["parameters"][property] = value;
    } else {
        ensure!(
            allowed.contains(&property),
            "unregistered animation property"
        );
        let mut target = item;
        let parts: Vec<_> = property.split('.').collect();
        for (n, k) in parts.iter().enumerate() {
            ensure!(
                !anim::is_animation(target),
                "replace whole-vector animation before animating a component"
            );
            if target.is_null() {
                *target = json!({});
            }
            if n + 1 == parts.len() {
                target[*k] = value;
                break;
            }
            if matches!(*k, "position" | "anchor" | "scale")
                && target[*k].is_null()
                && n + 2 < parts.len()
            {
                let d = if *k == "scale" { 1. } else { 0.5 };
                target[*k] = json!({"x":d,"y":d});
            }
            target = &mut target[*k];
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub fn retime(
    project: &Value,
    sid: &str,
    iid: &str,
    mapping: Option<Value>,
    rate: Option<&str>,
    ripple: &str,
    animations: &str,
    replace: bool,
    audio_policy: Option<&str>,
) -> Result<Value> {
    let seq = &project["sequences"][sid];
    let primary = array(&seq["tracks"])
        .iter()
        .flat_map(|t| array(&t["items"]))
        .find(|i| i["id"] == iid)
        .context("item not found")?;
    let mut targets = vec![iid.to_string()];
    if let Some(lid) = primary["linkGroupId"].as_str() {
        let group = array(&seq["links"])
            .iter()
            .find(|l| l["id"] == lid)
            .context("missing link")?;
        targets = array(&group["itemIds"])
            .iter()
            .map(|v| text(v).into())
            .collect();
    }
    let old = Time::from_json(&primary["timelineRange"]["duration"])?;
    let source = Time::from_json(&primary["sourceRange"]["duration"])?;
    let (new, playback) = if let Some(mapping) = mapping {
        let keys = array(&mapping["$animation"]["keyframes"]);
        let duration = Time::from_json(&keys.last().context("timeMap needs keys")?["offset"])?;
        (
            duration,
            json!({"mode":"remap","version":"1.0.0","timeMap":mapping}),
        )
    } else {
        let r = rate.context("missing rate")?;
        let (reverse, r) = if let Some(s) = r.strip_prefix('-') {
            (true, s)
        } else {
            (false, r)
        };
        let magnitude: Time = r.parse()?;
        ensure!(
            magnitude > Time::zero(),
            "zero speed needs explicit timeMap duration"
        );
        (
            source.div(&magnitude)?,
            json!({"mode":"fit","reverse":reverse}),
        )
    };
    ensure!(new > Time::zero(), "duration must be positive");
    let delta = new.sub(&old);
    let factor = new.div(&old)?;
    let mut commands = vec![];
    let mut affected_tracks = vec![];
    let boundary = bounds(&primary["timelineRange"])?.1;
    for track in array(&seq["tracks"]) {
        for i in array(&track["items"]) {
            if !targets.contains(&text(&i["id"]).to_string()) {
                continue;
            }
            ensure!(i["timelineRange"]==primary["timelineRange"] && Time::from_json(&i["sourceRange"]["duration"])?==source,"linked retime requires equal timeline/source durations; edit linked items explicitly in a batch");
            ensure!(
                replace || playback["mode"] == "remap" || i["playback"]["mode"] != "remap",
                "use --replace-remap to discard existing timeMap"
            );
            let mut item = i.clone();
            item["timelineRange"]["duration"] = new.json()?;
            item["playback"] = playback.clone();
            for k in ["audioPolicy", "videoSampling"] {
                if let Some(v) = i["playback"].get(k) {
                    item["playback"][k] = v.clone();
                }
            }
            if track["kind"] == "audio" {
                if let Some(policy) = audio_policy {
                    item["playback"]["audioPolicy"] = if policy == "mute" {
                        json!({"mode":"mute"})
                    } else {
                        json!({"mode":"resample","processor":{"id":"cutcraft.linear-resample","version":"1.0.0","parameters":{}}})
                    };
                }
            }
            if animations == "stretch" {
                anim::slots(&mut item, |v| {
                    if anim::is_animation(v) {
                        for k in v["$animation"]["keyframes"].as_array_mut().unwrap() {
                            k["offset"] = Time::from_json(&k["offset"])?.mul(&factor).json()?;
                        }
                    }
                    Ok(())
                })?;
            }
            affected_tracks.push(text(&track["id"]).to_string());
            commands.push(
                json!({"type":"item.put","sequenceId":sid,"trackId":track["id"],"item":item}),
            );
        }
    }
    if ripple != "none" {
        ensure!(
            matches!(ripple, "insert" | "remove-gap"),
            "unsupported retime ripple policy"
        );
        let mut shifted = std::collections::BTreeSet::new();
        for track in array(&seq["tracks"]) {
            if !affected_tracks.contains(&text(&track["id"]).to_string()) {
                continue;
            }
            for item in array(&track["items"]) {
                if !targets.contains(&text(&item["id"]).to_string())
                    && bounds(&item["timelineRange"])?.0 >= boundary
                {
                    shifted.insert(text(&item["id"]).to_string());
                    if let Some(lid) = item["linkGroupId"].as_str() {
                        for l in array(&seq["links"]).iter().filter(|l| l["id"] == lid) {
                            for id in array(&l["itemIds"]) {
                                shifted.insert(text(id).into());
                            }
                        }
                    }
                }
            }
        }
        for track in array(&seq["tracks"]) {
            for i in array(&track["items"]) {
                if shifted.contains(text(&i["id"]))
                    && !targets.contains(&text(&i["id"]).to_string())
                {
                    let mut item = i.clone();
                    item["timelineRange"]["start"] =
                        bounds(&i["timelineRange"])?.0.add(&delta).json()?;
                    commands.push(json!({"type":"item.put","sequenceId":sid,"trackId":track["id"],"item":item}));
                }
            }
        }
    }
    Ok(json!({"type":"batch","commands":commands}))
}
