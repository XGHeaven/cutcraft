use cutcraft::{id, media, project, render, time::Time};
use serde_json::{json, Value};
use std::{path::Path, process::Command};
fn range() -> Value {
    json!({"start":{"num":0,"den":1},"duration":{"num":1,"den":1}})
}
fn pixels(path: &Path) -> Vec<u8> {
    let r = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-vf",
            "scale=640:360",
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(r.status.success());
    r.stdout
}
#[test]
fn small_preview_preserves_nested_text_layout_without_editing_project() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project::new("preview");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let child = id("seq");
    p["sequences"][&sid]["format"]["width"] = json!(1280);
    p["sequences"][&sid]["format"]["height"] = json!(720);
    p["sequences"][&sid]["format"]["frameRate"] = Time::integer(1).json().unwrap();
    p["sequences"][&child] = p["sequences"][&sid].clone();
    p["sequences"][&child]["tracks"] = json!([{"id":id("track"),"kind":"video","name":"Text","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.text","version":"1.0.0","parameters":{"text":"Cutcraft preview","fontSizePixels":96,"position":"bottom","align":"left"},"timelineRange":range()}]}]);
    p["sequences"][&sid]["tracks"] = json!([{"id":id("track"),"kind":"video","name":"Nested","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"sequence","sequenceId":child,"timelineRange":range(),"sourceRange":range()}]}]);
    let original = p.clone();
    let out = tmp.path().join("full.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let result = render::preview(&p, tmp.path(), &sid).unwrap();
    assert_eq!(p, original);
    assert_eq!(result["width"], 640);
    assert_eq!(result["height"], 360);
    assert_eq!(result["frameRate"], json!({"num":1,"den":1}));
    assert_eq!(result["frames"], 1);
    let path = Path::new(result["video"].as_str().unwrap());
    assert!(path.is_file());
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    let a = pixels(&out);
    let b = pixels(path);
    assert_eq!(a.len(), b.len());
    let error = a
        .iter()
        .zip(&b)
        .map(|(a, b)| (*a as f64 - *b as f64).abs())
        .sum::<f64>()
        / a.len() as f64;
    assert!(error < 5., "text layout/scale changed: MAE {error}");
    assert!(b.iter().filter(|v| **v > 150).count() > 1000);
    // Portrait previews also use a 640-pixel long edge, without altering final format.
    p["sequences"][&sid]["format"]["width"] = json!(1080);
    p["sequences"][&sid]["format"]["height"] = json!(1920);
    let result = render::preview(&p, tmp.path(), &sid).unwrap();
    let probe = media::probe(Path::new(result["video"].as_str().unwrap())).unwrap();
    assert_eq!(probe["streams"][0]["width"], 360);
    assert_eq!(probe["streams"][0]["height"], 640);
}
