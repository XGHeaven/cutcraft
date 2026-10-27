//! Pure transactional domain operations. The caller owns persistence and locking.
use crate::{
    id, now,
    project::{self, array, text},
    schema,
    time::{bounds, range, Time},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub fn operation(project: &Value, command: Value) -> Value {
    json!({"specVersion":project["specVersion"],"operationId":id("op"),"projectId":project["id"],"baseRevision":project["revision"],"actor":project::execution_actor(),"timestamp":now(),"command":command})
}
pub fn apply(project: &Value, operation: &Value, override_locks: bool) -> Result<(Value, Value)> {
    project::validate(project)?;
    schema::validate("operation", operation)?;
    let mut operation = operation.clone();
    project::normalize(&mut operation)?;
    ensure!(operation["projectId"] == project["id"], "PROJECT_MISMATCH");
    ensure!(
        operation["specVersion"] == project["specVersion"],
        "SPEC_VERSION_MISMATCH"
    );
    ensure!(
        operation["baseRevision"] == project["revision"],
        "REVISION_CONFLICT: expected {}, received {}",
        project["revision"],
        operation["baseRevision"]
    );
    let mut next = project.clone();
    let commands = if operation["command"]["type"] == "batch" {
        array(&operation["command"]["commands"]).to_vec()
    } else {
        vec![operation["command"].clone()]
    };
    for command in commands {
        let before = next.clone();
        execute(&mut next, &command)?;
        if !override_locks {
            check_locks(&before, &next)?;
        }
    }
    project::normalize(&mut next)?;
    next["revision"] = json!(project["revision"]
        .as_u64()
        .unwrap()
        .checked_add(1)
        .context("revision overflow")?);
    next["updatedAt"] = json!(now());
    project::record_editor(&mut next);
    project::validate(&next)?;
    let inverse = diff_commands(&next, project);
    let mut event = json!({"eventVersion":project["specVersion"],"eventId":id("event"),"operationId":operation["operationId"],"projectId":project["id"],"revisionBefore":project["revision"],"revisionAfter":next["revision"],"actor":operation["actor"],"timestamp":operation["timestamp"],"status":"applied","command":operation["command"],"inverseCommands":inverse,"diagnostics":[],"projectHashBefore":crate::store::hash(project)?,"projectHashAfter":crate::store::hash(&next)?});
    if let Some(metadata) = operation.get("metadata") {
        event["metadata"] = metadata.clone();
    }
    schema::validate("event", &event)?;
    Ok((next, event))
}
pub fn rejection(project: &Value, op: &Value, error: &anyhow::Error) -> Result<Value> {
    schema::validate("operation", op)?;
    let mut op = op.clone();
    project::normalize(&mut op)?;
    let mut event = json!({"eventVersion":project["specVersion"],"eventId":id("event"),"operationId":op["operationId"],"projectId":project["id"],"revisionBefore":project["revision"],"revisionAfter":project["revision"],"actor":op["actor"],"timestamp":op["timestamp"],"status":"rejected","command":op["command"],"diagnostics":[{"severity":"error","code":"OPERATION_REJECTED","message":format!("{error:#}")}],"projectHashBefore":crate::store::hash(project)?,"projectHashAfter":crate::store::hash(project)?});
    if let Some(metadata) = op.get("metadata") {
        event["metadata"] = metadata.clone();
    }
    schema::validate("event", &event)?;
    Ok(event)
}
/// Aggregate-level inverse commands preserve all removed entities, array order and extensions.
fn diff_commands(from: &Value, to: &Value) -> Vec<Value> {
    let mut commands = vec![];
    let mut changes = serde_json::Map::new();
    for key in ["name", "rootSequenceId", "metadata", "extensions"] {
        if from[key] != to[key] {
            changes.insert(
                key.into(),
                to.get(key).cloned().unwrap_or_else(|| json!({})),
            );
        }
    }
    for (map, singular) in [("assets", "asset"), ("sequences", "sequence")] {
        for (key, value) in to[map].as_object().unwrap() {
            if from[map].get(key) != Some(value) {
                let mut c = json!({"type":format!("{singular}.put")});
                c[format!("{singular}Id")] = json!(key);
                c[singular] = value.clone();
                commands.push(c);
            }
        }
    }
    if !changes.is_empty() {
        commands.push(json!({"type":"project.update","changes":changes}));
    }
    for (map, singular) in [("sequences", "sequence"), ("assets", "asset")] {
        for key in from[map].as_object().unwrap().keys() {
            if to[map].get(key).is_none() {
                let mut c = json!({"type":format!("{singular}.remove")});
                c[format!("{singular}Id")] = json!(key);
                commands.push(c);
            }
        }
    }
    if commands.is_empty() {
        commands.push(json!({"type":"project.update","changes":{"name":to["name"]}}));
    }
    commands
}
fn check_locks(before: &Value, after: &Value) -> Result<()> {
    for (sid, seq) in before["sequences"].as_object().unwrap() {
        for (index, track) in array(&seq["tracks"])
            .iter()
            .enumerate()
            .filter(|(_, t)| t["locked"] == true)
        {
            ensure!(
                after["sequences"][sid]["tracks"].get(index) == Some(track),
                "TRACK_LOCKED: {} (use explicit --override-locks)",
                track["id"]
            );
            let tids: BTreeSet<_> = array(&track["items"])
                .iter()
                .map(|i| text(&i["id"]))
                .collect();
            for key in ["transitions", "links"] {
                let related = |v: &Value| {
                    array(&v[key])
                        .iter()
                        .filter(|x| {
                            tids.contains(text(&x["fromItemId"]))
                                || tids.contains(text(&x["toItemId"]))
                                || array(&x["itemIds"])
                                    .iter()
                                    .any(|id| tids.contains(text(id)))
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };
                ensure!(
                    related(seq) == related(&after["sequences"][sid]),
                    "TRACK_LOCKED: linked content changed"
                );
            }
        }
    }
    Ok(())
}
fn seq_mut<'a>(p: &'a mut Value, c: &Value) -> Result<&'a mut Value> {
    p["sequences"]
        .get_mut(text(&c["sequenceId"]))
        .context("sequence not found")
}
fn tracks_mut(seq: &mut Value) -> &mut Vec<Value> {
    seq["tracks"].as_array_mut().unwrap()
}
fn locate(seq: &Value, iid: &str) -> Result<(usize, usize)> {
    for (ti, t) in array(&seq["tracks"]).iter().enumerate() {
        if let Some(ii) = array(&t["items"])
            .iter()
            .position(|i| text(&i["id"]) == iid)
        {
            return Ok((ti, ii));
        }
    }
    bail!("item not found: {iid}")
}
fn track_index(seq: &Value, tid: &str) -> Result<usize> {
    array(&seq["tracks"])
        .iter()
        .position(|t| text(&t["id"]) == tid)
        .context("track not found")
}
fn put(list: &mut Value, entity: &Value, index: Option<usize>) -> Result<()> {
    let a = list.as_array_mut().context("expected entity array")?;
    let old = a.iter().position(|e| e["id"] == entity["id"]);
    let at = index.or(old).unwrap_or(a.len());
    if let Some(old) = old {
        a.remove(old);
    }
    ensure!(at <= a.len(), "index out of bounds");
    a.insert(at, entity.clone());
    Ok(())
}
fn remove(list: &mut Value, entity_id: &str) -> Result<()> {
    let a = list.as_array_mut().unwrap();
    let at = a
        .iter()
        .position(|e| text(&e["id"]) == entity_id)
        .context("entity not found")?;
    a.remove(at);
    Ok(())
}
fn group_targets(seq: &Value, iid: &str) -> Result<Vec<String>> {
    let (ti, ii) = locate(seq, iid)?;
    let item = &seq["tracks"][ti]["items"][ii];
    if let Some(lid) = item["linkGroupId"].as_str() {
        let group = array(&seq["links"])
            .iter()
            .find(|l| text(&l["id"]) == lid)
            .context("link not found")?;
        return Ok(array(&group["itemIds"])
            .iter()
            .map(|id| text(id).into())
            .collect());
    }
    Ok(vec![iid.into()])
}
fn cleanup(seq: &mut Value, removed: &BTreeSet<String>) -> Result<()> {
    seq["transitions"].as_array_mut().unwrap().retain(|t| {
        !removed.contains(text(&t["fromItemId"])) && !removed.contains(text(&t["toItemId"]))
    });
    let mut dead_links = BTreeSet::new();
    for link in seq["links"].as_array_mut().unwrap().iter_mut() {
        link["itemIds"]
            .as_array_mut()
            .unwrap()
            .retain(|id| !removed.contains(text(id)));
        if array(&link["itemIds"]).len() < 2 {
            dead_links.insert(text(&link["id"]).to_string());
        }
    }
    seq["links"]
        .as_array_mut()
        .unwrap()
        .retain(|l| !dead_links.contains(text(&l["id"])));
    for track in tracks_mut(seq) {
        for item in track["items"].as_array_mut().unwrap() {
            if dead_links.contains(text(&item["linkGroupId"])) {
                item.as_object_mut().unwrap().remove("linkGroupId");
            }
        }
    }
    Ok(())
}
fn execute(p: &mut Value, c: &Value) -> Result<()> {
    let ty = text(&c["type"]);
    match ty {
        "project.update" => {
            for (k, v) in c["changes"].as_object().unwrap() {
                p[k] = v.clone();
            }
        }
        "asset.put" | "sequence.put" => {
            let entity = ty.split('.').next().unwrap();
            let map = format!("{entity}s");
            p[&map][text(&c[format!("{entity}Id")])] = c[entity].clone();
        }
        "asset.remove" | "sequence.remove" => {
            let entity = ty.split('.').next().unwrap();
            let target = text(&c[format!("{entity}Id")]);
            let map = format!("{entity}s");
            ensure!(
                p[&map].as_object_mut().unwrap().remove(target).is_some(),
                "entity not found"
            );
            if c["cascade"] == true {
                for seq in p["sequences"].as_object_mut().unwrap().values_mut() {
                    let mut removed = BTreeSet::new();
                    for track in tracks_mut(seq) {
                        track["items"].as_array_mut().unwrap().retain(|i| {
                            let matches = if entity == "asset" {
                                text(&i["assetId"]) == target
                                    || text(&i["style"]["fontAssetId"]) == target
                                    || array(&i["effects"]).iter().any(|e| {
                                        e["effectType"] == "builtin.lut"
                                            && text(&e["parameters"]["assetId"]) == target
                                    })
                            } else {
                                text(&i["sequenceId"]) == target
                            };
                            if matches {
                                removed.insert(text(&i["id"]).into());
                            }
                            !matches
                        });
                    }
                    cleanup(seq, &removed)?;
                }
            }
        }
        "track.put" => {
            put(
                &mut seq_mut(p, c)?["tracks"],
                &c["track"],
                c["index"].as_u64().map(|v| v as usize),
            )?;
        }
        "track.remove" => {
            let seq = seq_mut(p, c)?;
            let ti = track_index(seq, text(&c["trackId"]))?;
            let removed = array(&seq["tracks"][ti]["items"])
                .iter()
                .map(|i| text(&i["id"]).into())
                .collect();
            tracks_mut(seq).remove(ti);
            if c["cascade"] == true {
                cleanup(seq, &removed)?;
            }
        }
        "track.reorder" => {
            let seq = seq_mut(p, c)?;
            let old = array(&seq["tracks"]);
            ensure!(
                array(&c["trackIds"]).len() == old.len(),
                "trackIds must include every track exactly once"
            );
            let next = array(&c["trackIds"])
                .iter()
                .map(|id| {
                    old.iter()
                        .find(|t| t["id"] == *id)
                        .cloned()
                        .context("unknown track ID")
                })
                .collect::<Result<Vec<_>>>()?;
            seq["tracks"] = json!(next);
        }
        "item.put" => {
            let seq = seq_mut(p, c)?;
            let ti = track_index(seq, text(&c["trackId"]))?;
            // put can replace in place; cross-track placement uses item.move.
            if let Ok((old, _)) = locate(seq, text(&c["item"]["id"])) {
                ensure!(old == ti, "use item.move to change track");
            }
            put(&mut seq["tracks"][ti]["items"], &c["item"], None)?;
        }
        "item.remove" => {
            let seq = seq_mut(p, c)?;
            let targets = group_targets(seq, text(&c["itemId"]))?;
            ensure!(
                matches!(text(&c["ripple"]), "" | "none" | "remove-gap"),
                "item.remove supports none or remove-gap ripple"
            );
            if c["ripple"] == "remove-gap" {
                close_gaps(seq, &targets)?;
            }
            for iid in &targets {
                let (ti, ii) = locate(seq, iid)?;
                seq["tracks"][ti]["items"]
                    .as_array_mut()
                    .unwrap()
                    .remove(ii);
            }
            if c["cascade"] == true {
                cleanup(seq, &targets.into_iter().collect())?;
            }
            // Without cascade, final validation rejects still-referenced entities.
        }
        "item.move" | "item.trim" => edit_items(seq_mut(p, c)?, c)?,
        "item.split" => split(seq_mut(p, c)?, c)?,
        "link.put" => {
            let seq = seq_mut(p, c)?;
            let link = &c["link"];
            unlink(seq, text(&link["id"]));
            for iid in array(&link["itemIds"]) {
                let (ti, ii) = locate(seq, text(iid))?;
                let item = &mut seq["tracks"][ti]["items"][ii];
                ensure!(
                    item["linkGroupId"].is_null(),
                    "item already belongs to another link group"
                );
                item["linkGroupId"] = link["id"].clone();
            }
            put(&mut seq["links"], link, None)?;
        }
        "link.remove" => {
            let seq = seq_mut(p, c)?;
            unlink(seq, text(&c["linkId"]));
            remove(&mut seq["links"], text(&c["linkId"]))?;
        }
        "transition.put" | "marker.put" => {
            let name = ty.split('.').next().unwrap();
            put(&mut seq_mut(p, c)?[format!("{name}s")], &c[name], None)?;
        }
        "transition.remove" | "marker.remove" => {
            let name = ty.split('.').next().unwrap();
            remove(
                &mut seq_mut(p, c)?[format!("{name}s")],
                text(&c[format!("{name}Id")]),
            )?;
        }
        _ => bail!("unsupported command: {ty}"),
    }
    Ok(())
}
fn unlink(seq: &mut Value, lid: &str) {
    for track in tracks_mut(seq) {
        for item in track["items"].as_array_mut().unwrap() {
            if text(&item["linkGroupId"]) == lid {
                item.as_object_mut().unwrap().remove("linkGroupId");
            }
        }
    }
}
fn edit_items(seq: &mut Value, c: &Value) -> Result<()> {
    let targets = group_targets(seq, text(&c["itemId"]))?;
    let (ti, ii) = locate(seq, text(&c["itemId"]))?;
    let primary = seq["tracks"][ti]["items"][ii].clone();
    let (ps, pe) = bounds(&primary["timelineRange"])?;
    let trim = c["type"] == "item.trim";
    let (new_start, new_end) = if trim {
        bounds(&c["timelineRange"])?
    } else {
        let s = if c["timelineStart"].is_null() {
            ps.clone()
        } else {
            Time::from_json(&c["timelineStart"])?
        };
        let e = s.add(&pe.sub(&ps));
        (s, e)
    };
    let delta_start = new_start.sub(&ps);
    let delta_end = new_end.sub(&pe);
    let mut moved = vec![];
    for iid in &targets {
        let (ti, ii) = locate(seq, iid)?;
        let mut item = seq["tracks"][ti]["items"]
            .as_array_mut()
            .unwrap()
            .remove(ii);
        let (s, e) = bounds(&item["timelineRange"])?;
        let ns = s.add(&delta_start);
        let ne = e.add(&delta_end);
        let dest = if iid == text(&c["itemId"]) && !c["toTrackId"].is_null() {
            track_index(seq, text(&c["toTrackId"]))?
        } else {
            ti
        };
        let curve_trim =
            trim && (crate::animation::animated(&item) || item["playback"]["mode"] == "remap");
        if curve_trim {
            let sliced = slice(&item, &ns, &ne, text(&item["id"]))?;
            if iid == text(&c["itemId"]) && !c["sourceRange"].is_null() {
                ensure!(c["sourceRange"]==sliced["sourceRange"], "curve-preserving trim sourceRange mismatch; omit source range or use item.put for a new mapping");
            }
            item = sliced;
        }
        if trim && !curve_trim {
            if iid == text(&c["itemId"]) {
                if !item["sourceRange"].is_null() {
                    ensure!(
                        !c["sourceRange"].is_null(),
                        "timed item trim requires complete sourceRange"
                    );
                }
                if let Some(sr) = c.get("sourceRange") {
                    item["sourceRange"] = sr.clone();
                }
            } else if !item["sourceRange"].is_null() {
                ensure!(
                    matches!(text(&item["playback"]["mode"]), "" | "fit"),
                    "linked trim requires fit playback"
                );
                let (ss, se) = bounds(&item["sourceRange"])?;
                let rate = se.sub(&ss).div(&e.sub(&s))?;
                let reverse = item["playback"]["reverse"] == true;
                let (a, b) = if reverse {
                    (
                        ss.sub(&delta_end.mul(&rate)),
                        se.sub(&delta_start.mul(&rate)),
                    )
                } else {
                    (
                        ss.add(&delta_start.mul(&rate)),
                        se.add(&delta_end.mul(&rate)),
                    )
                };
                item["sourceRange"] = range(&a, &b.sub(&a))?;
            }
        }
        item["timelineRange"] = range(&ns, &ne.sub(&ns))?;
        moved.push((ti, dest, s, e, ns, ne, item));
    }
    // A linked group can touch multiple tracks; shift each downstream group once.
    let mut shifted = BTreeSet::new();
    for (old, dest, s, e, ns, ne, item) in &moved {
        match text(&c["ripple"]) {
            "none" => {}
            "insert" => {
                let delta = if trim && old == dest {
                    ne.sub(e)
                } else {
                    ne.sub(ns)
                };
                let boundary = if trim && old == dest { e } else { ns };
                ripple_shift_once(seq, *dest, boundary, &delta, &targets, &mut shifted)?;
            }
            "remove-gap" => {
                let delta = if trim { ne.sub(e) } else { s.sub(e) };
                ripple_shift_once(seq, *old, e, &delta, &targets, &mut shifted)?;
            }
            "overwrite" => overwrite(seq, *dest, ns, ne, &targets)?,
            _ => bail!("unknown ripple mode"),
        }
        let _ = item;
    }
    for (_, dest, _, _, _, _, item) in moved {
        seq["tracks"][dest]["items"]
            .as_array_mut()
            .unwrap()
            .push(item);
    }
    Ok(())
}
fn close_gaps(seq: &mut Value, targets: &[String]) -> Result<()> {
    let mut shifts: BTreeMap<String, Time> = BTreeMap::new();
    for track in array(&seq["tracks"]) {
        let mut intervals = vec![];
        for item in array(&track["items"]) {
            if targets.iter().any(|id| id == text(&item["id"])) {
                intervals.push(bounds(&item["timelineRange"])?);
            }
        }
        intervals.sort();
        let mut merged: Vec<(Time, Time)> = vec![];
        for (start, end) in intervals {
            if let Some(last) = merged.last_mut() {
                if start <= last.1 {
                    last.1 = last.1.clone().max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }
        for item in array(&track["items"]) {
            if targets.iter().any(|id| id == text(&item["id"])) {
                continue;
            }
            let start = Time::from_json(&item["timelineRange"]["start"])?;
            let delta = merged
                .iter()
                .filter(|(_, end)| end <= &start)
                .fold(Time::zero(), |acc, (s, e)| acc.sub(&e.sub(s)));
            if delta == Time::zero() {
                continue;
            }
            for iid in group_targets(seq, text(&item["id"]))? {
                if targets.contains(&iid) {
                    continue;
                }
                if let Some(old) = shifts.insert(iid, delta.clone()) {
                    ensure!(
                        old == delta,
                        "AMBIGUOUS_LINK_RIPPLE: linked tracks require different shifts"
                    );
                }
            }
        }
    }
    for (iid, delta) in shifts {
        let (ti, ii) = locate(seq, &iid)?;
        let start = &mut seq["tracks"][ti]["items"][ii]["timelineRange"]["start"];
        let moved = Time::from_json(start)?.add(&delta);
        ensure!(moved.nonnegative(), "ripple creates negative start");
        *start = moved.json()?;
    }
    Ok(())
}
fn ripple_shift_once(
    seq: &mut Value,
    ti: usize,
    boundary: &Time,
    delta: &Time,
    exclude: &[String],
    shifted: &mut BTreeSet<String>,
) -> Result<()> {
    let mut selected = BTreeSet::new();
    for item in array(&seq["tracks"][ti]["items"]) {
        let iid = text(&item["id"]);
        if !exclude.iter().any(|x| x == iid)
            && !shifted.contains(iid)
            && Time::from_json(&item["timelineRange"]["start"])? >= *boundary
        {
            selected.extend(group_targets(seq, iid)?);
        }
    }
    for iid in selected {
        if shifted.insert(iid.clone()) && !exclude.contains(&iid) {
            let (ti, ii) = locate(seq, &iid)?;
            let start = &mut seq["tracks"][ti]["items"][ii]["timelineRange"]["start"];
            let value = Time::from_json(start)?.add(delta);
            ensure!(value.nonnegative(), "ripple creates negative start");
            *start = value.json()?;
        }
    }
    Ok(())
}
/// Slice fit-mapped media exactly. Complex looping/automation cannot be losslessly
/// represented by the v0.3.0 Item schema after an arbitrary cut, so reject explicitly.
fn slice(item: &Value, start: &Time, end: &Time, new_id: &str) -> Result<Value> {
    let (old_start, old_end) = bounds(&item["timelineRange"])?;
    ensure!(
        start >= &old_start && end <= &old_end && end > start,
        "slice outside item"
    );
    let mut result = item.clone();
    result["id"] = json!(new_id);
    result["timelineRange"] = range(start, &end.sub(start))?;
    let offset = start.sub(&old_start);
    let duration = end.sub(start);
    if !item["sourceRange"].is_null() {
        let mode = text(&item["playback"]["mode"]);
        ensure!(mode!="loop","UNREPRESENTABLE_EDIT: split/overwrite of looping source needs a phase field; trim explicitly instead");
        if mode == "remap" {
            result["playback"]["timeMap"] = crate::animation::slice(
                &item["playback"]["timeMap"],
                &offset,
                &offset.add(&duration),
            )?;
        } else if mode != "hold" {
            let (ss, se) = bounds(&item["sourceRange"])?;
            let rate = se.sub(&ss).div(&old_end.sub(&old_start))?;
            let source_start = if item["playback"]["reverse"] == true {
                se.sub(&end.sub(&old_start).mul(&rate))
            } else {
                ss.add(&offset.mul(&rate))
            };
            result["sourceRange"] = range(&source_start, &duration.mul(&rate))?;
        }
    }
    crate::animation::slots(&mut result, |v| {
        *v = crate::animation::slice(v, &offset, &offset.add(&duration))?;
        Ok(())
    })?;
    if let Some(words) = result.get_mut("words").and_then(Value::as_array_mut) {
        let mut sliced = vec![];
        for word in words.iter() {
            let (s, e) = bounds(&word["range"])?;
            let a = s.max(offset.clone());
            let b = e.min(offset.add(&duration));
            if b > a {
                let mut w = word.clone();
                w["range"] = range(&a.sub(&offset), &b.sub(&a))?;
                sliced.push(w);
            }
        }
        *words = sliced;
    }
    for effect in result
        .get_mut("effects")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        ensure!(
            array(&effect["automation"]).is_empty(),
            "UNREPRESENTABLE_EDIT: slicing automated effects needs plugin-specific interpolation"
        );
        if new_id != text(&item["id"]) {
            effect["id"] = json!(id("effect"));
        }
    }
    // Piecewise fades need new curves to preserve sound, which v0.3.0 does not define.
    for key in ["fadeIn", "fadeOut"] {
        if let Some(fade) = item["audio"].get(key) {
            ensure!(
                Time::from_json(fade)? == Time::zero(),
                "UNREPRESENTABLE_EDIT: slicing fades needs automation; trim explicitly instead"
            );
        }
    }
    Ok(result)
}
fn overwrite(
    seq: &mut Value,
    ti: usize,
    start: &Time,
    end: &Time,
    exclude: &[String],
) -> Result<()> {
    let old = std::mem::take(seq["tracks"][ti]["items"].as_array_mut().unwrap());
    let mut next = vec![];
    let mut removed = BTreeSet::new();
    for item in old {
        let iid = text(&item["id"]);
        let (s, e) = bounds(&item["timelineRange"])?;
        if exclude.iter().any(|id| id == iid) || e <= *start || s >= *end {
            next.push(item);
            continue;
        }
        ensure!(
            item["linkGroupId"].is_null(),
            "overwrite of linked items requires explicit unlink to preserve sync"
        );
        ensure!(
            !array(&seq["transitions"])
                .iter()
                .any(|t| text(&t["fromItemId"]) == iid || text(&t["toItemId"]) == iid),
            "overwrite would change a transition; remove transition explicitly"
        );
        let left = s < *start;
        let right = e > *end;
        if left {
            next.push(slice(&item, &s, start, iid)?);
        }
        if right {
            next.push(slice(
                &item,
                end,
                &e,
                &if left { id("item") } else { iid.into() },
            )?);
        }
        if !left && !right {
            removed.insert(iid.to_string());
        }
    }
    seq["tracks"][ti]["items"] = json!(next);
    cleanup(seq, &removed)?;
    Ok(())
}
fn split(seq: &mut Value, c: &Value) -> Result<()> {
    let target = text(&c["itemId"]);
    let targets = group_targets(seq, target)?;
    let (pt, pi) = locate(seq, target)?;
    let primary = seq["tracks"][pt]["items"][pi].clone();
    let at = Time::from_json(&c["at"])?;
    let delta = at.sub(&Time::from_json(&primary["timelineRange"]["start"])?);
    ensure!(
        c["leftItemId"] != c["rightItemId"]
            && c["leftItemId"] != c["itemId"]
            && c["rightItemId"] != c["itemId"],
        "split requires two fresh IDs"
    );
    let linked = targets.len() > 1;
    let left_link = id("link");
    let right_link = id("link");
    let mut left_ids = vec![];
    let mut right_ids = vec![];
    let old_link = array(&seq["links"])
        .iter()
        .find(|l| l["id"] == primary["linkGroupId"])
        .cloned();
    for iid in targets {
        let (ti, ii) = locate(seq, &iid)?;
        let item = seq["tracks"][ti]["items"]
            .as_array_mut()
            .unwrap()
            .remove(ii);
        let (s, e) = bounds(&item["timelineRange"])?;
        let cut = s.add(&delta);
        ensure!(
            cut > s && cut < e,
            "split point must be strictly inside all linked items"
        );
        let lid = if iid == target {
            text(&c["leftItemId"]).into()
        } else {
            id("item")
        };
        let rid = if iid == target {
            text(&c["rightItemId"]).into()
        } else {
            id("item")
        };
        let mut left = slice(&item, &s, &cut, &lid)?;
        let mut right = slice(&item, &cut, &e, &rid)?;
        if linked {
            left["linkGroupId"] = json!(left_link);
            right["linkGroupId"] = json!(right_link);
        }
        for transition in seq["transitions"].as_array_mut().unwrap() {
            if text(&transition["fromItemId"]) == iid {
                transition["fromItemId"] = json!(rid);
            }
            if text(&transition["toItemId"]) == iid {
                transition["toItemId"] = json!(lid);
            }
        }
        left_ids.push(lid);
        right_ids.push(rid);
        seq["tracks"][ti]["items"]
            .as_array_mut()
            .unwrap()
            .extend([left, right]);
    }
    if let Some(old) = old_link {
        remove(&mut seq["links"], text(&old["id"]))?;
        for (lid, members) in [(left_link, left_ids), (right_link, right_ids)] {
            let mut new = old.clone();
            new["id"] = json!(lid);
            new["itemIds"] = json!(members);
            seq["links"].as_array_mut().unwrap().push(new);
        }
    }
    Ok(())
}
