use clap::Parser;
use cutcraft::{
    app::{self, Cli},
    id, preview,
    progress::Progress,
    project, render, schema,
    store::Store,
    time::Time,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
fn rgba(r: f64, g: f64, b: f64, a: f64) -> Value {
    json!({"r":r,"g":g,"b":b,"a":a})
}
fn fixture(caption: bool) -> (Value, String) {
    let mut p = project::new("card");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(192);
    seq["format"]["height"] = json!(128);
    seq["format"]["frameRate"] = Time::integer(2).json().unwrap();
    let range = json!({"start":{"num":0,"den":1},"duration":{"num":1,"den":1}});
    let style = json!({"fontSizePixels":20,"align":"center","position":"center","color":rgba(1.,1.,1.,0.5),"card":{"widthPixels":128,"heightPixels":80,"paddingPixels":8,"radiusPixels":16,"background":{"type":"solid","color":rgba(1.,0.,0.,0.5)},"border":{"widthPixels":4,"color":rgba(0.,1.,0.,1.)}}});
    let mut item = json!({"id":id("item"),"timelineRange":range});
    if caption {
        item["type"] = json!("caption");
        item["text"] = json!(" ");
        item["style"] = style;
    } else {
        item["type"] = json!("generator");
        item["generatorType"] = json!("builtin.text");
        item["version"] = json!("1.0.0");
        item["parameters"] = style;
        item["parameters"]["text"] = json!(" ");
    }
    seq["tracks"] = json!([{"id":id("track"),"kind":"video","name":"blue","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.color","parameters":{"color":rgba(0.,0.,1.,1.)},"timelineRange":range}]},{"id":id("track"),"kind":if caption{"subtitle"}else{"video"},"name":"card","overlapPolicy":"forbid","items":[item]}]);
    (p, sid)
}
fn style<'a>(p: &'a mut Value, sid: &str) -> &'a mut Value {
    let item = &mut p["sequences"][sid]["tracks"][1]["items"][0];
    if item["type"] == "caption" {
        &mut item["style"]
    } else {
        &mut item["parameters"]
    }
}
fn decode(path: &Path) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn pix(data: &[u8], w: usize, x: usize, y: usize) -> &[u8] {
    &data[(y * w + x) * 3..(y * w + x) * 3 + 3]
}
#[test]
fn actual_export_preview_nested_and_alpha() {
    let tmp = tempfile::tempdir().unwrap();
    for caption in [false, true] {
        let (mut p, sid) = fixture(caption);
        project::validate(&p).unwrap();
        let out = tmp.path().join(format!("{caption}.mp4"));
        render::render(&p, tmp.path(), &sid, &out, false).unwrap();
        let raw = decode(&out);
        assert_eq!(raw.len(), 192 * 128 * 3 * 2);
        let c = pix(&raw, 192, 35, 27);
        assert!(c[2] > 235 && c[0] < 15 && c[1] < 15, "rounded corner {c:?}");
        let c = pix(&raw, 192, 96, 60);
        assert!(
            (170..=190).contains(&c[0]) && (170..=190).contains(&c[2]) && c[1] < 15,
            "half alpha {c:?}"
        );
        let c = pix(&raw, 192, 96, 26);
        assert!(c[1] > 225 && c[0] < 25 && c[2] < 25, "border {c:?}");
        let frame = preview::frame_scaled(
            &p,
            tmp.path(),
            &sid,
            &Time::zero(),
            &Progress::default(),
            Some(0.5),
        )
        .unwrap();
        let path = tmp.path().join("scaled.png");
        fs::write(&path, frame.bytes).unwrap();
        let raw = decode(&path);
        assert_eq!(raw.len(), 96 * 64 * 3);
        let c = pix(&raw, 96, 48, 30);
        assert!(
            (180..=195).contains(&c[0]) && (180..=195).contains(&c[2]),
            "scaled half alpha {c:?}"
        );
        let child = id("seq");
        let mut nested = p["sequences"][&sid].clone();
        nested["background"] = rgba(0., 0., 0., 0.);
        nested["tracks"] = json!([p["sequences"][&sid]["tracks"][1].clone()]);
        p["sequences"][&child] = nested;
        p["sequences"][&sid]["tracks"][1] = json!({"id":id("track"),"kind":"video","name":"nested","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"sequence","sequenceId":child,"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}},"sourceRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}}]});
        let out = tmp.path().join(format!("nested{caption}.mp4"));
        render::render(&p, tmp.path(), &sid, &out, false).unwrap();
        let raw = decode(&out);
        let c = pix(&raw, 192, 96, 60);
        assert!(
            (170..=190).contains(&c[0]) && (170..=190).contains(&c[2]),
            "nested half alpha {c:?}"
        );
    }
}
#[test]
fn gradients_shadow_multiline_stroke_and_translucent_glyphs() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture(false);
    let s = style(&mut p, &sid);
    s["text"] = json!("X\nX");
    s["card"]["border"] = json!({"widthPixels":1,"color":rgba(0.,1.,0.,1.)});
    s["card"]["background"] = json!({"type":"linear-gradient","startColor":rgba(1.,0.,0.,1.),"endColor":rgba(0.,0.,1.,1.)});
    s["card"]["textStroke"] = json!({"widthPixels":2,"color":rgba(0.,1.,0.,1.)});
    s["card"]["shadow"] = json!({"offsetYPixels":8,"sigmaPixels":2,"color":rgba(0.,0.,0.,0.5)});
    s["card"]["verticalAlign"] = json!("center");
    s["card"]["lineHeight"] = json!(1.5);
    let f = preview::frame(&p, tmp.path(), &sid, &Time::zero(), &Progress::default()).unwrap();
    let path = tmp.path().join("card.png");
    fs::write(&path, f.bytes).unwrap();
    let raw = decode(&path);
    let l = pix(&raw, 192, 50, 60);
    let r = pix(&raw, 192, 142, 60);
    assert!(l[0] > l[2] && r[2] > r[0], "gradient {l:?}/{r:?}");
    let sh = pix(&raw, 192, 96, 108);
    assert!((175..=210).contains(&sh[2]) && sh[0] < 5, "shadow {sh:?}");
    for (y0, y1) in [(34, 60), (64, 90)] {
        assert!(
            (y0..y1).any(|y| (75..117).any(|x| pix(&raw, 192, x, y)[1] > 220)),
            "missing green stroke in line {y0}"
        );
    }
    // Isolate 50% glyph coverage on blue: full-coverage white must become linear
    // (0.5,0.5,1), while pixels outside glyphs remain blue.
    let s = style(&mut p, &sid);
    s["card"] =
        json!({"widthPixels":128,"heightPixels":80,"paddingPixels":8,"verticalAlign":"center"});
    let f = preview::frame(&p, tmp.path(), &sid, &Time::zero(), &Progress::default()).unwrap();
    fs::write(&path, f.bytes).unwrap();
    let raw = decode(&path);
    assert!(raw
        .chunks_exact(3)
        .any(|p| (184..=192).contains(&p[0]) && (184..=192).contains(&p[1]) && p[2] > 250));
    assert_eq!(pix(&raw, 192, 45, 60), [0, 0, 255]);
}
#[test]
fn schema_versions_cli_atomic_undo_and_read_only_dry_run() {
    let (p, sid) = fixture(false);
    let original = p.clone();
    for version in ["0.3.0", "0.4.0", "0.5.0"] {
        let mut old = p.clone();
        old["specVersion"] = json!(version);
        old["$schema"] = json!(format!(
            "https://cutstate.dev/schema/v{}/project.schema.json",
            &version[..3]
        ));
        assert!(project::validate(&old)
            .unwrap_err()
            .to_string()
            .contains("TEXT_CARD_UNSUPPORTED_VERSION"));
    }
    for (key, value) in [
        ("radiusPixels", json!(1000)),
        ("paddingPixels", json!(40)),
        ("lineHeight", json!(0)),
        ("widthPixels", json!(0)),
        ("unknown", json!(true)),
    ] {
        let mut bad = p.clone();
        style(&mut bad, &sid)["card"][key] = value;
        assert!(project::validate(&bad).is_err());
    }
    schema::validate("project", &p).unwrap();
    assert_eq!(p, original);
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("cutcraft.json"),
        project::canonical(&p).unwrap(),
    )
    .unwrap();
    let track = p["sequences"][&sid]["tracks"][1]["id"].as_str().unwrap();
    let parse = |extra: &[&str]| {
        let mut args = vec![
            "cutcraft",
            "-p",
            tmp.path().to_str().unwrap(),
            "item",
            "text",
            "--track",
            track,
            "--start",
            "1",
            "--duration",
            "1",
            "--text",
            "Hello\nCard",
            "--card-width",
            "128",
            "--card-height",
            "80",
            "--card-radius",
            "12",
            "--card-color",
            "#ff000080",
            "--card-border-width",
            "2",
            "--card-shadow-color",
            "#00000080",
            "--card-shadow-blur",
            "3",
            "--text-stroke-width",
            "1",
        ];
        args.extend_from_slice(extra);
        Cli::try_parse_from(args).unwrap()
    };
    let before = fs::read(tmp.path().join("cutcraft.json")).unwrap();
    let dry = app::prepare(parse(&["--dry-run"])).unwrap();
    app::ApplicationHost.execute(dry).unwrap();
    assert_eq!(fs::read(tmp.path().join("cutcraft.json")).unwrap(), before);
    let request = app::prepare(parse(&[])).unwrap();
    let request = serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap();
    app::ApplicationHost.execute(request).unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let after = store.project().unwrap();
    assert_eq!(after["revision"], p["revision"].as_u64().unwrap() + 1);
    let event = store.history().unwrap()[0].clone();
    let added = &after["sequences"][&sid]["tracks"][1]["items"][1];
    assert_eq!(added["parameters"]["card"]["radiusPixels"], 12.0);
    let mut bad = added.clone();
    bad["parameters"]["card"]["paddingPixels"] = json!(500);
    assert!(store
        .command(
            json!({"type":"item.put","sequenceId":sid,"trackId":track,"item":bad}),
            false,
            false
        )
        .is_err());
    assert_eq!(store.project().unwrap(), after);
    assert_eq!(store.history().unwrap()[0], event);
    store.undo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["sequences"], p["sequences"]);
    assert_eq!(store.history().unwrap()[0], event);
}
