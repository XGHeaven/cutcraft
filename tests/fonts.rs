use cutcraft::{
    fonts, id, media, preview,
    progress::Progress,
    project, render, schema,
    store::{self, Store},
    time::Time,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn fixture(caption: bool, family: &str, content: &str) -> (Value, String) {
    let mut p = project::new("fonts");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(320);
    seq["format"]["height"] = json!(96);
    seq["format"]["frameRate"] = json!({"num":1,"den":1});
    let style = json!({"fontFamily":family,"fontSizePixels":40,"position":"center","color":{"r":1,"g":1,"b":1,"a":1}});
    let mut item = json!({"id":id("item"),"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}});
    if caption {
        item["type"] = json!("caption");
        item["text"] = json!(content);
        item["style"] = style;
    } else {
        item["type"] = json!("generator");
        item["generatorType"] = json!("builtin.text");
        item["version"] = json!("1.0.0");
        item["parameters"] = style;
        item["parameters"]["text"] = json!(content);
    }
    seq["tracks"] = json!([{"id":id("track"),"kind":if caption {"subtitle"} else {"video"},"name":"text","overlapPolicy":"forbid","items":[item]}]);
    (p, sid)
}
fn ffmpeg() -> bool {
    let available = Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|r| r.status.success());
    assert!(available || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    available
}
fn pixels(path: &Path) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn assert_glyphs(path: &Path) {
    let raw = pixels(path);
    assert_eq!(raw.len(), 320 * 96 * 3);
    let bright = raw
        .chunks_exact(3)
        .filter(|p| p[0] > 180 && p[1] > 180 && p[2] > 180)
        .count();
    assert!(bright > 100 && bright < 10000, "visible strokes: {bright}");
}
fn system_ascii_family() -> String {
    let p = project::new("font query");
    for family in ["Arial", "DejaVu Sans", "Liberation Sans"] {
        if fonts::resolve(&p, Path::new("."), &json!({"fontFamily":family})).is_ok() {
            return family.into();
        }
    }
    panic!("font tests need Arial, DejaVu Sans or Liberation Sans");
}

#[test]
fn family_contract_validation_legacy_versions_and_undo() {
    for version in ["0.1.0", "0.3.0", "0.4.0", "0.5.0"] {
        for caption in [false, true] {
            let (mut p, sid) = fixture(caption, "PingFang SC", "中文");
            p["specVersion"] = json!(version);
            p["$schema"] = json!(format!(
                "https://cutstate.dev/schema/v{}/project.schema.json",
                version.trim_end_matches(".0")
            ));
            // Archive URIs use major.minor while the persisted version uses semver.
            project::validate(&p).unwrap();
            let item = p["sequences"][&sid]["tracks"][0]["items"][0].clone();
            if !caption {
                schema::validate_generator(&item).unwrap();
            }
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("cutcraft.json"),
                project::canonical(&p).unwrap(),
            )
            .unwrap();
            let store = Store::open(dir.path()).unwrap();
            let original = store.project().unwrap();
            let original_hash = store::hash(&original).unwrap();
            let field = if caption { "style" } else { "parameters" };
            for invalid in [
                json!(""),
                json!("   "),
                json!("a".repeat(201)),
                json!(42),
                Value::Null,
            ] {
                let mut changed = item.clone();
                changed[field]["fontFamily"] = invalid;
                assert!(store.command(json!({"type":"item.put","sequenceId":sid,"trackId":p["sequences"][&sid]["tracks"][0]["id"],"item":changed}),false,false).is_err());
                assert_eq!(store.project().unwrap(), original);
            }
            let mut changed = item.clone();
            changed[field]["fontFamily"] = json!("Another Font");
            store.command(json!({"type":"item.put","sequenceId":sid,"trackId":p["sequences"][&sid]["tracks"][0]["id"],"item":changed}),false,false).unwrap();
            let event = store
                .history()
                .unwrap()
                .into_iter()
                .find(|e| e["status"] == "applied")
                .unwrap();
            assert_eq!(event["projectHashBefore"], original_hash);
            store.undo(false, false).unwrap();
            let restored = store.project().unwrap();
            assert_eq!(restored["specVersion"], p["specVersion"]);
            assert_eq!(restored["sequences"][&sid]["tracks"][0]["items"][0], item);
        }
    }
}

#[test]
fn explicit_fonts_fail_closed_and_pinned_assets_keep_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let (p, _) = fixture(false, "no-such-cutcraft-font-7f2c", "Hello");
    let error = media::validate(&p, temp.path()).unwrap_err().to_string();
    assert!(error.contains("FONT_NOT_FOUND"), "{error}");
    let name = system_ascii_family();
    let (mut p, sid) = fixture(false, &name, "Hello \u{10ffff}");
    let error = media::validate(&p, temp.path()).unwrap_err().to_string();
    assert!(
        error.contains("FONT_GLYPH_MISSING") && error.contains("U+10FFFF"),
        "{error}"
    );
    let selected = fonts::resolve(&p, temp.path(), &json!({"fontFamily":name}))
        .unwrap()
        .unwrap();
    selected
        .prepare(temp.path(), "pinned.ttf", "Hello")
        .unwrap();
    let aid = id("asset");
    p["assets"][&aid] = json!({"kind":"font","uri":"pinned.ttf","name":"Pinned","storage":"managed","availability":"online","fingerprint":{"algorithm":"sha256","value":media::fingerprint(&temp.path().join("pinned.ttf")).unwrap()},"streams":[]});
    let style = &mut p["sequences"][&sid]["tracks"][0]["items"][0]["parameters"];
    style["text"] = json!("Hello");
    style["fontFamily"] = json!("not installed");
    style["fontAssetId"] = json!(aid);
    media::validate(&p, temp.path()).unwrap();
    let resolved = fonts::resolve(
        &p,
        temp.path(),
        &p["sequences"][&sid]["tracks"][0]["items"][0]["parameters"],
    )
    .unwrap()
    .unwrap();
    assert_eq!(resolved.index, 0);
    assert_eq!(
        resolved.path.canonicalize().unwrap(),
        temp.path().join("pinned.ttf").canonicalize().unwrap()
    );
    fs::write(&resolved.path, b"broken font").unwrap();
    assert!(resolved
        .validate("Hello")
        .unwrap_err()
        .to_string()
        .contains("FONT_INVALID"));
}

#[test]
fn system_family_renders_titles_captions_and_legacy() {
    if !ffmpeg() {
        return;
    }
    let family = system_ascii_family();
    let temp = tempfile::tempdir().unwrap();
    for version in ["0.1.0", "0.3.0"] {
        for caption in [false, true] {
            let (mut p, sid) = fixture(caption, &family, "Hello");
            p["specVersion"] = json!(version);
            p["$schema"] = json!(format!(
                "https://cutstate.dev/schema/v{}/project.schema.json",
                version.trim_end_matches(".0")
            ));
            let path = temp.path().join(format!("{version}-{caption}.mp4"));
            render::render(&p, temp.path(), &sid, &path, false).unwrap();
            assert_glyphs(&path);
            if version == "0.1.0" {
                let frame =
                    preview::frame(&p, temp.path(), &sid, &Time::zero(), &Progress::default())
                        .unwrap();
                let png = temp.path().join(format!("{caption}.png"));
                fs::write(&png, frame.bytes).unwrap();
                assert_glyphs(&png);
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn real_pingfang_uses_the_selected_ttc_face_and_chinese_glyphs() {
    if !ffmpeg() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (p, sid) = fixture(false, "PingFang SC", "中文录屏");
    let font = match fonts::resolve(&p, temp.path(), &json!({"fontFamily":"PingFang SC"})) {
        Ok(Some(font)) => font,
        error => {
            assert!(
                std::env::var_os("CUTCRAFT_REQUIRE_PINGFANG").is_none(),
                "PingFang required: {error:?}"
            );
            eprintln!("PingFang is not installed; skipping native PingFang test");
            return;
        }
    };
    assert!(
        font.index > 0,
        "regression must exercise a nonzero collection face"
    );
    let original = fs::read(&font.path).unwrap();
    let face = ttf_parser::Face::parse(&original, font.index).unwrap();
    let glyphs = "中文录屏"
        .chars()
        .map(|c| face.glyph_index(c).unwrap())
        .collect::<Vec<_>>();
    assert!(glyphs.iter().all(|g| g.0 != 0));
    assert!(glyphs.windows(2).all(|g| g[0] != g[1]));
    font.prepare(temp.path(), "chosen.ttc", "中文录屏").unwrap();
    let prepared = fs::read(temp.path().join("chosen.ttc")).unwrap();
    let selected = ttf_parser::Face::parse(&prepared, 0).unwrap();
    assert_eq!(face.weight(), selected.weight());
    for glyph in glyphs {
        assert_eq!(
            face.glyph_bounding_box(glyph),
            selected.glyph_bounding_box(glyph)
        );
        assert!(selected.glyph_bounding_box(glyph).is_some());
    }
    assert_eq!(fs::read(&font.path).unwrap(), original);
    media::validate(&p, temp.path()).unwrap();
    let path = temp.path().join("pingfang.mp4");
    render::render(&p, temp.path(), &sid, &path, false).unwrap();
    assert_glyphs(&path);
    let frame = preview::frame(&p, temp.path(), &sid, &Time::zero(), &Progress::default()).unwrap();
    let png = temp.path().join("pingfang.png");
    fs::write(&png, frame.bytes).unwrap();
    assert_glyphs(&png);
    let bold = fonts::resolve(
        &p,
        temp.path(),
        &json!({"fontFamily":"PingFangSC-Semibold"}),
    )
    .unwrap()
    .unwrap();
    assert_ne!(font.index, bold.index);
}

#[test]
fn cli_font_family_is_persisted_and_invalid_options_do_not_edit() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path(), "font CLI").unwrap();
    let p = store.project().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap();
    let tid = id("track");
    store.command(json!({"type":"track.put","sequenceId":sid,"track":{"id":tid,"kind":"video","name":"T","overlapPolicy":"forbid","items":[]}}),false,false).unwrap();
    drop(store);
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cutcraft"))
            .env("CUTCRAFT_RUNTIME_DIR", temp.path().join("runtime"))
            .arg("-p")
            .arg(temp.path())
            .args([
                "item",
                "text",
                "--track",
                &tid,
                "--duration",
                "1",
                "--text",
                "中文",
                "--font-family",
                "PingFang SC",
            ])
            .args(extra)
            .output()
            .unwrap()
    };
    let before = fs::read(temp.path().join("cutcraft.json")).unwrap();
    let history = fs::read(temp.path().join("history.jsonl")).unwrap();
    let dry = run(&["--dry-run"]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(fs::read(temp.path().join("cutcraft.json")).unwrap(), before);
    assert_eq!(
        fs::read(temp.path().join("history.jsonl")).unwrap(),
        history
    );
    let conflict = run(&["--font-asset", &id("asset")]);
    assert!(!conflict.status.success());
    assert_eq!(fs::read(temp.path().join("cutcraft.json")).unwrap(), before);
    let out = run(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let edited = Store::open(temp.path()).unwrap().project().unwrap();
    assert_eq!(
        edited["sequences"][sid]["tracks"][0]["items"][0]["parameters"]["fontFamily"],
        "PingFang SC"
    );
    assert_eq!(
        edited["revision"].as_u64(),
        p["revision"].as_u64().map(|r| r + 2)
    );
}
