use cutcraft::{id, project, render};
use serde_json::{json, Value};
use std::{path::Path, process::Command};

fn available() -> bool {
    let yes = Command::new("ffmpeg").arg("-version").output().is_ok();
    assert!(yes || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    yes
}
fn range(start: i64, duration: i64) -> Value {
    json!({"start":{"num":start,"den":1},"duration":{"num":duration,"den":1}})
}
fn fixture(caption: bool, alpha: f64) -> Value {
    let mut p = project::new("text transparency");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(160);
    seq["format"]["height"] = json!(90);
    seq["format"]["frameRate"] = json!({"num":1,"den":1});
    let style =
        json!({"fontSizePixels":40,"position":"center","color":{"r":1,"g":1,"b":1,"a":alpha}});
    let mut item = json!({"id":id("item"),"timelineRange":range(1,1)});
    if caption {
        item["type"] = json!("caption");
        item["text"] = json!("X");
        item["style"] = style;
    } else {
        item["type"] = json!("generator");
        item["generatorType"] = json!("builtin.text");
        item["version"] = json!("1.0.0");
        item["parameters"] = style;
        item["parameters"]["text"] = json!("X");
    }
    seq["tracks"] = json!([
        {"id":id("track"),"kind":"video","name":"red","overlapPolicy":"forbid","items":[{
            "id":id("item"),"type":"generator","generatorType":"builtin.color","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":range(0,3)}]},
        {"id":id("track"),"kind":if caption {"subtitle"} else {"video"},"name":"text","overlapPolicy":"forbid","items":[item]}
    ]);
    p
}
fn pixels(path: &Path) -> Vec<u8> {
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
fn check(path: &Path) {
    let raw = pixels(path);
    assert_eq!(raw.len(), 160 * 90 * 3 * 3);
    for frame in raw.chunks_exact(160 * 90 * 3) {
        // Inside the unscaled text canvas, outside glyphs: the lower track must survive.
        let corner = &frame[3 * (10 * 160 + 10)..][..3];
        assert!(
            corner[0] > 235 && corner[1] < 20 && corner[2] < 20,
            "{corner:?}"
        );
    }
    let glyph = &raw[160 * 90 * 3..160 * 90 * 6];
    assert!(glyph.chunks_exact(3).any(|p| p[1] > 220 && p[2] > 220));
}

#[test]
fn captions_and_titles_preserve_background_in_render_and_preview() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    for caption in [true, false] {
        let p = fixture(caption, 1.);
        let sid = p["rootSequenceId"].as_str().unwrap();
        let output = temp.path().join(format!("{caption}.mp4"));
        render::render(&p, temp.path(), sid, &output, false).unwrap();
        check(&output);
        let preview = render::preview(&p, temp.path(), sid).unwrap();
        check(Path::new(preview["video"].as_str().unwrap()));
    }
}

#[test]
fn translucent_text_applies_alpha_once_and_nested_text_keeps_transparency() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut p = fixture(false, 0.5);
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    p["sequences"][&sid]["tracks"][0]["items"][0]["parameters"]["color"]["r"] = json!(0);
    let output = temp.path().join("half.mp4");
    render::render(&p, temp.path(), &sid, &output, false).unwrap();
    let raw = pixels(&output);
    let peak = *raw.iter().max().unwrap();
    // 50% white over black composed in linear light is about 180 in Rec.709,
    // not 64/128 from repeated alpha multiplication or encoded-space blending.
    assert!((170..=190).contains(&peak), "half-alpha peak {peak}");
    let mut p = fixture(false, 1.);
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let child = id("seq");
    let mut nested = p["sequences"][&sid].clone();
    nested["tracks"] = json!([p["sequences"][&sid]["tracks"][1].clone()]);
    nested["background"] = json!({"r":0,"g":0,"b":0,"a":0});
    p["sequences"][&child] = nested;
    p["sequences"][&sid]["tracks"][1] = json!({"id":id("track"),"kind":"video","name":"nested","overlapPolicy":"forbid","items":[{
        "id":id("item"),"type":"sequence","sequenceId":child,"timelineRange":range(0,2),"sourceRange":range(0,2)}]});
    let output = temp.path().join("nested.mp4");
    render::render(&p, temp.path(), &sid, &output, false).unwrap();
    check(&output);
}

#[test]
fn legacy_text_cpu_stage_preserves_the_inside_of_its_canvas() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut p = fixture(false, 1.);
    p["specVersion"] = json!("0.3.0");
    p["$schema"] = json!("https://cutstate.dev/schema/v0.3/project.schema.json");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    p["sequences"][&sid]["tracks"][1]["items"][0]["video"] =
        json!({"transform":{"scale":{"x":1,"y":1}}});
    let output = temp.path().join("legacy.mp4");
    render::render(&p, temp.path(), &sid, &output, false).unwrap();
    check(&output);
}
