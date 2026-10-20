//! Read-only snapshot discovery. Times are local to the containing sequence;
//! usage means a stored reference, including disabled and non-root items.
use crate::{
    media,
    project::{array, text},
    time::{bounds, Time},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[derive(Default)]
pub struct AssetFilter {
    pub query: Option<String>,
    pub name: Option<String>,
    pub path: Option<String>,
    pub kind: Option<String>,
    pub source_type: Option<String>,
    pub tag: Option<String>,
    pub unused: bool,
}
#[derive(Default)]
pub struct ItemFilter {
    pub query: Option<String>,
    pub asset: Option<String>,
    pub sequence: Option<String>,
    pub track: Option<String>,
    pub at: Option<Time>,
}
fn matches(filter: Option<&str>, values: &[&str]) -> bool {
    filter.is_none_or(|q| {
        let q = q.to_lowercase();
        values.iter().any(|v| v.to_lowercase().contains(&q))
    })
}
fn matches_path(filter: Option<&str>, values: &[&str]) -> bool {
    if cfg!(windows) {
        let normalized = values
            .iter()
            .map(|v| v.replace('\\', "/"))
            .collect::<Vec<_>>();
        let filter = filter.map(|q| q.replace('\\', "/"));
        matches(
            filter.as_deref(),
            &normalized.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    } else {
        matches(filter, values)
    }
}
fn result(p: &Value, key: &str, rows: Vec<Value>) -> Value {
    let mut v = json!({"ok":true,"revision":p["revision"],"count":rows.len()});
    v[key] = json!(rows);
    v
}
fn asset<'a>(p: &'a Value, id: &str) -> Result<&'a Value> {
    p["assets"].get(id).context("ASSET_NOT_FOUND")
}
fn sequence<'a>(p: &'a Value, id: &str) -> Result<&'a Value> {
    p["sequences"].get(id).context("SEQUENCE_NOT_FOUND")
}
fn asset_path(dir: &Path, a: &Value) -> Option<String> {
    // URL resolution decodes Chinese/spaces and resolves relative paths without
    // checking file existence, so offline assets remain searchable.
    media::local_path(dir, text(&a["uri"]))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
fn references(item: &Value) -> Vec<Value> {
    let mut refs = vec![];
    if item["type"] == "media" {
        refs.push(json!({"assetId":item["assetId"],"role":"media","field":"assetId","streamId":item["streamId"]}));
    }
    let font = match text(&item["type"]) {
        "caption" => Some((&item["style"]["fontAssetId"], "style.fontAssetId")),
        "generator" if item["generatorType"] == "builtin.text" => {
            Some((&item["parameters"]["fontAssetId"], "parameters.fontAssetId"))
        }
        _ => None,
    };
    if let Some((Value::String(id), field)) = font {
        refs.push(json!({"assetId":id,"role":"font","field":field}));
    }
    for (index, effect) in array(&item["effects"]).iter().enumerate() {
        if effect["effectType"] == "builtin.lut" {
            refs.push(json!({"assetId":effect["parameters"]["assetId"],"role":"lut","field":format!("effects.{index}.parameters.assetId"),"effectId":effect["id"]}));
        }
    }
    refs
}
struct Location<'a> {
    sid: &'a str,
    seq: &'a Value,
    track: &'a Value,
    item: &'a Value,
}
fn locations(p: &Value) -> Vec<Location<'_>> {
    let mut rows = vec![];
    for (sid, seq) in p["sequences"].as_object().unwrap() {
        for track in array(&seq["tracks"]) {
            for item in array(&track["items"]) {
                rows.push(Location {
                    sid,
                    seq,
                    track,
                    item,
                });
            }
        }
    }
    rows
}
fn location(p: &Value, loc: &Location<'_>) -> Value {
    json!({"sequenceId":loc.sid,"sequenceName":loc.seq["name"],"isRootSequence":p["rootSequenceId"]==loc.sid,
        "trackId":loc.track["id"],"trackName":loc.track["name"],"trackKind":loc.track["kind"],
        "itemId":loc.item["id"],"name":loc.item["name"],"type":loc.item["type"],
        "timelineRange":loc.item["timelineRange"],"timeReference":"sequence",
        "sourceRange":loc.item["sourceRange"],"assetId":loc.item["assetId"],"streamId":loc.item["streamId"],
        "referencedSequenceId":loc.item["sequenceId"],"references":references(loc.item),
        "itemEnabled":loc.item["enabled"]!=false,"trackEnabled":loc.track["enabled"]!=false})
}
fn counts(p: &Value) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for loc in locations(p) {
        let ids: std::collections::BTreeSet<_> = references(loc.item)
            .iter()
            .map(|r| text(&r["assetId"]).to_owned())
            .collect();
        for id in ids {
            *counts.entry(id).or_default() += 1;
        }
    }
    counts
}
fn asset_row(dir: &Path, aid: &str, a: &Value, count: usize) -> Value {
    let mut row = json!({"assetId":aid,"name":a["name"],"kind":a["kind"],"uri":a["uri"],"path":asset_path(dir,a),
        "sourceType":source_type(a),"storage":a["storage"],"availability":a["availability"],"tags":a["tags"],"usageCount":count,"streams":a["streams"]});
    if crate::image_sequence::is_sequence(a) {
        row["imageSequence"] = sequence_summary(a);
    }
    row
}
fn source_type(a: &Value) -> &'static str {
    if crate::image_sequence::is_sequence(a) {
        "image-sequence"
    } else {
        "file"
    }
}
fn sequence_summary(a: &Value) -> Value {
    json!({"frameCount":array(&a["source"]["frames"]).len(),"fileCount":array(&a["source"]["frames"]).iter().map(|f|text(&f["uri"])).collect::<std::collections::BTreeSet<_>>().len(),"frameRate":a["streams"][0]["video"]["frameRate"],"duration":a["streams"][0]["duration"]})
}
fn frame_matches(dir: &Path, a: &Value, q: Option<&str>) -> (usize, Value) {
    let Some(q) = q else { return (0, Value::Null) };
    let mut count = 0;
    let mut first = Value::Null;
    for (index, f) in array(&a["source"]["frames"]).iter().enumerate() {
        let path = crate::image_sequence::frame_path(dir, a, f)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if matches_path(Some(q), &[text(&f["uri"]), &path]) {
            count += 1;
            if first.is_null() {
                first = json!({"index":index,"uri":f["uri"],"path":path});
            }
        }
    }
    (count, first)
}
pub fn assets(p: &Value, dir: &Path, f: &AssetFilter) -> Value {
    let counts = counts(p);
    let rows = p["assets"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(aid, a)| {
            let path = asset_path(dir, a).unwrap_or_default();
            let usage = *counts.get(aid).unwrap_or(&0);
            let mut values = vec![
                aid.as_str(),
                text(&a["name"]),
                text(&a["uri"]),
                path.as_str(),
            ];
            values.extend(array(&a["tags"]).iter().filter_map(Value::as_str));
            let (path_count, path_first) = frame_matches(dir, a, f.path.as_deref());
            let (query_count, query_first) = frame_matches(dir, a, f.query.as_deref());
            ((matches(f.query.as_deref(), &values)
                || matches_path(f.query.as_deref(), &[text(&a["uri"]), &path])
                || query_count > 0)
                && matches(f.name.as_deref(), &[text(&a["name"])])
                && (matches_path(f.path.as_deref(), &[text(&a["uri"]), &path]) || path_count > 0)
                && f.kind.as_ref().is_none_or(|k| a["kind"] == *k)
                && f.source_type.as_ref().is_none_or(|k| source_type(a) == k)
                && f.tag.as_ref().is_none_or(|tag| {
                    array(&a["tags"]).iter().any(|t| {
                        t.as_str()
                            .is_some_and(|t| t.to_lowercase() == tag.to_lowercase())
                    })
                })
                && (!f.unused || usage == 0))
                .then(|| {
                    let mut row = asset_row(dir, aid, a, usage);
                    if path_count > 0 || query_count > 0 {
                        row["frameMatchCount"] = json!(if f.path.is_some() {
                            path_count
                        } else {
                            query_count
                        });
                        row["firstFrameMatch"] = if f.path.is_some() {
                            path_first
                        } else {
                            query_first
                        };
                    }
                    row
                })
        })
        .collect();
    result(p, "assets", rows)
}
pub fn asset_info(p: &Value, dir: &Path, id: &str) -> Result<Value> {
    asset_info_summary(p, dir, id, false)
}
pub fn asset_info_summary(p: &Value, dir: &Path, id: &str, summary: bool) -> Result<Value> {
    let a = asset(p, id)?;
    let mut out = json!({"ok":true,"revision":p["revision"],"assetId":id,"asset":a,"path":asset_path(dir,a),"usageCount":counts(p).get(id).copied().unwrap_or(0)});
    if crate::image_sequence::is_sequence(a) {
        out["imageSequence"] = sequence_summary(a);
        if summary {
            out["asset"]["source"]
                .as_object_mut()
                .unwrap()
                .remove("frames");
        }
    }
    Ok(out)
}
pub fn asset_frames(p: &Value, dir: &Path, id: &str, offset: usize, limit: usize) -> Result<Value> {
    let a = asset(p, id)?;
    ensure!(
        crate::image_sequence::is_sequence(a),
        "ASSET_IS_NOT_IMAGE_SEQUENCE"
    );
    ensure!((1..=1000).contains(&limit), "PAGE_LIMIT_INVALID: 1..1000");
    let fps = Time::from_json(&a["streams"][0]["video"]["frameRate"])?;
    let frames = array(&a["source"]["frames"]);
    let mut rows = vec![];
    for (i, f) in frames.iter().enumerate().skip(offset).take(limit) {
        rows.push(json!({"index":i,"uri":f["uri"],"path":crate::image_sequence::frame_path(dir,a,f)?,"fingerprint":f["fingerprint"],"sourceRange":crate::time::range(&Time::integer(i as i64).div(&fps)?,&Time::integer(1).div(&fps)?)?}));
    }
    let next = offset.saturating_add(rows.len());
    let mut out = result(p, "frames", rows);
    out["assetId"] = json!(id);
    out["total"] = json!(frames.len());
    out["offset"] = json!(offset);
    out["limit"] = json!(limit);
    out["nextOffset"] = if next < frames.len() {
        json!(next)
    } else {
        Value::Null
    };
    Ok(out)
}
pub fn asset_usages(p: &Value, id: &str) -> Result<Value> {
    asset(p, id)?;
    let rows = locations(p)
        .iter()
        .filter_map(|loc| {
            let refs: Vec<_> = references(loc.item)
                .into_iter()
                .filter(|r| r["assetId"] == id)
                .collect();
            if refs.is_empty() {
                return None;
            }
            let mut row = location(p, loc);
            row["references"] = json!(refs);
            Some(row)
        })
        .collect();
    let mut out = result(p, "usages", rows);
    out["assetId"] = json!(id);
    Ok(out)
}
pub fn items(p: &Value, dir: &Path, f: &ItemFilter) -> Result<Value> {
    if let Some(id) = &f.asset {
        asset(p, id)?;
    }
    if let Some(id) = &f.sequence {
        sequence(p, id)?;
    }
    if let Some(id) = &f.track {
        ensure!(
            p["sequences"].as_object().unwrap().iter().any(|(sid, s)| f
                .sequence
                .as_ref()
                .is_none_or(|v| v == sid)
                && array(&s["tracks"]).iter().any(|t| t["id"] == *id)),
            "TRACK_NOT_FOUND"
        );
    }
    ensure!(
        f.at.is_none() || f.sequence.is_some(),
        "--at requires --sequence; time is local to that sequence"
    );
    let mut rows = vec![];
    for loc in locations(p) {
        if f.sequence.as_ref().is_some_and(|s| s != loc.sid)
            || f.track.as_ref().is_some_and(|t| loc.track["id"] != *t)
        {
            continue;
        }
        let refs = references(loc.item);
        if f.asset
            .as_ref()
            .is_some_and(|id| !refs.iter().any(|r| r["assetId"] == *id))
        {
            continue;
        }
        if let Some(at) = &f.at {
            let (start, end) = bounds(&loc.item["timelineRange"])?;
            if at < &start || at >= &end {
                continue;
            }
        }
        let mut values = vec![
            text(&loc.item["id"]).to_owned(),
            text(&loc.item["name"]).to_owned(),
            text(&loc.item["text"]).to_owned(),
        ];
        if loc.item["generatorType"] == "builtin.text" {
            values.push(text(&loc.item["parameters"]["text"]).to_owned());
        }
        values.extend(
            array(&loc.item["tags"])
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        let mut reference_match = false;
        for r in &refs {
            let aid = text(&r["assetId"]);
            if let Some(a) = p["assets"].get(aid) {
                reference_match |= frame_matches(dir, a, f.query.as_deref()).0 > 0
                    || matches_path(
                        f.query.as_deref(),
                        &[text(&a["uri"]), &asset_path(dir, a).unwrap_or_default()],
                    );
                values.extend([
                    aid.to_owned(),
                    text(&a["name"]).to_owned(),
                    text(&a["uri"]).to_owned(),
                    asset_path(dir, a).unwrap_or_default(),
                ]);
            }
        }
        if loc.item["type"] == "sequence" {
            values.push(text(&p["sequences"][text(&loc.item["sequenceId"])]["name"]).to_owned());
        }
        if reference_match
            || matches(
                f.query.as_deref(),
                &values.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        {
            rows.push(location(p, &loc));
        }
    }
    Ok(result(p, "items", rows))
}
pub fn item_info(p: &Value, id: &str) -> Result<Value> {
    let locs = locations(p);
    let loc = locs
        .iter()
        .find(|l| l.item["id"] == id)
        .context("ITEM_NOT_FOUND")?;
    let mut out = location(p, loc);
    out["ok"] = json!(true);
    out["revision"] = p["revision"].clone();
    out["item"] = loc.item.clone();
    Ok(out)
}
pub fn sequences(p: &Value, q: Option<&str>) -> Value {
    result(p,"sequences",p["sequences"].as_object().unwrap().iter()
        .filter(|(id,s)|matches(q,&[id,text(&s["name"])]))
        .map(|(id,s)|json!({"sequenceId":id,"name":s["name"],"isRootSequence":p["rootSequenceId"]==*id,"format":s["format"],"trackCount":array(&s["tracks"]).len()})).collect())
}
pub fn tracks(p: &Value, sid: Option<&str>, q: Option<&str>) -> Result<Value> {
    if let Some(sid) = sid {
        sequence(p, sid)?;
    }
    let mut rows = vec![];
    for (id, s) in p["sequences"].as_object().unwrap() {
        if sid.is_some_and(|v| v != id) {
            continue;
        }
        for t in array(&s["tracks"]) {
            if matches(q, &[text(&t["id"]), text(&t["name"])]) {
                rows.push(json!({"sequenceId":id,"sequenceName":s["name"],"trackId":t["id"],"name":t["name"],"kind":t["kind"],"itemCount":array(&t["items"]).len()}));
            }
        }
    }
    Ok(result(p, "tracks", rows))
}
pub fn sequence_usages(p: &Value, id: &str) -> Result<Value> {
    sequence(p, id)?;
    let rows = locations(p)
        .iter()
        .filter(|loc| loc.item["type"] == "sequence" && loc.item["sequenceId"] == id)
        .map(|loc| location(p, loc))
        .collect();
    let mut out = result(p, "usages", rows);
    out["sequenceId"] = json!(id);
    Ok(out)
}
