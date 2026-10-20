use cutcraft::{
    id, media,
    project::{self, text},
    render,
    time::{range, Time},
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
fn t(n: i64) -> Value {
    json!({"num":n,"den":1})
}
fn curve(kind: &str, keys: Value) -> Value {
    json!({"$animation":{"version":"1.0.0","valueType":kind,"timeReference":"owner","keyframes":keys}})
}
fn fixture() -> (Value, String) {
    let mut p = project::new("render");
    let sid = text(&p["rootSequenceId"]).to_string();
    p["sequences"][&sid]["format"]["width"] = json!(32);
    p["sequences"][&sid]["format"]["height"] = json!(32);
    p["sequences"][&sid]["format"]["frameRate"] = t(4);
    (p, sid)
}
fn color(start: i64, duration: i64, c: [f64; 3]) -> Value {
    json!({"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":c[0],"g":c[1],"b":c[2],"a":1}},"timelineRange":{"start":t(start),"duration":t(duration)}})
}
fn track(items: Value) -> Value {
    json!({"id":id("track"),"name":"V","kind":"video","overlapPolicy":"forbid","items":items})
}
fn decoded(path: &Path) -> Vec<[u8; 3]> {
    let r = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    r.stdout
        .chunks_exact(32 * 32 * 3)
        .map(|f| {
            let i = (16 * 32 + 16) * 3;
            [f[i], f[i + 1], f[i + 2]]
        })
        .collect()
}
#[test]
fn transition_linear_light_and_animated_opacity_export() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    let a = color(0, 1, [1., 0., 0.]);
    let b = color(1, 1, [0., 0., 1.]);
    p["sequences"][&sid]["tracks"] = json!([track(json!([a, b]))]);
    p["sequences"][&sid]["transitions"] = json!([{"id":id("transition"),"fromItemId":a["id"],"toItemId":b["id"],"transitionType":"builtin.crossfade","version":"2.0.0","alignment":"center","duration":t(1)}]);
    let out = tmp.path().join("crossfade.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let pixels = decoded(&out);
    assert_eq!(pixels.len(), 8);
    assert!(pixels[0][0] > 240 && pixels[0][2] < 10);
    assert!(
        pixels[4][0] > 170 && pixels[4][2] > 170,
        "linear-light midpoint: {:?}",
        pixels[4]
    );
    assert!(pixels[7][2] > 240 && pixels[7][0] < 10);
    p["sequences"][&sid]["transitions"][0]["transitionType"] = json!("builtin.dip-to-color");
    p["sequences"][&sid]["transitions"][0]["parameters"] =
        json!({"color":{"r":0,"g":1,"b":0,"a":1}});
    let dip = tmp.path().join("dip.mp4");
    render::render(&p, tmp.path(), &sid, &dip, false).unwrap();
    let pixels = decoded(&dip);
    assert!(pixels[4][1] > 240 && pixels[4][0] < 10 && pixels[4][2] < 10);
    p["sequences"][&sid]["transitions"] = json!([]);
    p["sequences"][&sid]["tracks"][0]["items"] = json!([color(0, 2, [1., 0., 0.])]);
    p["sequences"][&sid]["tracks"][0]["items"][0]["video"]["opacity"] = curve(
        "number",
        json!([{"offset":t(0),"value":0},{"offset":t(2),"value":1}]),
    );
    let out = tmp.path().join("opacity.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let pixels = decoded(&out);
    assert!(pixels[0][0] < 10);
    assert!((170..200).contains(&pixels[4][0]));
    // A runtime cross-property crop error must not replace a previously valid output.
    let old = fs::read(&out).unwrap();
    p["sequences"][&sid]["tracks"][0]["items"][0]["video"]["transform"]["crop"] = json!({"left":curve("number",json!([{"offset":t(0),"value":0},{"offset":t(1),"value":0.8}])),"right":0.3});
    assert!(render::render(&p, tmp.path(), &sid, &out, true).is_err());
    assert_eq!(fs::read(&out).unwrap(), old);
}
#[test]
fn actual_source_mapping_freeze_reverse_and_frame_blend() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("source.rgb");
    let mut data = vec![];
    for c in [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 255],
        [255, 255, 255],
    ] {
        for _ in 0..32 * 32 {
            data.extend_from_slice(&c);
        }
    }
    fs::write(&raw, data).unwrap();
    let file = tmp.path().join("source.nut");
    let r = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            "32x32",
            "-framerate",
            "4",
            "-i",
        ])
        .arg(&raw)
        .args([
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "gbrp",
            "-color_trc",
            "bt709",
            "-color_primaries",
            "bt709",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(r.status.success());
    let (mut p, sid) = fixture();
    let aid = id("asset");
    let mut asset = media::asset(
        &file,
        url::Url::from_file_path(&file).unwrap().as_str(),
        false,
    )
    .unwrap();
    asset["streams"][0]["video"]["colorSpace"] = json!("rec709");
    let stream = asset["streams"][0]["id"].clone();
    p["assets"][&aid] = asset;
    let map = curve(
        "time",
        json!([{"offset":t(0),"value":t(0)},{"offset":{"num":1,"den":2},"value":{"num":1,"den":2}},{"offset":t(1),"value":{"num":1,"den":2}},{"offset":t(2),"value":t(0)}]),
    );
    let item = json!({"id":id("item"),"type":"media","assetId":aid,"streamId":stream,"timelineRange":range(&Time::zero(),&Time::integer(2)).unwrap(),"sourceRange":range(&Time::zero(),&Time::integer(1)).unwrap(),"playback":{"mode":"remap","version":"1.0.0","timeMap":map}});
    p["sequences"][&sid]["tracks"] = json!([track(json!([item]))]);
    let out = tmp.path().join("remap.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let pixels = decoded(&out);
    let dominant: Vec<_> = pixels
        .iter()
        .map(|p| p.iter().enumerate().max_by_key(|(_, v)| **v).unwrap().0)
        .collect();
    assert_eq!(dominant, vec![0, 1, 2, 2, 1, 1, 0, 0], "{pixels:?}");
    p["sequences"][&sid]["tracks"][0]["items"][0]["playback"] =
        json!({"mode":"fit","videoSampling":{"method":"frame-blend"}});
    let out = tmp.path().join("blend.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let pixels = decoded(&out);
    assert!(pixels[1][0] > 170 && pixels[1][1] > 170, "{:?}", pixels[1]);
}

#[test]
fn audio_remap_freeze_and_sample_clock_pan() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("tone.wav");
    let r = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=8000:duration=1",
            "-c:a",
            "pcm_f32le",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(r.status.success());
    let (mut p, sid) = fixture();
    let aid = id("asset");
    let asset = media::asset(
        &file,
        url::Url::from_file_path(&file).unwrap().as_str(),
        false,
    )
    .unwrap();
    let stream = asset["streams"][0]["id"].clone();
    p["assets"][&aid] = asset;
    p["sequences"][&sid]["format"]["audioSampleRate"] = json!(8000);
    let quarter = json!({"num":1,"den":4});
    let half = json!({"num":1,"den":2});
    let map = curve(
        "time",
        json!([{"offset":t(0),"value":t(0)},{"offset":quarter,"value":quarter},{"offset":half,"value":quarter},{"offset":t(1),"value":t(0)}]),
    );
    let item = json!({"id":id("item"),"type":"media","assetId":aid,"streamId":stream,"timelineRange":{"start":t(0),"duration":t(1)},"sourceRange":{"start":t(0),"duration":t(1)},"playback":{"mode":"remap","version":"1.0.0","timeMap":map,"audioPolicy":{"mode":"resample","processor":{"id":"cutcraft.linear-resample","version":"1.0.0","parameters":{}}}},"audio":{"pan":curve("number",json!([{"offset":t(0),"value":-1},{"offset":quarter,"value":1}]))}});
    p["sequences"][&sid]["tracks"] = json!([{"id":id("track"),"name":"A","kind":"audio","overlapPolicy":"forbid","items":[item]}]);
    let out = tmp.path().join("audio.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let r = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vn", "-ar", "8000", "-ac", "2", "-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(r.status.success());
    let pcm: Vec<_> = r
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64)
        .collect();
    let rms = |start: usize, end: usize, c: usize| {
        ((start..end).map(|i| pcm[i * 2 + c].powi(2)).sum::<f64>() / (end - start) as f64).sqrt()
    };
    assert!(rms(200, 500, 0) > rms(200, 500, 1) * 2.);
    assert!(rms(1500, 1800, 1) > rms(1500, 1800, 0) * 2.);
    assert!(rms(2900, 3400, 1) < 0.003, "freeze must be silence");
    assert!(
        rms(5000, 6000, 1) > 0.03,
        "reverse interval must contain sound"
    );
}
