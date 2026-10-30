//! Exact sequence-range edits compiled to an ordinary aggregate command.
//! Generated IDs are materialized in sequence.put, so event replay is deterministic.
use crate::{
    id,
    project::{self, array, text},
    time::{bounds, range, Time},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub struct CutOptions<'a> {
    pub start: &'a Time,
    pub duration: &'a Time,
    pub all_tracks: bool,
    pub tracks: &'a [String],
    pub ripple: &'a str,
    pub markers: &'a str,
}
pub struct CutPlan {
    pub command: Value,
    pub summary: Value,
}
#[derive(Clone)]
struct Piece {
    item: Value,
    // Original sequence coordinates, before ripple movement.
    start: Time,
    end: Time,
    after: bool,
}

pub fn cut(project: &Value, sid: &str, options: &CutOptions<'_>) -> Result<CutPlan> {
    project::validate(project)?;
    let original = project["sequences"]
        .get(sid)
        .context("SEQUENCE_NOT_FOUND")?;
    let start = options.start;
    let end = start.add(options.duration);
    ensure!(
        start.nonnegative() && options.duration > &Time::zero(),
        "INVALID_CUT_RANGE: nonnegative start and positive duration required"
    );
    end.json()?;
    ensure!(
        matches!(options.ripple, "none" | "remove-gap"),
        "INVALID_RIPPLE: expected none or remove-gap"
    );
    ensure!(
        matches!(options.markers, "auto" | "cut" | "keep"),
        "INVALID_MARKER_POLICY: expected auto, cut or keep"
    );
    ensure!(
        options.all_tracks == options.tracks.is_empty(),
        "CUT_SCOPE_REQUIRED: choose --all-tracks or one or more --track IDs"
    );
    let all: BTreeSet<String> = array(&original["tracks"])
        .iter()
        .map(|t| text(&t["id"]).into())
        .collect();
    let selected: BTreeSet<String> = if options.all_tracks {
        all.clone()
    } else {
        options.tracks.iter().cloned().collect()
    };
    ensure!(
        options.all_tracks || selected.len() == options.tracks.len(),
        "DUPLICATE_TRACK: repeat each track only once"
    );
    ensure!(
        selected.is_subset(&all),
        "TRACK_NOT_FOUND: cut track does not belong to this sequence"
    );
    let whole = selected == all;
    let ripple = options.ripple == "remove-gap";
    let cut_markers = options.markers == "cut" || (options.markers == "auto" && whole);
    let before_duration = project::sequence_duration(original)?;
    let mut extent = before_duration.clone();
    for track in array(&original["tracks"]) {
        for item in array(&track["items"]) {
            extent = extent.max(bounds(&item["timelineRange"])?.1);
        }
    }
    for marker in array(&original["markers"]) {
        extent = extent
            .max(Time::from_json(&marker["start"])?.add(&Time::from_json(&marker["duration"])?));
    }
    ensure!(
        end <= extent,
        "CUT_OUT_OF_BOUNDS: end {end} exceeds stored sequence extent {extent}"
    );
    let mut seq = original.clone();
    let mut pieces: BTreeMap<String, Vec<Piece>> = BTreeMap::new();
    let mut owner = BTreeMap::new();
    let mut changed = BTreeSet::new();
    let mut removed = vec![];
    let mut trimmed = vec![];
    let mut split = vec![];
    let mut moved = vec![];
    for track in seq["tracks"].as_array_mut().unwrap() {
        let tid = text(&track["id"]).to_owned();
        let mut output = vec![];
        for item in array(&track["items"]) {
            let iid = text(&item["id"]);
            owner.insert(iid.to_owned(), tid.clone());
            let (s, e) = bounds(&item["timelineRange"])?;
            let intersects = selected.contains(&tid) && s < end && e > *start;
            let mut kept = vec![];
            if !intersects {
                kept.push(Piece {
                    item: item.clone(),
                    start: s.clone(),
                    end: e.clone(),
                    after: s >= end,
                });
            } else {
                if s < *start {
                    kept.push(Piece {
                        item: crate::edit::slice(item, &s, start, iid)?,
                        start: s.clone(),
                        end: start.clone(),
                        after: false,
                    });
                }
                if e > end {
                    let new_id = if kept.is_empty() {
                        iid.to_owned()
                    } else {
                        id("item")
                    };
                    kept.push(Piece {
                        item: crate::edit::slice(item, &end, &e, &new_id)?,
                        start: end.clone(),
                        end: e.clone(),
                        after: true,
                    });
                }
                changed.insert(iid.to_owned());
                match kept.len() {
                    0 => removed.push(json!(iid)),
                    1 => trimmed.push(json!(iid)),
                    _ => split.push(json!({"itemId":iid,"leftItemId":kept[0].item["id"],"rightItemId":kept[1].item["id"]})),
                }
            }
            for piece in &mut kept {
                if selected.contains(&tid) && ripple && piece.after {
                    piece.item["timelineRange"]["start"] =
                        piece.start.sub(options.duration).json()?;
                    moved.push(piece.item["id"].clone());
                    changed.insert(iid.to_owned());
                }
                output.push(piece.item.clone());
            }
            pieces.insert(iid.to_owned(), kept);
        }
        track["items"] = json!(output);
    }
    // An explicit track selection never expands through a link to hidden tracks.
    let mut links = vec![];
    let mut membership = BTreeMap::new();
    for link in array(&original["links"]) {
        let members = array(&link["itemIds"]);
        if !members.iter().any(|v| changed.contains(text(v))) {
            links.push(link.clone());
            continue;
        }
        ensure!(members.iter().all(|v| selected.contains(&owner[text(v)])), "LINK_SCOPE_CONFLICT: group {} spans unselected tracks; select every member track or unlink explicitly", link["id"]);
        let mut kept_id = false;
        for after in [false, true] {
            let ids: Vec<Value> = members
                .iter()
                .flat_map(|v| &pieces[text(v)])
                .filter(|p| p.after == after)
                .map(|p| p.item["id"].clone())
                .collect();
            for iid in &ids {
                membership.insert(text(iid).to_owned(), Value::Null);
            }
            if ids.len() >= 2 {
                let mut next = link.clone();
                if kept_id {
                    next["id"] = json!(id("link"));
                }
                kept_id = true;
                next["itemIds"] = json!(ids);
                for iid in &ids {
                    membership.insert(text(iid).to_owned(), next["id"].clone());
                }
                links.push(next);
            }
        }
    }
    for track in seq["tracks"].as_array_mut().unwrap() {
        for item in track["items"].as_array_mut().unwrap() {
            if let Some(lid) = membership.get(text(&item["id"])) {
                if lid.is_null() {
                    item.as_object_mut().unwrap().remove("linkGroupId");
                } else {
                    item["linkGroupId"] = lid.clone();
                }
            }
        }
    }
    let changed_links: Vec<_> = array(&original["links"])
        .iter()
        .filter(|link| !links.contains(link))
        .map(|link| link["id"].clone())
        .collect();
    seq["links"] = json!(links);
    let mut transitions = vec![];
    let mut removed_transitions = vec![];
    for tr in array(&original["transitions"]) {
        let from = text(&tr["fromItemId"]);
        let to = text(&tr["toItemId"]);
        if !selected.contains(&owner[from]) {
            transitions.push(tr.clone());
            continue;
        }
        let old_from = find_item(original, from);
        let old_to = find_item(original, to);
        let (ws, we) = crate::animation::transition_window(tr, old_from)?;
        let outgoing = pieces[from].last();
        let incoming = pieces[to].first();
        let keep = if let (Some(a), Some(b)) = (outgoing, incoming) {
            a.end == bounds(&old_from["timelineRange"])?.1
                && b.start == bounds(&old_to["timelineRange"])?.0
                && !(ws < end && we > *start)
        } else {
            false
        };
        if keep {
            let mut next = tr.clone();
            next["fromItemId"] = outgoing.unwrap().item["id"].clone();
            next["toItemId"] = incoming.unwrap().item["id"].clone();
            transitions.push(next);
        } else {
            removed_transitions.push(tr["id"].clone());
        }
    }
    seq["transitions"] = json!(transitions);
    let mut changed_markers = vec![];
    if cut_markers {
        let mut markers = vec![];
        for marker in array(&original["markers"]) {
            let replacements = cut_marker(marker, start, &end, ripple)?;
            if replacements.as_slice() != std::slice::from_ref(marker) {
                changed_markers.push(json!({"markerId":marker["id"],"resultIds":replacements.iter().map(|m|m["id"].clone()).collect::<Vec<_>>()}));
            }
            markers.extend(replacements);
        }
        seq["markers"] = json!(markers);
    }
    if whole && ripple && original["durationMode"] == "fixed" {
        let fixed = Time::from_json(&original["duration"])?;
        let overlap = end
            .clone()
            .min(fixed.clone())
            .sub(&start.clone().min(fixed.clone()));
        let remaining = fixed.sub(&overlap);
        if remaining == Time::zero() {
            seq["durationMode"] = json!("auto");
            seq.as_object_mut().unwrap().remove("duration");
        } else {
            seq["duration"] = remaining.json()?;
        }
    }
    let mut candidate = project.clone();
    candidate["sequences"][sid] = seq.clone();
    project::normalize(&mut candidate)?;
    project::validate(&candidate).context("CUT_INVALID_RESULT: range edit would violate project invariants (including nested references and transition handles)")?;
    let summary = json!({"sequenceId":sid,"range":range(start,options.duration)?,"trackIds":selected,"allTracks":whole,"ripple":options.ripple,"markers":if cut_markers {"cut"}else{"keep"},"removedItemIds":removed,"trimmedItemIds":trimmed,"splitItems":split,"movedItemIds":moved,"changedLinkIds":changed_links,"removedTransitionIds":removed_transitions,"changedMarkers":changed_markers,"durationBefore":before_duration.json()?,"durationAfter":project::sequence_duration(&candidate["sequences"][sid])?.json()?,"durationModeBefore":original["durationMode"],"durationModeAfter":seq["durationMode"]});
    Ok(CutPlan {
        command: json!({"type":"sequence.put","sequenceId":sid,"sequence":candidate["sequences"][sid]}),
        summary,
    })
}
fn find_item<'a>(seq: &'a Value, iid: &str) -> &'a Value {
    array(&seq["tracks"])
        .iter()
        .flat_map(|t| array(&t["items"]))
        .find(|i| i["id"] == iid)
        .unwrap()
}
fn cut_marker(marker: &Value, start: &Time, end: &Time, ripple: bool) -> Result<Vec<Value>> {
    let s = Time::from_json(&marker["start"])?;
    let d = Time::from_json(&marker["duration"])?;
    let e = s.add(&d);
    let delta = end.sub(start);
    if d == Time::zero() {
        if s >= *start && s < *end {
            return Ok(vec![]);
        }
        let mut point = marker.clone();
        if ripple && s >= *end {
            point["start"] = s.sub(&delta).json()?;
        }
        return Ok(vec![point]);
    }
    let mut spans = vec![];
    if s < *start {
        spans.push((s.clone(), e.clone().min(start.clone())));
    }
    if e > *end {
        let a = s.clone().max(end.clone());
        spans.push(if ripple {
            (a.sub(&delta), e.sub(&delta))
        } else {
            (a, e)
        });
    }
    if ripple && spans.len() == 2 {
        let last = spans.pop().unwrap();
        spans[0].1 = last.1;
    }
    spans
        .into_iter()
        .enumerate()
        .map(|(n, (a, b))| {
            let mut result = marker.clone();
            if n > 0 {
                result["id"] = json!(id("marker"));
            }
            result["start"] = a.json()?;
            result["duration"] = b.sub(&a).json()?;
            Ok(result)
        })
        .collect()
}
