use clap::Parser;
use cutcraft::{
    animation,
    app::{self, Cli},
    edit, id,
    project::{self, array, text},
    range_edit::{self, CutOptions},
    store::{self, Store},
    time::{bounds, range, Time},
};
use serde_json::{json, Value};
use std::{fs, path::Path};
fn t(n: i64) -> Value {
    Time::integer(n).json().unwrap()
}
fn r(s: i64, d: i64) -> Value {
    range(&Time::integer(s), &Time::integer(d)).unwrap()
}
fn color(s: i64, d: i64) -> Value {
    json!({"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":r(s,d),"metadata":{"keep":true},"extensions":{"test.plugin":{"x":1}}})
}
fn track(items: Vec<Value>) -> Value {
    json!({"id":id("track"),"kind":"video","name":"V","overlapPolicy":"allow","items":items})
}
fn fixture(tracks: Vec<Value>) -> (Value, String) {
    let mut p = project::new("cut");
    let sid = text(&p["rootSequenceId"]).to_owned();
    p["sequences"][&sid]["tracks"] = json!(tracks);
    project::normalize(&mut p).unwrap();
    (p, sid)
}
fn plan(
    p: &Value,
    sid: &str,
    start: i64,
    duration: i64,
    tracks: &[String],
    ripple: &str,
    markers: &str,
) -> anyhow::Result<range_edit::CutPlan> {
    range_edit::cut(
        p,
        sid,
        &CutOptions {
            start: &Time::integer(start),
            duration: &Time::integer(duration),
            all_tracks: tracks.is_empty(),
            tracks,
            ripple,
            markers,
        },
    )
}
fn applied(p: &Value, c: &range_edit::CutPlan) -> Value {
    edit::apply(p, &edit::operation(p, c.command.clone()), false)
        .unwrap()
        .0
}
fn run(dir: &Path, args: &[&str]) -> anyhow::Result<Value> {
    let cli = Cli::try_parse_from(
        ["cutcraft", "-p", dir.to_str().unwrap()]
            .into_iter()
            .chain(args.iter().copied()),
    )?;
    let cli = app::prepare(cli)?;
    let transported = serde_json::from_slice(&serde_json::to_vec(&cli)?)?;
    app::ApplicationHost.execute(transported)
}
#[test]
fn interval_geometry_covers_edges_gaps_overlaps_and_disabled_items() {
    let cases = [
        (0, 3),
        (6, 3),
        (4, 1),
        (1, 3),
        (5, 3),
        (1, 8),
        (3, 3),
        (0, 9),
    ];
    let mut items: Vec<_> = cases.iter().map(|(s, d)| color(*s, *d)).collect();
    items[7]["enabled"] = json!(false);
    let (p, sid) = fixture(vec![track(items.clone()), track(vec![color(0, 10)])]);
    for ripple in ["none", "remove-gap"] {
        let chosen = vec![text(&p["sequences"][&sid]["tracks"][0]["id"]).into()];
        let c = plan(&p, &sid, 3, 3, &chosen, ripple, "auto").unwrap();
        let q = applied(&p, &c);
        assert_eq!(
            q["sequences"][&sid]["tracks"][1],
            p["sequences"][&sid]["tracks"][1]
        );
        let output = array(&q["sequences"][&sid]["tracks"][0]["items"]);
        assert_eq!(output.len(), 8);
        assert_eq!(c.summary["removedItemIds"].as_array().unwrap().len(), 2);
        for (input, (s, d)) in items.iter().zip(cases) {
            let mut expected = vec![];
            let e = s + d;
            if s < 3 {
                expected.push((s, e.min(3)));
            }
            if e > 6 {
                expected.push((s.max(6), e));
            }
            let parts: Vec<_> = output
                .iter()
                .filter(|o| {
                    o["id"] == input["id"]
                        || c.summary["splitItems"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|split| {
                                split["itemId"] == input["id"] && split["rightItemId"] == o["id"]
                            })
                })
                .collect();
            assert_eq!(parts.len(), expected.len());
            for (piece, (a, b)) in parts.iter().zip(expected) {
                let delta = if ripple == "remove-gap" && a >= 6 {
                    3
                } else {
                    0
                };
                assert_eq!(
                    bounds(&piece["timelineRange"]).unwrap(),
                    (Time::integer(a - delta), Time::integer(b - delta))
                );
                assert_eq!(piece["metadata"], input["metadata"]);
                assert_eq!(piece["extensions"], input["extensions"]);
                assert_eq!(piece["enabled"], input["enabled"]);
            }
        }
    }
}
fn marker(s: i64, d: i64) -> Value {
    json!({"id":id("marker"),"name":"M","start":t(s),"duration":t(d),"metadata":{"keep":true}})
}
#[test]
fn markers_fixed_duration_whole_sequence_and_partial_scope() {
    let (mut p, sid) = fixture(vec![track(vec![color(0, 10)]), track(vec![color(0, 10)])]);
    p["sequences"][&sid]["durationMode"] = json!("fixed");
    p["sequences"][&sid]["duration"] = t(10);
    p["sequences"][&sid]["markers"] = json!([
        marker(2, 0),
        marker(3, 0),
        marker(5, 0),
        marker(6, 0),
        marker(1, 8),
        marker(3, 3),
        marker(1, 3),
        marker(5, 3)
    ]);
    let chosen = vec![text(&p["sequences"][&sid]["tracks"][0]["id"]).into()];
    let q = applied(
        &p,
        &plan(&p, &sid, 3, 3, &chosen, "remove-gap", "auto").unwrap(),
    );
    assert_eq!(
        q["sequences"][&sid]["markers"],
        p["sequences"][&sid]["markers"]
    );
    assert_eq!(q["sequences"][&sid]["duration"], t(10));
    for ripple in ["none", "remove-gap"] {
        let q = applied(&p, &plan(&p, &sid, 3, 3, &[], ripple, "auto").unwrap());
        let ms = array(&q["sequences"][&sid]["markers"]);
        let expected = if ripple == "none" {
            vec![(2, 0), (6, 0), (1, 2), (6, 3), (1, 2), (6, 2)]
        } else {
            vec![(2, 0), (3, 0), (1, 5), (1, 2), (3, 2)]
        };
        assert_eq!(ms.len(), expected.len());
        for (m, (s, d)) in ms.iter().zip(expected) {
            assert_eq!(m["start"], t(s));
            assert_eq!(m["duration"], t(d));
            assert_eq!(m["metadata"], json!({"keep":true}));
        }
        assert_eq!(
            q["sequences"][&sid]["duration"],
            t(if ripple == "none" { 10 } else { 7 })
        );
    }
    let q = applied(
        &p,
        &plan(&p, &sid, 0, 10, &[], "remove-gap", "auto").unwrap(),
    );
    let seq = &q["sequences"][&sid];
    assert_eq!(seq["durationMode"], "auto");
    assert!(seq.get("duration").is_none());
    assert_eq!(project::sequence_duration(seq).unwrap(), Time::zero());
    assert!(array(&seq["markers"]).is_empty());
    assert!(array(&seq["tracks"])
        .iter()
        .all(|t| array(&t["items"]).is_empty()));
}
#[test]
fn linked_cuts_use_absolute_boundaries_and_do_not_expand_scope() {
    let lid = id("link");
    let mut items = vec![color(0, 8), color(1, 6), color(2, 4)];
    for i in &mut items {
        i["linkGroupId"] = json!(lid);
    }
    let (mut p, sid) = fixture(items.iter().map(|i| track(vec![i.clone()])).collect());
    p["sequences"][&sid]["links"] = json!([{"id":lid,"kind":"group","itemIds":items.iter().map(|i|i["id"].clone()).collect::<Vec<_>>(),"metadata":{"keep":true}}]);
    let partial = vec![text(&p["sequences"][&sid]["tracks"][0]["id"]).into()];
    assert!(format!(
        "{:#}",
        plan(&p, &sid, 3, 2, &partial, "remove-gap", "auto")
            .err()
            .unwrap()
    )
    .contains("LINK_SCOPE_CONFLICT"));
    let c = plan(&p, &sid, 3, 2, &[], "remove-gap", "auto").unwrap();
    let q = applied(&p, &c);
    let links = array(&q["sequences"][&sid]["links"]);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0]["id"], lid);
    assert_ne!(links[1]["id"], lid);
    for (n, tr) in array(&q["sequences"][&sid]["tracks"]).iter().enumerate() {
        assert_eq!(tr["items"][0]["timelineRange"], r(n as i64, 3 - n as i64));
        assert_eq!(tr["items"][1]["timelineRange"], r(3, 3 - n as i64));
        assert_eq!(tr["items"][0]["id"], items[n]["id"]);
        for (side, link) in links.iter().enumerate() {
            assert_eq!(tr["items"][side]["linkGroupId"], link["id"]);
            assert_eq!(link["metadata"], json!({"keep":true}));
        }
    }
    // Removing the only companion leaves a valid unlinked singleton.
    let c = plan(&p, &sid, 1, 6, &[], "none", "keep").unwrap();
    let q = applied(&p, &c);
    assert!(array(&q["sequences"][&sid]["links"]).is_empty());
    assert!(array(&q["sequences"][&sid]["tracks"][0]["items"])
        .iter()
        .all(|i| i.get("linkGroupId").is_none()));
}
fn transition(a: &Value, b: &Value) -> Value {
    json!({"id":id("transition"),"fromItemId":a["id"],"toItemId":b["id"],"transitionType":"builtin.crossfade","version":"2.0.0","alignment":"center","duration":t(1),"extensions":{"test.plugin":{"keep":true}}})
}
#[test]
fn transitions_outside_cut_are_rebound_and_intersected_windows_removed() {
    let clips = vec![color(0, 4), color(4, 4), color(8, 4)];
    let (mut p, sid) = fixture(vec![track(clips.clone())]);
    p["sequences"][&sid]["transitions"] = json!([
        transition(&clips[0], &clips[1]),
        transition(&clips[1], &clips[2])
    ]);
    let c = plan(&p, &sid, 1, 1, &[], "remove-gap", "auto").unwrap();
    let q = applied(&p, &c);
    let tr = &q["sequences"][&sid]["transitions"];
    assert_eq!(
        tr[0]["fromItemId"],
        c.summary["splitItems"][0]["rightItemId"]
    );
    assert_eq!(tr[0]["id"], p["sequences"][&sid]["transitions"][0]["id"]);
    assert_eq!(array(tr).len(), 2);
    let c = range_edit::cut(
        &p,
        &sid,
        &CutOptions {
            start: &Time::new(15, 4).unwrap(),
            duration: &Time::new(1, 2).unwrap(),
            all_tracks: true,
            tracks: &[],
            ripple: "remove-gap",
            markers: "auto",
        },
    )
    .unwrap();
    let q = applied(&p, &c);
    assert_eq!(array(&q["sequences"][&sid]["transitions"]).len(), 1);
    assert_eq!(
        c.summary["removedTransitionIds"],
        json!([p["sequences"][&sid]["transitions"][0]["id"]])
    );
    let c = plan(&p, &sid, 3, 6, &[], "remove-gap", "auto").unwrap();
    assert!(array(&applied(&p, &c)["sequences"][&sid]["transitions"]).is_empty());
}
fn curve(kind: &str, keys: Value) -> Value {
    json!({"$animation":{"version":"1.0.0","valueType":kind,"timeReference":"owner","keyframes":keys}})
}
#[test]
fn slices_preserve_reverse_remap_animation_effect_identity_and_caption_words() {
    for playback in [
        json!({"mode":"fit"}),
        json!({"mode":"fit","reverse":true}),
        json!({"mode":"hold"}),
        json!({"mode":"remap","version":"1.0.0","timeMap":curve("time",json!([{"offset":t(0),"value":t(0),"easing":{"type":"cubic-hermite","outRate":t(1),"inRate":t(1)}},{"offset":t(5),"value":t(5)},{"offset":t(10),"value":t(0)}]))}),
    ] {
        let (mut p, sid) = fixture(vec![]);
        let child = id("seq");
        p["sequences"][&child] = p["sequences"][&sid].clone();
        p["sequences"][&child]["durationMode"] = json!("fixed");
        p["sequences"][&child]["duration"] = t(20);
        let clip = json!({"id":id("item"),"type":"sequence","sequenceId":child,"timelineRange":r(0,10),"sourceRange":r(1,10),"playback":playback,"video":{"opacity":curve("number",json!([{"offset":t(0),"value":0,"easing":{"type":"cubic-bezier","x1":0.2,"x2":0.8,"y1":0.1,"y2":0.9}},{"offset":t(10),"value":1}]))},"effects":[{"id":id("effect"),"effectType":"builtin.gaussian-blur","version":"1.0.0","parameters":{"sigma":1}}]});
        p["sequences"][&sid]["tracks"] = json!([track(vec![clip.clone()])]);
        let c = plan(&p, &sid, 3, 2, &[], "remove-gap", "auto").unwrap();
        let q = applied(&p, &c);
        let parts = array(&q["sequences"][&sid]["tracks"][0]["items"]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["effects"][0]["id"], clip["effects"][0]["id"]);
        assert_ne!(parts[1]["effects"][0]["id"], clip["effects"][0]["id"]);
        for (piece, offset) in parts.iter().zip([0, 5]) {
            for half in [0, 1, 2, 3] {
                let local = Time::new(half, 2).unwrap();
                let original = local.add(&Time::integer(offset));
                assert_eq!(
                    animation::mapping(piece, &local, false).unwrap(),
                    animation::mapping(&clip, &original, false).unwrap()
                );
                let a = animation::evaluate(&piece["video"]["opacity"], &local)
                    .unwrap()
                    .as_f64()
                    .unwrap();
                let b = animation::evaluate(&clip["video"]["opacity"], &original)
                    .unwrap()
                    .as_f64()
                    .unwrap();
                assert!((a - b).abs() < 1e-8);
            }
        }
    }
    let caption = json!({"id":id("item"),"type":"caption","text":"one two three","timelineRange":r(0,8),"words":[{"text":"one","range":r(0,2)},{"text":"two","range":r(2,4)},{"text":"three","range":r(6,2)}]});
    let mut tr = track(vec![caption]);
    tr["kind"] = json!("subtitle");
    let (p, sid) = fixture(vec![tr]);
    let q = applied(
        &p,
        &plan(&p, &sid, 3, 2, &[], "remove-gap", "auto").unwrap(),
    );
    let parts = &q["sequences"][&sid]["tracks"][0]["items"];
    assert_eq!(parts[0]["words"][1]["range"], r(2, 1));
    assert_eq!(parts[1]["words"][0]["range"], r(0, 1));
    assert_eq!(parts[1]["words"][1]["range"], r(1, 2));
}
#[test]
fn partial_unrepresentable_slices_reject_but_whole_removal_and_movement_work() {
    let (mut p, sid) = fixture(vec![]);
    let child = id("seq");
    p["sequences"][&child] = p["sequences"][&sid].clone();
    p["sequences"][&child]["durationMode"] = json!("fixed");
    p["sequences"][&child]["duration"] = t(10);
    let base = json!({"id":id("item"),"type":"sequence","sequenceId":child,"timelineRange":r(2,6),"sourceRange":r(0,6)});
    for feature in ["loop", "fade"] {
        let mut item = base.clone();
        if feature == "loop" {
            item["playback"] = json!({"mode":"loop"});
        } else {
            item["audio"] = json!({"fadeIn":t(1)});
        }
        p["sequences"][&sid]["tracks"] = json!([track(vec![item])]);
        assert!(format!(
            "{:#}",
            plan(&p, &sid, 3, 2, &[], "remove-gap", "auto")
                .err()
                .unwrap()
        )
        .contains("UNREPRESENTABLE_EDIT"));
        let q = applied(
            &p,
            &plan(&p, &sid, 2, 6, &[], "remove-gap", "auto").unwrap(),
        );
        assert!(array(&q["sequences"][&sid]["tracks"][0]["items"]).is_empty());
        assert!(plan(&p, &sid, 0, 1, &[], "remove-gap", "auto").is_ok());
    }
    p["sequences"][&sid]["tracks"] = json!([track(vec![base])]);
    assert!(format!(
        "{:#}",
        plan(&p, &child, 0, 8, &[], "remove-gap", "auto")
            .err()
            .unwrap()
    )
    .contains("nested sourceRange"));
}
#[test]
fn cli_scope_locks_dry_run_undo_redo_and_event_replay_are_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (p, sid) = fixture(vec![track(vec![color(0, 10)]), track(vec![color(0, 10)])]);
    fs::write(dir.join("cutcraft.json"), project::canonical(&p).unwrap()).unwrap();
    let before = fs::read(dir.join("cutcraft.json")).unwrap();
    for args in [
        vec!["sequence", "cut", "--start", "2", "--duration", "3"],
        vec![
            "sequence",
            "cut",
            "--start",
            "2",
            "--duration",
            "0",
            "--all-tracks",
        ],
        vec![
            "sequence",
            "cut",
            "--start",
            "8",
            "--duration",
            "3",
            "--all-tracks",
        ],
        vec![
            "sequence",
            "cut",
            "--start",
            "2",
            "--duration",
            "3",
            "--track",
            "unknown",
        ],
    ] {
        assert!(run(dir, &args).is_err());
        assert_eq!(fs::read(dir.join("cutcraft.json")).unwrap(), before);
    }
    let dry = run(
        dir,
        &[
            "sequence",
            "cut",
            "--start",
            "2",
            "--duration",
            "3",
            "--all-tracks",
            "--dry-run",
        ],
    )
    .unwrap();
    assert_eq!(dry["dryRun"], true);
    assert_eq!(fs::read(dir.join("cutcraft.json")).unwrap(), before);
    assert!(!dir.join("history.jsonl").exists());
    let result = run(
        dir,
        &[
            "sequence",
            "cut",
            "--start",
            "2",
            "--duration",
            "3",
            "--all-tracks",
        ],
    )
    .unwrap();
    assert_eq!(result["project"]["revision"], 1);
    assert_eq!(
        result["event"]["metadata"]["cutcraft.rangeEdit"],
        result["cut"]
    );
    let replay = edit::apply(
        &p,
        &edit::operation(&p, result["event"]["command"].clone()),
        false,
    )
    .unwrap()
    .0;
    assert_eq!(replay["sequences"], result["project"]["sequences"]);
    let store = Store::open(dir).unwrap();
    let history = store.history().unwrap();
    drop(store);
    run(dir, &["history", "undo"]).unwrap();
    assert_eq!(
        Store::open(dir).unwrap().project().unwrap()["sequences"],
        p["sequences"]
    );
    run(dir, &["history", "redo"]).unwrap();
    let store = Store::open(dir).unwrap();
    assert_eq!(
        store.project().unwrap()["sequences"],
        result["project"]["sequences"]
    );
    assert_eq!(store.history().unwrap()[0], history[0]);
    let mut seq = store.project().unwrap()["sequences"][&sid].clone();
    seq["tracks"][1]["locked"] = json!(true);
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":seq}),
            false,
            false,
        )
        .unwrap();
    drop(store);
    let before = fs::read(dir.join("cutcraft.json")).unwrap();
    assert!(format!(
        "{:#}",
        run(
            dir,
            &[
                "sequence",
                "cut",
                "--start",
                "0",
                "--duration",
                "1",
                "--all-tracks"
            ]
        )
        .unwrap_err()
    )
    .contains("TRACK_LOCKED"));
    assert_eq!(fs::read(dir.join("cutcraft.json")).unwrap(), before);
    run(
        dir,
        &[
            "sequence",
            "cut",
            "--start",
            "0",
            "--duration",
            "1",
            "--all-tracks",
            "--override-locks",
        ],
    )
    .unwrap();
}
#[test]
fn compatible_versions_keep_schema_ids_and_historical_hashes() {
    for path in [
        "spec/examples/minimal.cutcraft.json",
        "spec/archive/0.3.0/examples/minimal.project.json",
        "spec/archive/0.4.0/examples/animated.cutcraft.json",
        "spec/archive/0.5.0/examples/image-sequence.cutcraft.json",
    ] {
        let mut p = store::read_json(Path::new(path)).unwrap();
        let sid = text(&p["rootSequenceId"]).to_owned();
        p["sequences"][&sid]["tracks"] = json!([track(vec![color(0, 10)])]);
        p["sequences"][&sid]["links"] = json!([]);
        p["sequences"][&sid]["transitions"] = json!([]);
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("cutcraft.json"),
            project::canonical(&p).unwrap(),
        )
        .unwrap();
        let result = run(
            tmp.path(),
            &[
                "sequence",
                "cut",
                "--start",
                "2",
                "--duration",
                "3",
                "--all-tracks",
            ],
        )
        .unwrap();
        for key in ["id", "specVersion", "$schema", "assets"] {
            assert_eq!(result["project"][key], p[key]);
        }
        assert_eq!(result["event"]["eventVersion"], p["specVersion"]);
        assert_eq!(
            result["event"]["projectHashBefore"],
            store::hash(&p).unwrap()
        );
        run(tmp.path(), &["undo"]).unwrap();
        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(store.project().unwrap()["sequences"], p["sequences"]);
        assert_eq!(
            store.history().unwrap()[0]["projectHashBefore"],
            store::hash(&p).unwrap()
        );
    }
}

#[test]
fn actual_export_has_expected_frames_source_pixels_and_audio_after_cut() {
    use std::process::Command;
    let available = ["ffmpeg", "ffprobe"]
        .iter()
        .all(|p| Command::new(p).arg("-version").output().is_ok());
    assert!(available || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    if !available {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("source.rgb");
    let pcm = tmp.path().join("source.f32");
    let source = tmp.path().join("source.avi");
    // 64px avoids a reproducible 32px libx264 chroma artifact in this FFmpeg build.
    let mut pixels = vec![];
    for color in [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]] {
        for _ in 0..4 * 64 * 64 {
            pixels.extend_from_slice(&color);
        }
    }
    fs::write(&raw, pixels).unwrap();
    let mut audio = vec![];
    for n in 0..24000 {
        let hz = [440., 880., 1320.][n / 8000];
        let x = (0.2 * (std::f64::consts::TAU * hz * n as f64 / 8000.).sin()) as f32;
        audio.extend(x.to_le_bytes());
    }
    fs::write(&pcm, audio).unwrap();
    let out = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            "64x64",
            "-framerate",
            "4",
            "-i",
        ])
        .arg(&raw)
        .args(["-f", "f32le", "-ar", "8000", "-ac", "1", "-i"])
        .arg(&pcm)
        .args(["-c:v", "ffv1", "-c:a", "pcm_s16le"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let aid = id("asset");
    let mut asset = cutcraft::media::asset(
        &source,
        url::Url::from_file_path(&source).unwrap().as_str(),
        false,
    )
    .unwrap();
    asset["streams"][0]["video"]["colorSpace"] = json!("rec709");
    let lid = id("link");
    let mut tracks = vec![];
    let mut members = vec![];
    for stream in array(&asset["streams"]) {
        let clip = json!({"id":id("item"),"type":"media","assetId":aid,"streamId":stream["id"],"timelineRange":r(0,3),"sourceRange":r(0,3),"linkGroupId":lid});
        members.push(clip["id"].clone());
        let mut tr = track(vec![clip]);
        tr["kind"] = stream["kind"].clone();
        tracks.push(tr);
    }
    let (mut p, sid) = fixture(tracks);
    p["assets"][&aid] = asset;
    p["sequences"][&sid]["links"] = json!([{"id":lid,"kind":"av","itemIds":members}]);
    let format = &mut p["sequences"][&sid]["format"];
    format["width"] = json!(64);
    format["height"] = json!(64);
    format["frameRate"] = t(4);
    format["audioSampleRate"] = json!(8000);
    for ripple in ["remove-gap", "none"] {
        let q = applied(&p, &plan(&p, &sid, 1, 1, &[], ripple, "auto").unwrap());
        let output = tmp.path().join(format!("{ripple}.mp4"));
        cutcraft::render::render(&q, tmp.path(), &sid, &output, false).unwrap();
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-an", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        let frames: Vec<_> = decoded.stdout.chunks_exact(64 * 64 * 3).collect();
        assert_eq!(frames.len(), if ripple == "none" { 12 } else { 8 });
        for (n, frame) in frames.iter().enumerate() {
            let px = &frame[(32 * 64 + 32) * 3..][..3];
            if n < 4 {
                assert!(px[0] > 230 && px[2] < 20, "{n}: {px:?}");
            } else if ripple == "none" && n < 8 {
                assert!(px.iter().all(|x| *x < 15), "{px:?}");
            } else {
                assert!(px[2] > 230 && px[0] < 20, "{n}: {px:?}");
            }
        }
        let seconds = if ripple == "none" { "3" } else { "2" };
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args([
                "-vn", "-ar", "8000", "-ac", "1", "-t", seconds, "-f", "f32le", "-",
            ])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        let samples: Vec<f64> = decoded
            .stdout
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64)
            .collect();
        assert_eq!(samples.len(), seconds.parse::<usize>().unwrap() * 8000);
        let power = |start: usize, hz: f64| {
            let mut re = 0.;
            let mut im = 0.;
            for (n, s) in samples[start..start + 2400].iter().enumerate() {
                let phase = std::f64::consts::TAU * hz * n as f64 / 8000.;
                re += s * phase.cos();
                im += s * phase.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(power(1600, 440.) > power(1600, 1320.) * 10.);
        let tail = if ripple == "none" { 17600 } else { 9600 };
        assert!(power(tail, 1320.) > power(tail, 880.) * 10.);
        if ripple == "none" {
            let rms = (samples[9600..12000].iter().map(|s| s * s).sum::<f64>() / 2400.).sqrt();
            assert!(rms < 0.005, "gap audio {rms}");
        }
    }
}

#[test]
fn library_scope_validation_does_not_depend_on_clap() {
    let (p, sid) = fixture(vec![track(vec![color(0, 10)])]);
    let tid = text(&p["sequences"][&sid]["tracks"][0]["id"]).to_owned();
    for (all, tracks, ripple, markers, start, duration) in [
        (
            false,
            vec![],
            "none",
            "auto",
            Time::zero(),
            Time::integer(1),
        ),
        (
            true,
            vec![tid.clone()],
            "none",
            "auto",
            Time::zero(),
            Time::integer(1),
        ),
        (
            false,
            vec![tid.clone(), tid.clone()],
            "none",
            "auto",
            Time::zero(),
            Time::integer(1),
        ),
        (
            true,
            vec![],
            "overwrite",
            "auto",
            Time::zero(),
            Time::integer(1),
        ),
        (
            true,
            vec![],
            "none",
            "guess",
            Time::zero(),
            Time::integer(1),
        ),
        (
            true,
            vec![],
            "none",
            "auto",
            Time::integer(-1),
            Time::integer(1),
        ),
        (true, vec![], "none", "auto", Time::zero(), Time::zero()),
        (
            true,
            vec![],
            "none",
            "auto",
            Time::integer(i64::MAX),
            Time::integer(1),
        ),
    ] {
        assert!(range_edit::cut(
            &p,
            &sid,
            &CutOptions {
                start: &start,
                duration: &duration,
                all_tracks: all,
                tracks: &tracks,
                ripple,
                markers
            }
        )
        .is_err());
    }
    // A delayed materialized operation still obeys the domain revision guard.
    let c = plan(&p, &sid, 1, 2, &[], "remove-gap", "auto").unwrap();
    let op = edit::operation(&p, c.command);
    let newer = edit::apply(
        &p,
        &edit::operation(
            &p,
            json!({"type":"project.update","changes":{"name":"other editor"}}),
        ),
        false,
    )
    .unwrap()
    .0;
    assert!(
        format!("{:#}", edit::apply(&newer, &op, false).unwrap_err()).contains("REVISION_CONFLICT")
    );
}
