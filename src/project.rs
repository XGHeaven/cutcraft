use crate::{
    id, now, schema,
    time::{bounds, Time},
    SPEC_VERSION,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

thread_local! {
    static EXECUTION_CONTEXT: RefCell<Option<(Value, Value)>> = const { RefCell::new(None) };
}

/// Scope client attribution to one synchronous host execution, including unwinding.
pub fn with_context<T>(application: Value, actor: Value, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<(Value, Value)>);
    impl Drop for Reset {
        fn drop(&mut self) {
            EXECUTION_CONTEXT.with(|context| *context.borrow_mut() = self.0.take());
        }
    }
    let _reset =
        Reset(EXECUTION_CONTEXT.with(|context| context.replace(Some((application, actor)))));
    run()
}

pub(crate) fn execution_actor() -> Value {
    EXECUTION_CONTEXT
        .with(|context| context.borrow().as_ref().map(|(_, actor)| actor.clone()))
        .unwrap_or_else(|| json!({"type":"user","id":id("actor"),"displayName":"Cutcraft CLI"}))
}

pub fn new(name: &str) -> Value {
    let seq = id("seq");
    let timestamp = now();
    let mut project: Value =
        serde_json::from_str(include_str!("../spec/examples/minimal.cutcraft.json")).unwrap();
    let sequence = project["sequences"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    project["specVersion"] = json!(SPEC_VERSION);
    project["$schema"] = json!("https://cutstate.dev/schema/v0.1/project.schema.json");
    project["sequences"] = json!({});
    project["sequences"][&seq] = sequence;
    project["id"] = json!(id("project"));
    project["name"] = json!(name);
    project["rootSequenceId"] = json!(seq);
    project["createdAt"] = json!(timestamp);
    project["updatedAt"] = json!(timestamp);
    project["createdWith"] = application();
    project["lastModifiedWith"] = application();
    project["editors"] = json!([application()]);
    project
}
pub fn application() -> Value {
    EXECUTION_CONTEXT
        .with(|context| {
            context
                .borrow()
                .as_ref()
                .map(|(application, _)| application.clone())
        })
        .unwrap_or_else(
            || json!({"id":"cutcraft.cli","kind":"cli","version":env!("CARGO_PKG_VERSION")}),
        )
}
pub fn record_editor(project: &mut Value) {
    let client = application();
    project["lastModifiedWith"] = client.clone();
    let editors = project["editors"]
        .as_array_mut()
        .expect("validated editors");
    if !editors.contains(&client) {
        editors.push(client);
    }
}
pub fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
pub fn enabled(v: &Value) -> bool {
    v["enabled"].as_bool().unwrap_or(true)
}
pub fn normalize(v: &mut Value) -> Result<()> {
    match v {
        Value::Object(obj) => {
            if obj.len() == 2 && obj.contains_key("num") && obj.contains_key("den") {
                *v = Time::from_json(&Value::Object(obj.clone()))?.json()?;
                return Ok(());
            }
            for (key, child) in obj.iter_mut() {
                // Opaque user/plugin JSON is never interpreted or rewritten.
                if !matches!(
                    key.as_str(),
                    "metadata" | "extensions" | "parameters" | "value"
                ) {
                    normalize(child)?;
                }
            }
            if let Some(items) = obj.get_mut("items").and_then(Value::as_array_mut) {
                let mut keyed = items
                    .drain(..)
                    .map(|v| {
                        Ok((
                            Time::from_json(&v["timelineRange"]["start"])?,
                            text(&v["id"]).to_string(),
                            v,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                items.extend(keyed.into_iter().map(|(_, _, v)| v));
            }
        }
        Value::Array(a) => {
            for child in a {
                normalize(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn canonical(v: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(v)?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub fn sequence_duration(seq: &Value) -> Result<Time> {
    if seq["durationMode"] == "fixed" {
        return Time::from_json(&seq["duration"]);
    }
    let mut end = Time::zero();
    for track in array(&seq["tracks"]) {
        if !enabled(track) {
            continue;
        }
        for item in array(&track["items"]).iter().filter(|i| enabled(i)) {
            end = end.max(bounds(&item["timelineRange"])?.1);
        }
    }
    for marker in array(&seq["markers"]) {
        end =
            end.max(Time::from_json(&marker["start"])?.add(&Time::from_json(&marker["duration"])?));
    }
    Ok(end)
}
fn claim(ids: &mut BTreeSet<String>, entity: &Value) -> Result<()> {
    ensure!(
        ids.insert(text(&entity["id"]).into()),
        "DUPLICATE_ID: {}",
        entity["id"]
    );
    Ok(())
}
pub fn validate(project: &Value) -> Result<()> {
    schema::validate("project", project)?;
    ensure!(
        matches!(
            text(&project["specVersion"]),
            "0.1.0" | "0.3.0" | "0.4.0" | "0.5.0"
        ),
        "unsupported spec version"
    );
    // Reject noncanonical time rather than quietly repairing a read.
    let mut normalized = project.clone();
    normalize(&mut normalized)?;
    ensure!(
        normalized == *project,
        "NON_CANONICAL: rational values must be reduced and items sorted by start/id"
    );
    let assets = project["assets"].as_object().unwrap();
    for key in ["createdWith", "lastModifiedWith"] {
        ensure!(
            array(&project["editors"]).contains(&project[key]),
            "editors must contain {key}"
        );
    }
    let sequences = project["sequences"].as_object().unwrap();
    ensure!(
        sequences.contains_key(text(&project["rootSequenceId"])),
        "MISSING_REFERENCE: root sequence"
    );
    let mut streams = BTreeMap::new();
    for (aid, asset) in assets {
        if crate::image_sequence::is_sequence(asset) {
            crate::image_sequence::validate_asset(asset)?;
        }
        if asset["kind"] == "lut" {
            ensure!(
                array(&asset["streams"]).is_empty(),
                "LUT assets must not contain streams"
            );
        }
        let mut indexes = BTreeSet::new();
        for stream in array(&asset["streams"]) {
            ensure!(
                streams
                    .insert((aid.as_str(), text(&stream["id"])), stream)
                    .is_none(),
                "duplicate stream ID in {aid}"
            );
            ensure!(
                indexes.insert(stream["index"].as_u64().unwrap()),
                "duplicate stream index in {aid}"
            );
            if stream["kind"] == "video" {
                ensure!(
                    !stream["video"]["frameRate"].is_null(),
                    "video stream requires frameRate"
                );
            }
            let prefix = match text(&stream["kind"]) {
                "video" => "v",
                "audio" => "a",
                "image" => "i",
                "subtitle" => "s",
                _ => "d",
            };
            ensure!(
                text(&stream["id"]).starts_with(&format!("stream_{prefix}")),
                "stream ID kind mismatch"
            );
        }
    }
    let mut ids = BTreeSet::new();
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (sid, seq) in sequences {
        let mut items = BTreeMap::new();
        for track in array(&seq["tracks"]) {
            claim(&mut ids, track)?;
            let mut previous_end = Time::zero();
            for item in array(&track["items"]) {
                claim(&mut ids, item)?;
                items.insert(text(&item["id"]), (track, item));
                let (start, end) = bounds(&item["timelineRange"])?;
                if enabled(item) && track["overlapPolicy"] != "allow" {
                    ensure!(start >= previous_end, "ILLEGAL_OVERLAP: {}", track["id"]);
                    previous_end = previous_end.max(end.clone());
                }
                let tk = text(&track["kind"]);
                let ty = text(&item["type"]);
                let allowed = match tk {
                    "video" | "audio" => {
                        matches!(ty, "media" | "sequence" | "generator" | "adjustment")
                    }
                    "subtitle" => ty == "caption",
                    _ => ty == "generator",
                };
                ensure!(allowed, "TRACK_TYPE_MISMATCH: {ty} on {tk}");
                match ty {
                    "media" => {
                        let stream = streams
                            .get(&(text(&item["assetId"]), text(&item["streamId"])))
                            .context("MISSING_REFERENCE: asset/stream")?;
                        let kind = text(&stream["kind"]);
                        ensure!(
                            match kind {
                                "video" | "image" => tk == "video",
                                "audio" => tk == "audio",
                                _ => false,
                            },
                            "stream/track kind mismatch"
                        );
                        if kind == "image" {
                            ensure!(item["sourceRange"].is_null(), "image must omit sourceRange");
                        } else {
                            let (_, source_end) = bounds(&item["sourceRange"])?;
                            if !stream["duration"].is_null() {
                                ensure!(
                                    source_end <= Time::from_json(&stream["duration"])?,
                                    "SOURCE_OUT_OF_BOUNDS: {}",
                                    item["id"]
                                );
                            }
                        }
                        ensure!(
                            kind != "audio" || item["playback"]["mode"] != "hold",
                            "audio cannot hold"
                        );
                    }
                    "sequence" => {
                        let target = text(&item["sequenceId"]);
                        ensure!(
                            sequences.contains_key(target),
                            "MISSING_REFERENCE: nested sequence"
                        );
                        edges.entry(sid).or_default().push(target);
                        ensure!(
                            tk != "audio" || item["playback"]["mode"] != "hold",
                            "audio cannot hold"
                        );
                    }
                    "generator" => match text(&item["generatorType"]) {
                        "builtin.color" | "builtin.text" => {
                            ensure!(tk == "video", "visual generator requires video track")
                        }
                        "builtin.silence" => ensure!(tk == "audio", "silence requires audio track"),
                        _ => {}
                    },
                    _ => {}
                }
                let duration = end.sub(&start);
                if project["specVersion"] != "0.3.0" {
                    crate::animation::validate_item(item, tk)?;
                }
                for key in ["fadeIn", "fadeOut"] {
                    if !item["audio"][key].is_null() {
                        ensure!(
                            Time::from_json(&item["audio"][key])? <= duration,
                            "fade exceeds item duration"
                        );
                    }
                }
                let crop = &item["video"]["transform"]["crop"];
                let number = |key: &str| crop[key].as_f64().unwrap_or(0.0);
                ensure!(
                    number("left") + number("right") < 1.0
                        && number("top") + number("bottom") < 1.0,
                    "crop removes entire image"
                );
                for effect in array(&item["effects"]) {
                    claim(&mut ids, effect)?;
                    if effect["effectType"] == "builtin.gaussian-blur" {
                        ensure!(crate::capabilities::supports(text(&project["specVersion"]), crate::capabilities::Capability::GaussianBlur),
                            "PROJECT_UPGRADE_REQUIRED: gaussian-blur requires a supported project version; create a new public project (no in-place migration)");
                    }
                    match text(&effect["effectType"]) {
                        "builtin.lut"
                        | "builtin.curves"
                        | "builtin.color-grade"
                        | "builtin.gaussian-blur" => {
                            ensure!(
                                text(&track["kind"]) == "video",
                                "visual effects require a video track"
                            );
                            // Version 1.0 has static color parameters. Do not accept
                            // automation whose interpolation semantics are undefined.
                            ensure!(
                                array(&effect["automation"]).is_empty(),
                                "color effect v1 does not support automation"
                            );
                        }
                        _ => {}
                    }
                    if effect["effectType"] == "builtin.lut" {
                        let aid = text(&effect["parameters"]["assetId"]);
                        ensure!(
                            assets.get(aid).is_some_and(|a| a["kind"] == "lut"),
                            "MISSING_REFERENCE: LUT asset {aid}"
                        );
                    }
                    if effect["effectType"] == "builtin.curves" {
                        for channel in ["rgb", "r", "g", "b"] {
                            let points = array(&effect["parameters"][channel]);
                            if points.is_empty() {
                                continue;
                            }
                            ensure!(
                                points.first().unwrap()["x"] == 0
                                    || points.first().unwrap()["x"] == 0.0,
                                "curve must start at x=0"
                            );
                            ensure!(
                                points.last().unwrap()["x"] == 1
                                    || points.last().unwrap()["x"] == 1.0,
                                "curve must end at x=1"
                            );
                            ensure!(
                                points
                                    .windows(2)
                                    .all(|p| p[0]["x"].as_f64().unwrap()
                                        < p[1]["x"].as_f64().unwrap()),
                                "curve x values must increase strictly"
                            );
                        }
                    }
                    for automation in array(&effect["automation"]) {
                        let mut last = None;
                        for key in array(&automation["keyframes"]) {
                            let t = Time::from_json(&key["offset"])?;
                            ensure!(
                                t <= duration && last.as_ref().is_none_or(|p| p < &t),
                                "keyframes must be strictly increasing and within item"
                            );
                            last = Some(t);
                        }
                    }
                }
                let mut last = Time::zero();
                for word in array(&item["words"]) {
                    let (ws, we) = bounds(&word["range"])?;
                    ensure!(
                        ws >= last && we <= duration,
                        "caption words out of order or bounds"
                    );
                    last = ws;
                }
                if let Some(font) = item["style"]["fontAssetId"].as_str() {
                    ensure!(
                        assets.get(font).is_some_and(|a| a["kind"] == "font"),
                        "missing font asset"
                    );
                }
            }
        }
        let mut windows: BTreeMap<&str, Vec<(Time, Time)>> = BTreeMap::new();
        for transition in array(&seq["transitions"]) {
            claim(&mut ids, transition)?;
            let (ft, from) = items
                .get(text(&transition["fromItemId"]))
                .context("missing transition from item")?;
            let (tt, to) = items
                .get(text(&transition["toItemId"]))
                .context("missing transition to item")?;
            ensure!(
                ft["id"] == tt["id"] && from["id"] != to["id"],
                "transition must connect two items on same track"
            );
            ensure!(
                bounds(&from["timelineRange"])?.1 == bounds(&to["timelineRange"])?.0,
                "transition items must be adjacent"
            );
            let duration = Time::from_json(&transition["duration"])?;
            if project["specVersion"] != "0.3.0" {
                ensure!(
                    ft["kind"] == "video",
                    "built-in transitions require video track"
                );
                ensure!(
                    enabled(from) && enabled(to),
                    "transition requires enabled items"
                );
                let (ws, we) = crate::animation::transition_window(transition, from)?;
                ensure!(
                    ws.nonnegative() && we <= sequence_duration(seq)?,
                    "transition exceeds sequence duration"
                );
                let list = windows.entry(text(&ft["id"])).or_default();
                ensure!(
                    list.iter().all(|(s, e)| ws >= *e || we <= *s),
                    "transition windows overlap"
                );
                list.push((ws, we));
                if let Some(progress) = transition.get("progress") {
                    crate::animation::validate(progress, &duration)?;
                    let keys = array(&progress["$animation"]["keyframes"]);
                    ensure!(
                        Time::from_json(&keys[0]["offset"])? == Time::zero()
                            && keys[0]["value"].as_f64() == Some(0.)
                            && Time::from_json(&keys.last().unwrap()["offset"])? == duration
                            && keys.last().unwrap()["value"].as_f64() == Some(1.),
                        "transition progress must cover 0→1 over its duration"
                    );
                }
            }
            let before = match text(&transition["alignment"]) {
                "before" => duration.clone(),
                "center" => duration.div(&Time::integer(2))?,
                _ => Time::zero(),
            };
            let after = duration.sub(&before);
            ensure!(
                before <= Time::from_json(&from["timelineRange"]["duration"])?
                    && after <= Time::from_json(&to["timelineRange"]["duration"])?,
                "transition exceeds adjacent item duration"
            );
            check_handle(project, from, &after, false)?;
            check_handle(project, to, &before, true)?;
        }
        let links: BTreeMap<_, _> = array(&seq["links"])
            .iter()
            .map(|l| (text(&l["id"]), l))
            .collect();
        for link in array(&seq["links"]) {
            claim(&mut ids, link)?;
            for iid in array(&link["itemIds"]) {
                let (_, item) = items.get(text(iid)).context("missing link member")?;
                ensure!(
                    item["linkGroupId"] == link["id"],
                    "link member must point back to group"
                );
            }
        }
        for (_, item) in items.values() {
            if let Some(lid) = item["linkGroupId"].as_str() {
                let link = links.get(lid).context("missing link in item's sequence")?;
                ensure!(
                    array(&link["itemIds"]).contains(&item["id"]),
                    "link must include item pointing to it"
                );
            }
        }
        for marker in array(&seq["markers"]) {
            claim(&mut ids, marker)?;
        }
    }
    fn visit<'a>(
        sid: &'a str,
        edges: &BTreeMap<&'a str, Vec<&'a str>>,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        if visited.contains(sid) {
            return Ok(());
        }
        ensure!(visiting.insert(sid), "SEQUENCE_CYCLE: {sid}");
        for child in edges.get(sid).into_iter().flatten() {
            visit(child, edges, visiting, visited)?;
        }
        visiting.remove(sid);
        visited.insert(sid);
        Ok(())
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for sid in sequences.keys() {
        visit(sid, &edges, &mut visiting, &mut visited)?;
    }
    for seq in sequences.values() {
        for track in array(&seq["tracks"]) {
            for item in array(&track["items"]) {
                if item["type"] == "sequence" {
                    ensure!(
                        bounds(&item["sourceRange"])?.1
                            <= sequence_duration(&sequences[text(&item["sequenceId"])])?,
                        "nested sourceRange exceeds sequence duration"
                    );
                }
            }
        }
    }
    Ok(())
}
fn check_handle(project: &Value, item: &Value, extra: &Time, before: bool) -> Result<()> {
    if extra == &Time::zero()
        || item["sourceRange"].is_null()
        || matches!(text(&item["playback"]["mode"]), "hold" | "loop")
    {
        return Ok(());
    }
    let duration = Time::from_json(&item["timelineRange"]["duration"])?;
    let t = if before {
        Time::zero().sub(extra)
    } else {
        duration.add(extra)
    };
    let (source, _, _) = crate::animation::mapping(item, &t, false)?;
    ensure!(
        source.nonnegative(),
        "TRANSITION_HANDLE: insufficient leading source handle"
    );
    let limit = if item["type"] == "sequence" {
        Some(sequence_duration(
            &project["sequences"][text(&item["sequenceId"])],
        )?)
    } else {
        array(&project["assets"][text(&item["assetId"])]["streams"])
            .iter()
            .find(|s| s["id"] == item["streamId"])
            .and_then(|s| s.get("duration"))
            .map(Time::from_json)
            .transpose()?
    };
    if let Some(limit) = limit {
        ensure!(
            source <= limit,
            "TRANSITION_HANDLE: insufficient trailing source handle"
        );
    }
    Ok(())
}
/// Resolve active layers and exact source positions. Reverse playback uses an
/// exclusive upper boundary, including through nested sequences (no float epsilon).
pub fn sample(project: &Value, sid: &str, time: &Time) -> Result<Value> {
    validate(project)?;
    ensure!(time.nonnegative(), "sample time must be non-negative");
    let layers = sample_inner(project, sid, time, false, None, 0)?;
    Ok(json!({"sequenceId":sid,"time":time.json()?,"layers":layers}))
}
fn sample_inner(
    project: &Value,
    sid: &str,
    time: &Time,
    upper: bool,
    output_kind: Option<&str>,
    depth: usize,
) -> Result<Vec<Value>> {
    ensure!(depth < 64, "nested sequence depth exceeds 64");
    let seq = project["sequences"]
        .get(sid)
        .context("sequence not found")?;
    let duration = sequence_duration(seq)?;
    if if upper {
        time > &duration || time <= &Time::zero()
    } else {
        time >= &duration
    } {
        return Ok(vec![]);
    }
    let mut layers = vec![];
    for kind in ["video", "audio", "subtitle", "data"] {
        if output_kind.is_some_and(|k| {
            if k == "video" {
                kind != "video" && kind != "subtitle"
            } else {
                k != kind
            }
        }) {
            continue;
        }
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
            let mut active_transitions = vec![];
            for tr in array(&seq["transitions"]) {
                if let Some(from) = array(&track["items"])
                    .iter()
                    .find(|i| i["id"] == tr["fromItemId"])
                {
                    let (ws, we) = crate::animation::transition_window(tr, from)?;
                    if if upper {
                        time > &ws && time <= &we
                    } else {
                        time >= &ws && time < &we
                    } {
                        active_transitions.push((tr, ws, we));
                    }
                }
            }
            for item in array(&track["items"]) {
                let transition = active_transitions.iter().find(|(tr, _, _)| {
                    tr["fromItemId"] == item["id"] || tr["toItemId"] == item["id"]
                });
                let (start, end) = bounds(&item["timelineRange"])?;
                let active = if upper {
                    time > &start && time <= &end
                } else {
                    time >= &start && time < &end
                };
                if !enabled(item)
                    || (!active && transition.is_none())
                    || (kind == "audio" && item["audio"]["muted"] == true)
                {
                    continue;
                }
                let mut layer = json!({"trackId":track["id"],"kind":kind,"itemId":item["id"],"type":item["type"]});
                if let Some((tr, ws, we)) = transition {
                    let progress = if tr["progress"].is_null() {
                        json!(crate::animation::float(&time.sub(ws).div(&we.sub(ws))?)?)
                    } else {
                        crate::animation::evaluate(&tr["progress"], &time.sub(ws))?
                    };
                    layer["transition"] = json!({"id":tr["id"],"transitionType":tr["transitionType"],"progress":progress,"role":if tr["fromItemId"]==item["id"]{"from"}else{"to"}});
                }
                if !item["sourceRange"].is_null() {
                    let (source, source_upper, _) =
                        crate::animation::mapping(item, &time.sub(&start), upper)?;
                    layer["sourceTime"] = source.json()?;
                    layer["sourceBoundary"] = json!(if source_upper {
                        "exclusive-upper"
                    } else {
                        "inclusive-lower"
                    });
                    if item["type"] == "media" {
                        let asset = &project["assets"][text(&item["assetId"])];
                        if crate::image_sequence::is_sequence(asset) {
                            layer["imageSequenceFrames"] = crate::image_sequence::sample(
                                asset,
                                item,
                                &time.sub(&start),
                                upper,
                                transition.is_some(),
                            )?;
                        }
                    }
                    if item["type"] == "sequence" {
                        layer["children"] = json!(sample_inner(
                            project,
                            text(&item["sequenceId"]),
                            &source,
                            source_upper,
                            Some(kind),
                            depth + 1
                        )?);
                    }
                }
                layer["evaluated"] = crate::animation::evaluated(item, &time.sub(&start))?;
                layers.push(layer);
            }
        }
    }
    Ok(layers)
}
