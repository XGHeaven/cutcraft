use cutcraft::{
    id, media,
    project::{self, text},
    render,
    store::Store,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};
fn available() -> bool {
    let yes = Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok();
    assert!(
        yes || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none(),
        "FFmpeg required by test environment"
    );
    if !yes {
        eprintln!("Skipping media integration test: ffmpeg/ffprobe unavailable");
    }
    yes
}
fn ffmpeg(args: &[&str], output: &Path) {
    let result = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(args)
        .args([
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
        ])
        .arg(output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn cli(project: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn cli_import_edit_render_preview_and_integrity_checks() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("camera ' A.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=30:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
        ],
        &source,
    );
    let dir = temp.path().join("film.cut");
    cli(
        &dir,
        &[
            "init",
            dir.to_str().unwrap(),
            "--name",
            "Demo </script><script>alert(1)</script>",
        ],
    );
    let imported = cli(
        &dir,
        &[
            "import",
            source.to_str().unwrap(),
            "--managed",
            "--append",
            "--color-space",
            "rec709",
        ],
    );
    let aid = imported["import"]["assetId"].as_str().unwrap();
    let store = Store::open(&dir).unwrap();
    let p = store.project().unwrap();
    let sid = text(&p["rootSequenceId"]);
    let mut seq = p["sequences"][sid].clone();
    seq["format"]["width"] = json!(160);
    seq["format"]["height"] = json!(90);
    // Blue overlay begins exactly at output frame 30.
    seq["tracks"].as_array_mut().unwrap().push(json!({"id":id("track"),"kind":"video","name":"Blue","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":0,"g":0,"b":1,"a":1}},"timelineRange":{"start":{"num":1,"den":1},"duration":{"num":1,"den":1}}}]}));
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":seq}),
            false,
            false,
        )
        .unwrap();
    drop(store);
    cli(&dir, &["render"]);
    let rendered = dir.join("output/render.mp4");
    let probed = media::probe(&rendered).unwrap();
    let video = probed["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["nb_frames"], "60");
    assert_eq!(video["duration"], "2.000000");
    let audio = probed["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .unwrap();
    assert_eq!(audio["duration"], "2.000000");
    let pixels = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&rendered)
        .args([
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .unwrap();
    assert!(pixels.status.success());
    assert_eq!(pixels.stdout.len(), 60 * 3);
    for frame in [0, 29] {
        let pixel = &pixels.stdout[frame * 3..frame * 3 + 3];
        assert!(pixel[0] > 200 && pixel[2] < 40, "frame {frame}: {pixel:?}");
    }
    for frame in [30, 59] {
        let pixel = &pixels.stdout[frame * 3..frame * 3 + 3];
        assert!(pixel[2] > 200 && pixel[0] < 40, "frame {frame}: {pixel:?}");
    }
    let audio_samples = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&rendered)
        .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"])
        .output()
        .unwrap();
    assert!(audio_samples.status.success());
    assert!(
        audio_samples
            .stdout
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]) as i32)
            .any(|x| x.abs() > 1000),
        "rendered audio must contain the input sine wave"
    );
    let preview = cli(&dir, &["preview"]);
    assert!(preview.get("page").is_none());
    let preview_path = Path::new(preview["video"].as_str().unwrap());
    assert!(preview_path.starts_with(dir.canonicalize().unwrap().join("cache")));
    assert!(!preview_path.parent().unwrap().join("index.html").exists());
    let preview_probe = media::probe(preview_path).unwrap();
    let preview_video = preview_probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["codec_type"] == "video")
        .unwrap();
    assert_eq!(preview_video["width"], 160);
    assert_eq!(preview_video["height"], 90);
    assert_eq!(preview_video["nb_frames"], "48");
    assert_eq!(preview_video["avg_frame_rate"], "24/1");
    let store = Store::open(&dir).unwrap();
    let p = store.project().unwrap();
    assert!(media::validate(&p, &dir).is_ok());
    let asset_path = media::local_path(&dir, text(&p["assets"][aid]["uri"])).unwrap();
    std::fs::write(asset_path, "tampered").unwrap();
    assert!(media::validate(&p, &dir)
        .unwrap_err()
        .to_string()
        .contains("FINGERPRINT_MISMATCH"));
}
#[test]
fn unsupported_render_fails_before_overwriting_output() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut p = project::new("unsupported");
    let sid = text(&p["rootSequenceId"]).to_string();
    let seq = &mut p["sequences"][&sid];
    seq["durationMode"] = json!("fixed");
    seq["duration"] = json!({"num":1,"den":1});
    seq["tracks"] = json!([{"id":id("track"),"kind":"video","name":"V","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"adjustment","video":{"blendMode":"multiply"},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}}]}]);
    let out = temp.path().join("out.mp4");
    std::fs::write(&out, "keep").unwrap();
    assert!(render::render(&p, temp.path(), &sid, &out, true)
        .unwrap_err()
        .to_string()
        .contains("BACKEND_UNSUPPORTED"));
    assert_eq!(std::fs::read_to_string(out).unwrap(), "keep");
}

#[test]
fn dji_attached_cover_is_an_image_and_not_appended_as_video() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let video = temp.path().join("video.mp4");
    let cover = temp.path().join("cover.jpg");
    let source = temp.path().join("with-cover.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=30:d=1",
            "-c:v",
            "libx264",
        ],
        &video,
    );
    ffmpeg(
        &["-f", "lavfi", "-i", "color=blue:s=160x90", "-frames:v", "1"],
        &cover,
    );
    ffmpeg(
        &[
            "-i",
            video.to_str().unwrap(),
            "-i",
            cover.to_str().unwrap(),
            "-map",
            "0:v:0",
            "-map",
            "1:v:0",
            "-c",
            "copy",
            "-disposition:v:1",
            "attached_pic",
        ],
        &source,
    );
    let asset = media::asset(&source, "with-cover.mp4", false).unwrap();
    let streams = asset["streams"].as_array().unwrap();
    assert_eq!(streams[1]["kind"], "image");
    assert_eq!(streams[1]["metadata"]["role"], "attached-picture");
    assert!(streams[1]["video"]["frameRate"].is_null());
    let dir = temp.path().join("edit.cut");
    cli(&dir, &["init", dir.to_str().unwrap()]);
    cli(&dir, &["import", source.to_str().unwrap(), "--append"]);
    let project = cli(&dir, &["inspect"]);
    let sid = text(&project["rootSequenceId"]);
    assert_eq!(
        project["sequences"][sid]["tracks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn seeking_nonzero_source_keeps_exact_frame_content_and_duration() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("count.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=30:d=4",
            "-c:v",
            "libx264",
            "-g",
            "90",
        ],
        &source,
    );
    let dir = temp.path().join("seek.cut");
    cli(&dir, &["init", dir.to_str().unwrap()]);
    let imported = cli(
        &dir,
        &[
            "import",
            source.to_str().unwrap(),
            "--color-space",
            "rec709",
        ],
    );
    let aid = imported["assetId"].clone();
    let store = Store::open(&dir).unwrap();
    let p = store.project().unwrap();
    let sid = text(&p["rootSequenceId"]);
    let mut seq = p["sequences"][sid].clone();
    seq["format"]["width"] = json!(160);
    seq["format"]["height"] = json!(90);
    seq["tracks"] = json!([{"id":id("track"),"name":"seek","kind":"video","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"media","assetId":aid,"streamId":"stream_v0","timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}},"sourceRange":{"start":{"num":6,"den":5},"duration":{"num":1,"den":1}}}]}]);
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":seq}),
            false,
            false,
        )
        .unwrap();
    drop(store);
    cli(&dir, &["render"]);
    let output = dir.join("output/render.mp4");
    let probe = media::probe(&output).unwrap();
    assert_eq!(probe["streams"][0]["nb_frames"], "30");
    let get = |path: &Path, filter: &str| {
        let r = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-vf",
                filter,
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .unwrap();
        assert!(r.status.success());
        r.stdout
    };
    let expected = get(&source, "select=eq(n\\,36)");
    let actual = get(&output, "select=eq(n\\,0)");
    assert_eq!(expected.len(), actual.len());
    let mean_error = expected
        .iter()
        .zip(&actual)
        .map(|(a, b)| (*a as f64 - *b as f64).abs())
        .sum::<f64>()
        / expected.len() as f64;
    assert!(
        mean_error < 8.0,
        "seek selected wrong frame: pixel MAE {mean_error}"
    );
}

#[test]
fn fractional_frame_rate_offsets_do_not_drop_the_last_active_frame() {
    if !available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("green.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=lime:s=160x90:r=60000/1001:d=6",
            "-c:v",
            "libx264",
        ],
        &source,
    );
    let dir = temp.path().join("fractional.cut");
    cli(&dir, &["init", dir.to_str().unwrap()]);
    let imported = cli(
        &dir,
        &[
            "import",
            source.to_str().unwrap(),
            "--color-space",
            "rec709",
        ],
    );
    let store = Store::open(&dir).unwrap();
    let p = store.project().unwrap();
    let sid = text(&p["rootSequenceId"]);
    let mut seq = p["sequences"][sid].clone();
    let frame = |n: i64, rate: i64| {
        cutcraft::time::Time::new(n * 1001, rate)
            .unwrap()
            .json()
            .unwrap()
    };
    seq["format"]["width"] = json!(160);
    seq["format"]["height"] = json!(90);
    seq["format"]["frameRate"] = json!({"num":30000,"den":1001});
    seq["durationMode"] = json!("fixed");
    seq["duration"] = frame(200, 30000);
    seq["background"] = json!({"r":1,"g":0,"b":0,"a":1});
    seq["tracks"] = json!([{"id":id("track"),"name":"fractional","kind":"video","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"media","assetId":imported["assetId"],"streamId":"stream_v0","timelineRange":{"start":frame(140,30000),"duration":frame(50,30000)},"sourceRange":{"start":frame(165,60000),"duration":frame(100,60000)}}]}]);
    seq["tracks"][0]["items"][0]["effects"] = json!([{"id":id("effect"),"effectType":"builtin.curves","version":"1.0.0","parameters":{}}]);
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":seq}),
            false,
            false,
        )
        .unwrap();
    drop(store);
    cli(&dir, &["render"]);
    let pixels = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(dir.join("output/render.mp4"))
        .args([
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .unwrap();
    assert!(pixels.status.success());
    assert_eq!(pixels.stdout.len(), 200 * 3);
    for n in [139usize, 190] {
        let p = &pixels.stdout[n * 3..n * 3 + 3];
        assert!(p[0] > 200 && p[1] < 40, "outside frame {n}: {p:?}");
    }
    for n in [140usize, 188, 189] {
        let p = &pixels.stdout[n * 3..n * 3 + 3];
        assert!(p[1] > 200 && p[0] < 40, "active frame {n}: {p:?}");
    }
}
