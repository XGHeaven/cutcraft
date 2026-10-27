use cutcraft::{
    animation, edit, id, media, preview,
    progress::Progress,
    project, render, schema,
    store::{self, Store},
    time::Time,
    visual::{Geometry, Pipeline, Space},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn t(n: i64) -> Value {
    Time::integer(n).json().unwrap()
}
fn blur(sigma: Value) -> Value {
    json!({"id":id("effect"),"effectType":"builtin.gaussian-blur","version":"1.0.0","parameters":{"sigma":sigma}})
}
fn curve() -> Value {
    json!({"$animation":{"version":"1.0.0","valueType":"number","timeReference":"owner","keyframes":[{"offset":t(0),"value":0},{"offset":t(1),"value":4}]}})
}
fn track(items: Value) -> Value {
    json!({"id":id("track"),"kind":"video","name":"V","overlapPolicy":"forbid","items":items})
}
fn color(start: i64, duration: i64, c: [f64; 3]) -> Value {
    json!({"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":c[0],"g":c[1],"b":c[2],"a":1}},"timelineRange":{"start":t(start),"duration":t(duration)}})
}
fn fixture() -> (Value, String) {
    let mut p = project::new("blur preview");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(64);
    seq["format"]["height"] = json!(48);
    seq["format"]["frameRate"] = t(6);
    seq["tracks"] = json!([track(json!([
        color(0, 1, [1., 0., 0.]),
        color(1, 1, [0., 0., 1.])
    ]))]);
    (p, sid)
}
fn available() -> bool {
    let ok = Command::new("ffmpeg").arg("-version").output().is_ok();
    assert!(ok || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    ok
}
fn success(out: Output) -> Vec<u8> {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn cli(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .env("CUTCRAFT_RUNTIME_DIR", dir.join("runtime"))
        .arg("-p")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}
fn result(dir: &Path, args: &[&str]) -> Value {
    serde_json::from_slice(&success(cli(dir, args))).unwrap()
}
fn decode(path: &Path, format: &str) -> Vec<u8> {
    success(
        Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-an", "-pix_fmt", format, "-f", "rawvideo", "pipe:1"])
            .output()
            .unwrap(),
    )
}

#[test]
fn gaussian_schema_animation_versions_and_atomic_undo() {
    let (mut p, sid) = fixture();
    let effect = blur(curve());
    p["sequences"][&sid]["tracks"][0]["items"][0]["effects"] = json!([effect]);
    project::validate(&p).unwrap();
    let item = &p["sequences"][&sid]["tracks"][0]["items"][0];
    assert_eq!(
        animation::evaluated(item, &Time::new(1, 2).unwrap()).unwrap()["effects"][0]["parameters"]
            ["sigma"],
        2.
    );
    for params in [
        json!({"sigma":-1}),
        json!({"sigma":101}),
        json!({"sigma":null}),
        json!({"sigma":"3"}),
        json!({"radius":3}),
    ] {
        let mut bad = blur(json!(1));
        bad["parameters"] = params;
        assert!(schema::validate("effect", &bad).is_err());
    }
    let mut bad = effect.clone();
    bad["version"] = json!("2.0.0");
    assert!(schema::validate("effect", &bad).is_err());
    let mut bad = p.clone();
    bad["sequences"][&sid]["tracks"][0]["items"][0]["effects"][0]["parameters"]["sigma"]
        ["$animation"]["keyframes"][1]["value"] = json!(101);
    assert!(project::validate(&bad).is_err());
    bad["sequences"][&sid]["tracks"][0]["items"][0]["effects"][0]["parameters"]["sigma"]
        ["$animation"]["keyframes"][1]["value"] = json!(4);
    bad["sequences"][&sid]["tracks"][0]["items"][0]["effects"][0]["parameters"]["sigma"]
        ["$animation"]["keyframes"][1]["offset"] = t(2);
    assert!(project::validate(&bad).is_err());
    for version in ["0.3.0", "0.4.0", "0.5.0"] {
        let (mut old, osid) = fixture();
        old["specVersion"] = json!(version);
        old["$schema"] = json!(format!(
            "https://cutstate.dev/schema/v{}/project.schema.json",
            &version[..3]
        ));
        project::validate(&old).unwrap();
        old["sequences"][&osid]["tracks"][0]["items"][0]["effects"] = json!([blur(json!(2))]);
        assert!(project::validate(&old)
            .unwrap_err()
            .to_string()
            .contains("PROJECT_UPGRADE_REQUIRED"));
    }
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("cutcraft.json"),
        project::canonical(&p).unwrap(),
    )
    .unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let iid = item["id"].as_str().unwrap();
    let left = id("item");
    let right = id("item");
    let cmd = json!({"type":"item.split","sequenceId":sid,"itemId":iid,"at":Time::new(1,2).unwrap().json().unwrap(),"leftItemId":left,"rightItemId":right});
    // Use the same command envelope as the CLI, preserving effects through slicing.
    store.command(cmd, false, false).unwrap();
    let split = store.project().unwrap();
    let items = split["sequences"][&sid]["tracks"][0]["items"]
        .as_array()
        .unwrap();
    let right_item = items.iter().find(|i| i["id"] == right).unwrap();
    assert_eq!(
        animation::evaluated(right_item, &Time::zero()).unwrap()["effects"][0]["parameters"]
            ["sigma"],
        2.
    );
    let first = store.history().unwrap()[0].clone();
    assert_eq!(first["projectHashBefore"], store::hash(&p).unwrap());
    store.undo(false, false).unwrap();
    let restored = store.project().unwrap();
    assert_eq!(restored["sequences"], p["sequences"]);
    assert_eq!(restored["specVersion"], p["specVersion"]);
    assert_eq!(store.history().unwrap()[0], first);
    let mut bad_item = item.clone();
    bad_item["effects"][0]["parameters"]["sigma"] = json!(101);
    let revision = restored["revision"].clone();
    assert!(store.command(json!({"type":"item.put","sequenceId":sid,"trackId":p["sequences"][&sid]["tracks"][0]["id"],"item":bad_item}),false,false).is_err());
    assert_eq!(store.project().unwrap()["revision"], revision);
    let op = edit::operation(
        &p,
        json!({"type":"project.update","changes":{"name":"test"}}),
    );
    assert_eq!(op["specVersion"], p["specVersion"]);
}

#[test]
fn gaussian_kernel_alpha_linear_light_and_effect_order() {
    let p = project::new("kernel");
    let tmp = tempfile::tempdir().unwrap();
    let geom = Geometry::new(9, 1, 1., 9, 1, &json!({})).unwrap();
    let mut pixels = [[65535u16, 0, 0, 0]; 9];
    pixels[4] = [0, 65535, 0, 32768];
    let raw: Vec<u8> = pixels
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let pipe = Pipeline::new(
        &json!({"effects":[blur(json!(1))]}),
        &p,
        tmp.path(),
        Space::Linear,
    )
    .unwrap();
    let mut out = vec![0; raw.len()];
    geom.frame(&raw, Some(&pipe), &mut out);
    let values: Vec<[f64; 4]> = out
        .chunks_exact(8)
        .map(|p| {
            std::array::from_fn(|i| u16::from_le_bytes([p[2 * i], p[2 * i + 1]]) as f64 / 65535.)
        })
        .collect();
    let sum: f64 = (-3..=3).map(|k| (-0.5 * (k * k) as f64).exp()).sum();
    for (x, p) in values.iter().enumerate() {
        assert_eq!(p[0], 0., "transparent red leaked");
        assert_eq!(p[2], 0.);
        let k = x as i32 - 4;
        let expected = if k.abs() <= 3 {
            (-0.5 * (k * k) as f64).exp() / sum * 32768. / 65535.
        } else {
            0.
        };
        assert!(
            (p[3] - expected).abs() < 2. / 65535.,
            "{x}: {p:?} != {expected}"
        );
        if expected > 0. {
            assert!((p[1] - 1.).abs() < 2. / 65535.);
        }
    }
    let step: Vec<u8> = (0..9)
        .flat_map(|x| [if x < 4 { 0u16 } else { 65535 }, 0, 0, 65535])
        .flat_map(u16::to_le_bytes)
        .collect();
    let grade = json!({"id":id("effect"),"effectType":"builtin.color-grade","version":"1.0.0","parameters":{"exposureEv":1}});
    let mut side = vec![];
    for effects in [
        json!([grade, blur(json!(1))]),
        json!([blur(json!(1)), grade]),
    ] {
        let pipe =
            Pipeline::new(&json!({"effects":effects}), &p, tmp.path(), Space::Linear).unwrap();
        geom.frame(&step, Some(&pipe), &mut out);
        side.push(Space::Rec709.decode(u16::from_le_bytes([out[24], out[25]]) as f64 / 65535.));
    }
    assert!((side[0] - (1. - 1. / sum) / 2.).abs() < 0.0001);
    assert!((side[1] - 2. * side[0]).abs() < 0.0001, "{side:?}");
}

#[test]
fn cli_preview_exact_interval_audio_and_read_only_state() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    let raw: Vec<u8> = (0..48000)
        .flat_map(|n| {
            let f = if n < 24000 { 440. } else { 880. };
            (0.3 * (std::f64::consts::TAU * f * n as f64 / 24000.).sin() as f32).to_le_bytes()
        })
        .collect();
    fs::write(tmp.path().join("audio.raw"), raw).unwrap();
    let wav = tmp.path().join("tone.wav");
    success(
        Command::new("ffmpeg")
            .args([
                "-v", "error", "-f", "f32le", "-ar", "24000", "-ac", "1", "-i",
            ])
            .arg(tmp.path().join("audio.raw"))
            .arg(&wav)
            .output()
            .unwrap(),
    );
    let aid = id("asset");
    let asset = media::asset(&wav, "tone.wav", false).unwrap();
    let stream = asset["streams"][0]["id"].clone();
    p["assets"][&aid] = asset;
    p["sequences"][&sid]["tracks"].as_array_mut().unwrap().push(json!({"id":id("track"),"kind":"audio","name":"A","overlapPolicy":"allow","items":[{"id":id("item"),"type":"media","assetId":aid,"streamId":stream,"timelineRange":{"start":t(0),"duration":t(2)},"sourceRange":{"start":t(0),"duration":t(2)}}]}));
    let before = project::canonical(&p).unwrap();
    fs::write(tmp.path().join("cutcraft.json"), &before).unwrap();
    let frame = result(tmp.path(), &["preview", "--at", "1", "--scale", "0.5"]);
    assert_eq!(frame["width"], 32);
    assert_eq!(frame["height"], 24);
    assert_eq!(frame["revision"], p["revision"]);
    let pixels = decode(Path::new(frame["output"].as_str().unwrap()), "rgb24");
    assert!(pixels.chunks_exact(3).all(|c| c[2] > 245 && c[0] < 10));
    let range = result(
        tmp.path(),
        &[
            "preview",
            "--start",
            "11/12",
            "--duration",
            "1/3",
            "--scale",
            "0.5",
        ],
    );
    assert_eq!(
        range["startTime"],
        Time::new(11, 12).unwrap().json().unwrap()
    );
    assert_eq!(range["frames"], 2);
    let pixels = decode(Path::new(range["output"].as_str().unwrap()), "rgb24");
    assert_eq!(pixels.len(), 32 * 24 * 3 * 2);
    assert!(pixels[..32 * 24 * 3]
        .chunks_exact(3)
        .all(|c| c[0] > 235 && c[2] < 15));
    assert!(pixels[32 * 24 * 3..]
        .chunks_exact(3)
        .all(|c| c[2] > 235 && c[0] < 15));
    let clip = result(
        tmp.path(),
        &["preview", "--start", "1", "--duration", "1/2"],
    );
    assert_eq!(clip["audioSamples"], 12000);
    let pcm = success(
        Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(clip["output"].as_str().unwrap())
            .args(["-vn", "-ac", "1", "-ar", "24000", "-f", "f32le", "pipe:1"])
            .output()
            .unwrap(),
    );
    let pcm: Vec<f64> = pcm
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()) as f64)
        .collect();
    assert!(pcm.len() >= 12000 && pcm.len() < 13024);
    let energy = |frequency: f64| {
        let (mut re, mut im) = (0., 0.);
        for (n, v) in pcm[1000..11000].iter().enumerate() {
            let phase = std::f64::consts::TAU * frequency * n as f64 / 24000.;
            re += v * phase.cos();
            im += v * phase.sin();
        }
        re * re + im * im
    };
    assert!(
        energy(880.) > 100. * energy(440.),
        "audio started at wrong source time"
    );
    for args in [
        vec!["preview", "--at", "2"],
        vec!["preview", "--start", "1", "--duration", "2"],
        vec!["preview", "--duration", "0"],
        vec!["preview", "--at", "0", "--scale", "0"],
        vec!["preview", "--at", "0", "--scale", "NaN"],
        vec!["preview", "--at", "0", "--scale", "inf"],
        vec!["preview", "--at", "0", "--duration", "1"],
    ] {
        assert!(!cli(tmp.path(), &args).status.success());
    }
    assert_eq!(fs::read(tmp.path().join("cutcraft.json")).unwrap(), before);
    assert!(Store::open(tmp.path())
        .unwrap()
        .history()
        .unwrap()
        .is_empty());
    // No options keeps the original full-sequence MP4 command working.
    let full = result(tmp.path(), &["preview"]);
    assert_eq!(full["frames"], 12);
}

#[test]
fn animated_blur_nested_export_and_scaled_preview_pixels() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("edge.png");
    let bytes: Vec<u8> = (0..48)
        .flat_map(|_| {
            (0..64).flat_map(|x| {
                if x < 32 {
                    [255, 0, 0, 0]
                } else {
                    [0, 255, 0, 128]
                }
            })
        })
        .collect();
    fs::write(tmp.path().join("edge.rgba"), bytes).unwrap();
    success(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba",
                "-video_size",
                "64x48",
                "-i",
            ])
            .arg(tmp.path().join("edge.rgba"))
            .args(["-frames:v", "1"])
            .arg(&source)
            .output()
            .unwrap(),
    );
    let (mut p, sid) = fixture();
    let aid = id("asset");
    let mut asset = media::asset(&source, "edge.png", false).unwrap();
    asset["streams"][0]["video"]["colorSpace"] = json!("srgb");
    let stream = asset["streams"][0]["id"].clone();
    p["assets"][&aid] = asset;
    let child_id = id("seq");
    let mut child = p["sequences"][&sid].clone();
    child["background"] = json!({"r":0,"g":0,"b":0,"a":0});
    let item = json!({"id":id("item"),"type":"media","assetId":aid,"streamId":stream,"timelineRange":{"start":t(0),"duration":t(2)},"effects":[blur(curve())]});
    child["tracks"] = json!([track(json!([item]))]);
    p["sequences"][&child_id] = child;
    p["sequences"][&sid]["background"] = json!({"r":0,"g":0,"b":1,"a":1});
    p["sequences"][&sid]["tracks"] = json!([track(
        json!([{"id":id("item"),"type":"sequence","sequenceId":child_id,"timelineRange":{"start":t(0),"duration":t(2)},"sourceRange":{"start":t(0),"duration":t(2)}}])
    )]);
    project::validate(&p).unwrap();
    let progress = Progress::default();
    for (scale, x, y, w) in [(1., 31, 24, 64), (0.5, 15, 12, 32)] {
        let frame = preview::frame_scaled(
            &p,
            tmp.path(),
            &sid,
            &Time::integer(1),
            &progress,
            Some(scale),
        )
        .unwrap();
        let output = tmp.path().join(format!("frame-{scale}.png"));
        fs::write(&output, frame.bytes).unwrap();
        let data = decode(&output, "rgb24");
        let px = &data[(y * w + x) * 3..][..3];
        assert!(
            px[0] < 4 && (110..150).contains(&px[1]) && (215..240).contains(&px[2]),
            "scale={scale}: {px:?}"
        );
    }
    let output = tmp.path().join("render.mp4");
    render::render(&p, tmp.path(), &sid, &output, false).unwrap();
    let data = decode(&output, "rgb24");
    assert_eq!(data.len(), 64 * 48 * 3 * 12);
    let a = &data[(24 * 64 + 29) * 3..][..3];
    let b = &data[(6 * 64 * 48 + 24 * 64 + 29) * 3..][..3];
    assert!(a[1] < 15 && a[2] > 235, "unblurred: {a:?}");
    assert!(b[0] < 15 && b[1] > 60 && b[2] > 220, "blurred: {b:?}");
    // Adjustment layers use the composited input, through the same ordered pipeline.
    p["sequences"][&child_id]["tracks"][0]["items"][0]["effects"] = json!([]);
    p["sequences"][&sid]["tracks"].as_array_mut().unwrap().push(track(json!([{"id":id("item"),"type":"adjustment","timelineRange":{"start":t(0),"duration":t(2)},"effects":[blur(json!(4))]}])));
    let frame = preview::frame(&p, tmp.path(), &sid, &Time::integer(1), &progress).unwrap();
    let output = tmp.path().join("adjustment.png");
    fs::write(&output, frame.bytes).unwrap();
    let data = decode(&output, "rgb24");
    let px = &data[(24 * 64 + 31) * 3..][..3];
    assert!(
        px[0] < 4 && (120..150).contains(&px[1]) && (215..240).contains(&px[2]),
        "adjustment: {px:?}"
    );
    // Semi-transparent native text remains transparent outside its blurred glyphs,
    // including the interior of its full-canvas text surface and nested previews.
    p["sequences"][&sid]["tracks"].as_array_mut().unwrap().pop();
    p["sequences"][&child_id]["tracks"][0]["items"] = json!([{
        "id":id("item"),"type":"generator","generatorType":"builtin.text","version":"1.0.0",
        "parameters":{"text":"X","fontSizePixels":28,"position":"center","color":{"r":1,"g":1,"b":1,"a":0.5}},
        "timelineRange":{"start":t(0),"duration":t(2)},"effects":[blur(json!(2))]
    }]);
    for scale in [1., 0.5] {
        let frame = preview::frame_scaled(
            &p,
            tmp.path(),
            &sid,
            &Time::integer(1),
            &progress,
            Some(scale),
        )
        .unwrap();
        let path = tmp.path().join(format!("text-{scale}.png"));
        fs::write(&path, frame.bytes).unwrap();
        let data = decode(&path, "rgb24");
        let width = (64. * scale) as usize;
        let outside = &data[(2 * width + 2) * 3..][..3];
        assert!(outside[0] < 3 && outside[1] < 3 && outside[2] > 252);
        let peak = data.chunks_exact(3).map(|c| c[1]).max().unwrap();
        assert!(
            (30..195).contains(&peak),
            "semi-transparent blurred glyph: {peak}"
        );
    }
}

#[test]
fn blur_cli_dry_run_animation_and_undo_keep_ids_and_history() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, sid) = fixture();
    fs::write(
        tmp.path().join("cutcraft.json"),
        project::canonical(&p).unwrap(),
    )
    .unwrap();
    let iid = p["sequences"][&sid]["tracks"][0]["items"][0]["id"]
        .as_str()
        .unwrap();
    let before = fs::read(tmp.path().join("cutcraft.json")).unwrap();
    result(
        tmp.path(),
        &[
            "--dry-run",
            "item",
            "effect",
            iid,
            "--kind",
            "gaussian-blur",
            "--parameters",
            "{\"sigma\":3}",
        ],
    );
    assert_eq!(fs::read(tmp.path().join("cutcraft.json")).unwrap(), before);
    result(
        tmp.path(),
        &[
            "item",
            "effect",
            iid,
            "--kind",
            "gaussian-blur",
            "--parameters",
            "{\"sigma\":3}",
        ],
    );
    let store = Store::open(tmp.path()).unwrap();
    let added = store.project().unwrap();
    let first = store.history().unwrap()[0].clone();
    drop(store);
    let eid = added["sequences"][&sid]["tracks"][0]["items"][0]["effects"][0]["id"]
        .as_str()
        .unwrap();
    let file = tmp.path().join("sigma.json");
    fs::write(&file, serde_json::to_vec(&curve()).unwrap()).unwrap();
    result(
        tmp.path(),
        &[
            "item",
            "animate",
            iid,
            "sigma",
            "--effect",
            eid,
            "--file",
            file.to_str().unwrap(),
        ],
    );
    result(tmp.path(), &["undo"]);
    let store = Store::open(tmp.path()).unwrap();
    let undone = store.project().unwrap();
    assert_eq!(undone["sequences"], added["sequences"]);
    assert_eq!(undone["id"], p["id"]);
    assert_eq!(undone["specVersion"], p["specVersion"]);
    assert_eq!(store.history().unwrap()[0], first);
}

#[test]
fn preview_versions_and_cancelled_export_preserve_existing_artifact() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    let options = preview::ExportOptions {
        at: Some(Time::zero()),
        ..Default::default()
    };
    for version in ["0.3.0", "0.4.0", "0.5.0"] {
        p["specVersion"] = json!(version);
        p["$schema"] = json!(format!(
            "https://cutstate.dev/schema/v{}/project.schema.json",
            &version[..3]
        ));
        let before = p.clone();
        let frame = preview::export(&p, tmp.path(), &sid, &options, &Progress::default());
        if version == "0.3.0" {
            assert!(frame
                .unwrap_err()
                .to_string()
                .contains("INTERACTIVE_PREVIEW_UNSUPPORTED_VERSION"));
        } else {
            assert!(Path::new(frame.unwrap()["output"].as_str().unwrap()).is_file());
        }
        assert_eq!(p, before);
    }
    let options = preview::ExportOptions {
        start: Some(Time::integer(1)),
        duration: Some(Time::new(1, 6).unwrap()),
        ..Default::default()
    };
    let artifact = preview::export(&p, tmp.path(), &sid, &options, &Progress::default()).unwrap();
    let path = Path::new(artifact["output"].as_str().unwrap());
    let bytes = fs::read(path).unwrap();
    let cancel = cutcraft::progress::CancellationToken::default();
    cancel.cancel();
    let progress = Progress::with_callback(
        cancel,
        std::sync::Arc::new(|_| panic!("cancelled preview emitted progress")),
    );
    assert!(preview::export(&p, tmp.path(), &sid, &options, &progress)
        .unwrap_err()
        .to_string()
        .contains("JOB_CANCELLED"));
    assert_eq!(fs::read(path).unwrap(), bytes);
}
