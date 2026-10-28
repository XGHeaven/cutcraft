use cutcraft::{
    image_sequence::{self as seq, ImportOptions, Pattern},
    media, project, query, render, schema,
    store::Store,
    time::{range, Time},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn options(root: &Path) -> ImportOptions {
    ImportOptions {
        directory: root.into(),
        pattern: Some("frame_%04d.bmp".into()),
        files: None,
        pattern_style: "printf".into(),
        start: None,
        end: None,
        missing: "error".into(),
        fps: Time::integer(3),
        color_space: Some("srgb".into()),
        managed: true,
        append: true,
    }
}
fn available() -> bool {
    let ok = Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok();
    assert!(
        ok || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none(),
        "FFmpeg required"
    );
    ok
}
// A real, uncompressed 24-bit BMP fixture; no image-generation dependency.
fn bmp(path: &Path, rgb: [u8; 3], w: u32, h: u32) {
    let stride = (w * 3 + 3) & !3;
    let size = 54 + stride * h;
    let mut b = vec![0; size as usize];
    b[..2].copy_from_slice(b"BM");
    for (at, v) in [
        (2, size),
        (10, 54),
        (14, 40),
        (18, w),
        (22, h),
        (34, stride * h),
    ] {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    b[26..28].copy_from_slice(&1u16.to_le_bytes());
    b[28..30].copy_from_slice(&24u16.to_le_bytes());
    for y in 0..h {
        for x in 0..w {
            let i = (54 + y * stride + x * 3) as usize;
            b[i..i + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
    }
    fs::write(path, b).unwrap();
}
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("云南 图片");
    fs::create_dir(&root).unwrap();
    for (n, c) in [
        (1001, [255, 0, 0]),
        (1002, [0, 255, 0]),
        (1003, [0, 0, 255]),
    ] {
        bmp(&root.join(format!("frame_{n}.bmp")), c, 32, 18);
    }
    let dir = tmp.path().join("edit.cut");
    let store = Store::init(&dir, "Sequence test").unwrap();
    let p = store.project().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap();
    let mut s = p["sequences"][sid].clone();
    s["format"]["width"] = json!(32);
    s["format"]["height"] = json!(18);
    s["format"]["frameRate"] = json!({"num":3,"den":1});
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":s}),
            false,
            false,
        )
        .unwrap();
    drop(store);
    (tmp, root, dir)
}
fn colors(path: &Path) -> Vec<[u8; 3]> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    out.stdout
        .chunks_exact(3)
        .map(|x| x.try_into().unwrap())
        .collect()
}
fn expect_color(c: [u8; 3], channel: usize) {
    assert!(
        c[channel] > 210 && c[(channel + 1) % 3] < 40 && c[(channel + 2) % 3] < 40,
        "{c:?}"
    );
}

#[test]
fn printf_hash_roundtrip_and_invalid_patterns() {
    let p = Pattern::parse("shot_v002.%04d.png", "printf").unwrap();
    for (file, n) in [
        ("shot_v002.0001.png", Some(1)),
        ("shot_v002.10000.png", Some(10000)),
        ("shot_v002.00001.png", None),
        ("shot_v002.1.png", None),
    ] {
        assert_eq!(p.number(file), n);
    }
    let p = Pattern::parse("rate_50%%_%d.png", "printf").unwrap();
    assert_eq!(p.number("rate_50%_1.png"), Some(1));
    assert_eq!(p.number("rate_50%_01.png"), None);
    assert_eq!(
        Pattern::parse("f_####.png", "hash")
            .unwrap()
            .number("f_10000.png"),
        Some(10000)
    );
    assert_eq!(
        Pattern::parse("f_#.png", "hash")
            .unwrap()
            .number("f_0001.png"),
        None
    );
    for s in [
        "f_####.png",
        "%s.png",
        "%4d.png",
        "%004d.png",
        "%020d.png",
        "%d_%d.png",
        "%",
        "file.png",
        "dir/%d.png",
    ] {
        assert!(Pattern::parse(s, "printf").is_err(), "{s}");
    }
    assert!(Pattern::parse("a_#_#", "hash").is_err());
}
#[test]
fn discovery_ranges_hold_skip_and_ordered_list() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for name in ["frame_9999.bmp", "frame_10001.bmp", "frame_09999.bmp"] {
        fs::write(root.join(name), b"unused").unwrap();
    }
    let mut o = options(root);
    assert!(seq::discover(root, &o)
        .unwrap_err()
        .to_string()
        .contains("MISSING_FRAMES"));
    o.missing = "hold".into();
    let d = seq::discover(root, &o).unwrap();
    assert_eq!(
        d.files,
        vec![
            PathBuf::from("frame_9999.bmp"),
            PathBuf::from("frame_9999.bmp"),
            PathBuf::from("frame_10001.bmp")
        ]
    );
    assert_eq!(d.missing_ranges, vec![(10000, 10000)]);
    o.missing = "skip".into();
    assert_eq!(seq::discover(root, &o).unwrap().files.len(), 2);
    o.missing = "hold".into();
    o.start = Some(9998);
    assert!(seq::discover(root, &o).is_err());
    o.start = Some(10000);
    o.end = Some(10000);
    assert!(seq::discover(root, &o).is_err());
    let list = root.join("list.json");
    fs::write(
        &list,
        r#"["frame_10001.bmp","frame_9999.bmp","frame_10001.bmp"]"#,
    )
    .unwrap();
    o = options(root);
    o.pattern = None;
    o.files = Some(list);
    assert_eq!(
        seq::discover(root, &o).unwrap().files[0],
        PathBuf::from("frame_10001.bmp")
    );
}
#[test]
fn managed_import_query_sample_and_render_are_consistent() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    let store = Store::open(&dir).unwrap();
    let before = store.project().unwrap()["revision"].as_u64().unwrap();
    let imported = seq::import(&store, &options(&root), false).unwrap();
    assert_eq!(imported["revision"], before + 1);
    assert_eq!(imported["frameCount"], 3);
    let p = store.project().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap();
    let aid = imported["assetId"].as_str().unwrap();
    schema::validate("project", &p).unwrap();
    assert_eq!(
        store.history().unwrap().last().unwrap()["command"]["type"],
        "batch"
    );
    assert_eq!(
        query::assets(
            &p,
            &dir,
            &query::AssetFilter {
                path: Some("frame_1002".into()),
                source_type: Some("image-sequence".into()),
                ..Default::default()
            }
        )["assets"][0]["firstFrameMatch"]["index"],
        1
    );
    assert_eq!(
        query::items(
            &p,
            &dir,
            &query::ItemFilter {
                query: Some("frame_1002".into()),
                ..Default::default()
            }
        )
        .unwrap()["count"],
        1
    );
    assert_eq!(query::asset_usages(&p, aid).unwrap()["count"], 1);
    let page = query::asset_frames(&p, &dir, aid, 1, 1).unwrap();
    assert_eq!(
        page["frames"][0]["sourceRange"]["start"],
        json!({"num":1,"den":3})
    );
    assert_eq!(page["nextOffset"], 2);
    assert!(query::asset_frames(&p, &dir, aid, 0, 1001).is_err());
    assert_eq!(
        query::asset_frames(&p, &dir, aid, 999, 10).unwrap()["count"],
        0
    );
    assert!(
        query::asset_info_summary(&p, &dir, aid, true).unwrap()["asset"]["source"]
            .get("frames")
            .is_none()
    );
    let s = project::sample(&p, sid, &"1/3".parse().unwrap()).unwrap();
    assert_eq!(s["layers"][0]["imageSequenceFrames"][0]["index"], 1);
    // New source files do not alter imported membership; managed media survives relocation.
    bmp(&root.join("frame_1004.bmp"), [255, 255, 255], 32, 18);
    drop(store);
    let moved = dir.with_file_name("moved.cut");
    fs::rename(&dir, &moved).unwrap();
    media::validate(&p, &moved).unwrap();
    let out = moved.join("output/check.mp4");
    render::render(&p, &moved, sid, &out, false).unwrap();
    let c = colors(&out);
    assert_eq!(c.len(), 3);
    for (i, c) in c.into_iter().enumerate() {
        expect_color(c, i);
    }
    let preview = render::preview(&p, &moved, sid).unwrap();
    assert_eq!(preview["width"], 32);
    assert_eq!(preview["frames"], 3);
    let mut reversed = p.clone();
    let track = 0;
    reversed["sequences"][sid]["tracks"][track]["items"][0]["playback"] =
        json!({"mode":"fit","reverse":true});
    assert_eq!(
        project::sample(&reversed, sid, &Time::zero()).unwrap()["layers"][0]["imageSequenceFrames"]
            [0]["index"],
        2
    );
    let out = moved.join("output/reverse.mp4");
    render::render(&reversed, &moved, sid, &out, false).unwrap();
    let c = colors(&out);
    for (i, c) in c.into_iter().enumerate() {
        expect_color(c, 2 - i);
    }
    let frame = seq::frame_path(
        &moved,
        &p["assets"][aid],
        &p["assets"][aid]["source"]["frames"][0],
    )
    .unwrap();
    fs::write(frame, b"changed").unwrap();
    assert!(media::validate(&p, &moved)
        .unwrap_err()
        .to_string()
        .contains("FINGERPRINT_MISMATCH"));
}
#[test]
fn exact_sampling_blend_non_grid_trim_and_rational_rate() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    let store = Store::open(&dir).unwrap();
    let mut o = options(&root);
    o.fps = "30000/1001".parse().unwrap();
    let imported = seq::import(&store, &o, false).unwrap();
    assert_eq!(imported["duration"], json!({"num":1001,"den":10000}));
    let p = store.project().unwrap();
    let aid = imported["assetId"].as_str().unwrap();
    let a = &p["assets"][aid];
    let sid = p["rootSequenceId"].as_str().unwrap();
    let item = &p["sequences"][sid]["tracks"][0]["items"][0];
    let frame = "1001/30000".parse::<Time>().unwrap();
    assert_eq!(
        seq::selection(a, item, &frame, false, false).unwrap()[0].0,
        1
    );
    let mut blend = item.clone();
    blend["playback"] = json!({"mode":"fit","videoSampling":{"method":"frame-blend"}});
    let samples = seq::selection(
        a,
        &blend,
        &frame.div(&Time::integer(2)).unwrap(),
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        samples,
        vec![(0, "1/2".parse().unwrap()), (1, "1/2".parse().unwrap())]
    );
    let mut trimmed = item.clone();
    trimmed["sourceRange"] = range(&frame.div(&Time::integer(2)).unwrap(), &frame).unwrap();
    trimmed["timelineRange"] = range(&Time::zero(), &frame).unwrap();
    assert_eq!(
        seq::selection(a, &trimmed, &Time::zero(), false, false).unwrap()[0].0,
        0
    );
    trimmed["playback"] = json!({"mode":"fit","reverse":true});
    assert_eq!(
        seq::selection(a, &trimmed, &Time::zero(), false, false).unwrap()[0].0,
        1
    );
    let mut freeze = item.clone();
    freeze["playback"] = json!({"mode":"remap","version":"1.0.0","timeMap":{"$animation":{"version":"1.0.0","valueType":"time","timeReference":"owner","keyframes":[{"offset":{"num":0,"den":1},"value":frame.json().unwrap()},{"offset":a["streams"][0]["duration"],"value":frame.json().unwrap()}]}}});
    assert_eq!(
        seq::selection(a, &freeze, &Time::zero(), false, false).unwrap()[0].0,
        1
    );
    assert_eq!(
        seq::selection(a, &freeze, &"1/20".parse().unwrap(), false, false).unwrap()[0].0,
        1
    );
}
#[test]
fn failed_imports_are_atomic_and_legacy_projects_require_upgrade() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    let store = Store::open(&dir).unwrap();
    let before = fs::read(dir.join("cutcraft.json")).unwrap();
    let history = fs::read(dir.join("history.jsonl")).unwrap();
    bmp(&root.join("frame_1003.bmp"), [0, 0, 255], 16, 18);
    assert!(seq::import(&store, &options(&root), false)
        .unwrap_err()
        .to_string()
        .contains("FORMAT_MISMATCH"));
    assert_eq!(fs::read_dir(dir.join("assets")).unwrap().count(), 0);
    assert_eq!(before, fs::read(dir.join("cutcraft.json")).unwrap());
    assert_eq!(history, fs::read(dir.join("history.jsonl")).unwrap());
    let mut p = store.project().unwrap();
    p["specVersion"] = json!("0.4.0");
    p["$schema"] = json!("https://cutstate.dev/schema/v0.4/project.schema.json");
    project::validate(&p).unwrap();
    fs::write(dir.join("cutcraft.json"), serde_json::to_vec(&p).unwrap()).unwrap();
    assert!(seq::import(&store, &options(&root), false)
        .unwrap_err()
        .to_string()
        .contains("PROJECT_UPGRADE_REQUIRED"));
}
#[test]
fn internal_05_projects_still_import_sample_and_undo_image_sequences() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    // Construct an internal-version fixture; this is not a migration path.
    let mut p = project::new("Internal 0.5 fixture");
    p["specVersion"] = json!("0.5.0");
    p["$schema"] = json!("https://cutstate.dev/schema/v0.5/project.schema.json");
    project::validate(&p).unwrap();
    fs::write(dir.join("cutcraft.json"), project::canonical(&p).unwrap()).unwrap();
    fs::write(dir.join("history.jsonl"), []).unwrap();
    let store = Store::open(&dir).unwrap();
    let result = seq::import(&store, &options(&root), false).unwrap();
    let imported = store.project().unwrap();
    assert_eq!(imported["specVersion"], "0.5.0");
    let asset = &imported["assets"][result["assetId"].as_str().unwrap()];
    let sid = imported["rootSequenceId"].as_str().unwrap();
    let item = &imported["sequences"][sid]["tracks"][0]["items"][0];
    assert_eq!(
        seq::selection(asset, item, &Time::zero(), false, false).unwrap()[0].0,
        0
    );
    assert_eq!(store.history().unwrap()[0]["eventVersion"], "0.5.0");
    store.undo(false, false).unwrap();
    let undone = store.project().unwrap();
    assert_eq!(undone["specVersion"], "0.5.0");
    assert_eq!(undone["assets"], p["assets"]);
    assert_eq!(undone["sequences"], p["sequences"]);
}

#[test]
fn manifest_schema_and_semantics_reject_bad_paths_hashes_and_duration() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    let store = Store::open(&dir).unwrap();
    let r = seq::import(&store, &options(&root), false).unwrap();
    let p = store.project().unwrap();
    let aid = r["assetId"].as_str().unwrap();
    let original = &p["assets"][aid];
    for uri in [
        "../outside.bmp",
        "/absolute.bmp",
        "file:///etc/passwd",
        "%2e%2e/out.bmp",
        "a?query",
        "a#fragment",
        "sub/%2F..%2F..%2Fescape.bmp",
    ] {
        let mut a = original.clone();
        a["source"]["frames"][0]["uri"] = json!(uri);
        assert!(seq::validate_asset(&a).is_err(), "{uri}");
    }
    let mut a = original.clone();
    a["streams"][0]["duration"] = json!({"num":2,"den":1});
    assert!(seq::validate_asset(&a).is_err());
    let mut a = original.clone();
    a["source"]["frames"].as_array_mut().unwrap().reverse();
    assert!(seq::validate_asset(&a).is_err());
    let mut old = p.clone();
    old["specVersion"] = json!("0.4.0");
    old["$schema"] = json!("https://cutstate.dev/schema/v0.4/project.schema.json");
    assert!(schema::validate("project", &old).is_err());
    let mut empty = p.clone();
    empty["assets"][aid]["source"]["frames"] = json!([]);
    assert!(schema::validate("project", &empty).is_err());
}
#[test]
fn cli_hash_import_and_frame_queries() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    let run = |args: Vec<&str>| {
        let out = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
            .arg("-p")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let r = run(vec![
        "import-sequence",
        root.to_str().unwrap(),
        "--pattern",
        "frame_####.bmp",
        "--pattern-style",
        "hash",
        "--fps",
        "3",
        "--color-space",
        "srgb",
        "--managed",
        "--append",
    ]);
    let aid = r["assetId"].as_str().unwrap();
    assert_eq!(
        run(vec!["asset", "frames", aid, "--limit", "1"])["total"],
        3
    );
    assert!(
        run(vec!["asset", "info", aid, "--summary"])["asset"]["source"]
            .get("frames")
            .is_none()
    );
    assert_eq!(
        run(vec![
            "asset",
            "search",
            "frame_1002",
            "--source-type",
            "image-sequence"
        ])["count"],
        1
    );
}

#[test]
fn ordered_external_files_literal_percent_relink_missing_and_undo() {
    if !available() {
        return;
    }
    let (tmp, root, dir) = setup();
    let literal = root.join("literal_%04d.bmp");
    bmp(&literal, [255, 0, 0], 32, 18);
    let list = tmp.path().join("frames.json");
    fs::write(
        &list,
        r#"["literal_%04d.bmp","frame_1003.bmp","literal_%04d.bmp"]"#,
    )
    .unwrap();
    let mut o = options(&root);
    o.pattern = None;
    o.files = Some(list);
    o.managed = false;
    let store = Store::open(&dir).unwrap();
    let before = store.project().unwrap();
    let r = seq::import(&store, &o, false).unwrap();
    assert_eq!(r["fileCount"], 2);
    let p = store.project().unwrap();
    let aid = r["assetId"].as_str().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap();
    let out = dir.join("output/literal.mp4");
    render::render(&p, &dir, sid, &out, false).unwrap();
    let c = colors(&out);
    expect_color(c[0], 0);
    expect_color(c[1], 2);
    expect_color(c[2], 0);
    let renamed = root.with_file_name("relocated frames");
    fs::rename(&root, &renamed).unwrap();
    assert!(media::validate(&p, &dir).is_err());
    let mut a = p["assets"][aid].clone();
    a["uri"] = json!(url::Url::from_directory_path(&renamed).unwrap().to_string());
    store
        .command(
            json!({"type":"asset.put","assetId":aid,"asset":a}),
            false,
            false,
        )
        .unwrap();
    media::validate(&store.project().unwrap(), &dir).unwrap();
    store.undo(false, false).unwrap(); // restore original root URI
                                       // Apply the original import inverse explicitly (undo toggles the latest edit).
    let event = store
        .history()
        .unwrap()
        .into_iter()
        .find(|e| e["status"] == "applied" && e["command"]["type"] == "batch")
        .unwrap();
    store
        .command(
            json!({"type":"batch","commands":event["inverseCommands"]}),
            false,
            false,
        )
        .unwrap();
    let p = store.project().unwrap();
    assert_eq!(p["assets"], before["assets"]);
    assert_eq!(p["sequences"], before["sequences"]);
    assert!(renamed.join("literal_%04d.bmp").is_file());
}

#[test]
fn missing_policy_is_compiled_once_and_media_failures_do_not_repair_it() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    fs::remove_file(root.join("frame_1002.bmp")).unwrap();
    let store = Store::open(&dir).unwrap();
    let mut o = options(&root);
    o.append = false;
    o.managed = false;
    o.missing = "hold".into();
    let r = seq::import(&store, &o, false).unwrap();
    let aid = r["assetId"].as_str().unwrap();
    assert_eq!(r["frameCount"], 3);
    assert_eq!(r["fileCount"], 2);
    assert_eq!(r["missingRanges"], json!([[1002, 1002]]));
    let p = store.project().unwrap();
    let a = &p["assets"][aid];
    assert_eq!(a["source"]["frames"][0], a["source"]["frames"][1]);
    bmp(&root.join("frame_1002.bmp"), [0, 255, 0], 32, 18);
    media::validate(&p, &dir).unwrap();
    assert_eq!(store.project().unwrap(), p);
    fs::remove_file(root.join("frame_1002.bmp")).unwrap();
    o.missing = "skip".into();
    let r = seq::import(&store, &o, false).unwrap();
    assert_eq!(r["frameCount"], 2);
    assert_eq!(r["duration"], json!({"num":2,"den":3}));
    fs::remove_file(root.join("frame_1001.bmp")).unwrap();
    assert!(media::validate(&store.project().unwrap(), &dir)
        .unwrap_err()
        .to_string()
        .contains("MEDIA_MISSING"));
}

#[test]
fn transparent_png_and_multiframe_container_guards() {
    if !available() {
        return;
    }
    let (tmp, root, dir) = setup();
    let png = root.join("alpha.png");
    let out = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red@0.5:s=32x18,format=rgba",
            "-frames:v",
            "1",
            "-threads",
            "1",
        ])
        .arg(&png)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let list = tmp.path().join("list.json");
    fs::write(&list, r#"["alpha.png"]"#).unwrap();
    let store = Store::open(&dir).unwrap();
    let mut o = options(&root);
    o.pattern = None;
    o.files = Some(list.clone());
    let r = seq::import(&store, &o, false).unwrap();
    let p = store.project().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap();
    assert_eq!(
        p["assets"][r["assetId"].as_str().unwrap()]["streams"][0]["video"]["alphaMode"],
        "straight"
    );
    let out = dir.join("output/alpha.mp4");
    render::render(&p, &dir, sid, &out, false).unwrap();
    let c = colors(&out)[0];
    assert!(c[0] > 150 && c[0] < 215 && c[1] < 20 && c[2] < 20, "{c:?}");
    let before = store.project().unwrap();
    let mut apng = fs::read(&png).unwrap();
    apng.splice(
        8..8,
        [
            0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    fs::write(root.join("animated.png"), apng).unwrap();
    let mut webp = b"RIFF\x0c\0\0\0WEBPANIM\0\0\0\0".to_vec();
    webp.resize(20, 0);
    fs::write(root.join("animated.webp"), webp).unwrap();
    let tiff = [b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0];
    fs::write(root.join("pages.tiff"), tiff).unwrap();
    for name in ["animated.png", "animated.webp", "pages.tiff"] {
        fs::write(&list, serde_json::to_vec(&json!([name])).unwrap()).unwrap();
        assert!(
            seq::import(&store, &o, false)
                .unwrap_err()
                .to_string()
                .contains("MULTIFRAME"),
            "{name}"
        );
        assert_eq!(store.project().unwrap(), before);
    }
}

#[test]
fn thumbnails_use_the_frozen_image_sequence_manifest() {
    if !available() {
        return;
    }
    let (_tmp, root, dir) = setup();
    let store = Store::open(&dir).unwrap();
    let mut import = options(&root);
    import.managed = false;
    let result = seq::import(&store, &import, false).unwrap();
    let p = store.project().unwrap();
    let before = fs::read(dir.join("cutcraft.json")).unwrap();
    let history = fs::read(dir.join("history.jsonl")).unwrap();
    // A newly discovered earlier frame must not change the frozen first frame.
    bmp(&root.join("frame_1000.bmp"), [0, 0, 255], 32, 18);
    let thumb = cutcraft::preview::thumbnail(
        &p,
        &dir,
        result["assetId"].as_str().unwrap(),
        "stream_v0",
        &cutcraft::progress::Progress::default(),
    )
    .unwrap();
    let output = dir.join("thumbnail.png");
    fs::write(&output, thumb.bytes).unwrap();
    for pixel in colors(&output) {
        expect_color(pixel, 0);
    }
    assert_eq!(fs::read(dir.join("cutcraft.json")).unwrap(), before);
    assert_eq!(fs::read(dir.join("history.jsonl")).unwrap(), history);
}
