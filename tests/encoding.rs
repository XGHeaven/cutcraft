use clap::Parser;
use cutcraft::{
    app::{self, Cli, Cmd},
    encoding::{EncodingOptions, VideoEncoder},
    id, preview,
    progress::Progress,
    project, render,
    time::Time,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn fixture(dir: &Path) -> (Value, String) {
    let mut p = project::new("encoding");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(128);
    seq["format"]["height"] = json!(96);
    seq["format"]["frameRate"] = Time::integer(12).json().unwrap();
    seq["tracks"] = json!([{"id":id("track"),"kind":"video","name":"Color","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}}]}]);
    fs::write(dir.join("cutcraft.json"), project::canonical(&p).unwrap()).unwrap();
    (p, sid)
}
fn pixels(path: &Path, frames: usize) {
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(decoded.stdout.len(), 128 * 96 * 3 * frames);
    assert!(decoded
        .stdout
        .chunks_exact(3)
        .all(|p| p[0] > 230 && p[1] < 20 && p[2] < 20));
}
fn cli(dir: &Path, args: &[&str]) -> anyhow::Result<Value> {
    let mut all = vec!["cutcraft", "-p", dir.to_str().unwrap()];
    all.extend_from_slice(args);
    let cli = app::prepare(Cli::try_parse_from(all).unwrap())?;
    // Exercise the same typed JSON request used for daemon forwarding.
    app::ApplicationHost
        .execute(serde_json::from_value(serde_json::to_value(cli).unwrap()).unwrap())
}
#[test]
fn cli_encoding_transport_defaults_and_validation() {
    for name in ["libx264", "h264_videotoolbox", "h264_nvenc", "h264_qsv"] {
        let request = Cli::try_parse_from([
            "cutcraft",
            "render",
            "--video-encoder",
            name,
            "--video-bitrate",
            "4000000",
        ])
        .unwrap();
        let wire = serde_json::to_value(&request).unwrap();
        let restored: Cli = serde_json::from_value(wire).unwrap();
        let Cmd::Render { encoding, .. } = restored.command else {
            panic!()
        };
        assert_eq!(encoding.video_encoder.name(), name);
        assert_eq!(encoding.video_bitrate, Some(4_000_000));
    }
    let request = Cli::try_parse_from(["cutcraft", "render"]).unwrap();
    let mut wire = serde_json::to_value(&request).unwrap();
    wire["command"]["Render"]
        .as_object_mut()
        .unwrap()
        .remove("encoding");
    let restored: Cli = serde_json::from_value(wire).unwrap();
    let Cmd::Render { encoding, .. } = restored.command else {
        panic!()
    };
    assert!(encoding.is_default());
    for value in ["0", "1000000001", "-1", "NaN"] {
        assert!(Cli::try_parse_from(["cutcraft", "render", "--video-bitrate", value]).is_err());
    }
    assert!(Cli::try_parse_from(["cutcraft", "render", "--video-encoder", "unknown"]).is_err());
    assert!(EncodingOptions {
        video_bitrate: Some(0),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(serde_json::from_value::<EncodingOptions>(json!({"unknown":true})).is_err());
}
#[test]
fn software_export_preview_cache_and_legacy_preserve_project() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture(tmp.path());
    let before = fs::read(tmp.path().join("cutcraft.json")).unwrap();
    let result = cli(
        tmp.path(),
        &[
            "render",
            "--video-encoder",
            "libx264",
            "--video-bitrate",
            "4000000",
            "--progress",
            "none",
        ],
    )
    .unwrap();
    assert_eq!(result["encoding"]["videoBitrate"], 4_000_000);
    pixels(Path::new(result["output"].as_str().unwrap()), 12);
    let default = cli(tmp.path(), &["preview", "--progress", "none"]).unwrap();
    let configured = cli(
        tmp.path(),
        &[
            "preview",
            "--video-bitrate",
            "1000000",
            "--progress",
            "none",
        ],
    )
    .unwrap();
    assert_ne!(default["output"], configured["output"]);
    pixels(Path::new(configured["output"].as_str().unwrap()), 12);
    let default = cli(
        tmp.path(),
        &["preview", "--duration", "1/2", "--progress", "none"],
    )
    .unwrap();
    let configured = cli(
        tmp.path(),
        &[
            "preview",
            "--duration",
            "1/2",
            "--video-bitrate",
            "1000000",
            "--progress",
            "none",
        ],
    )
    .unwrap();
    assert_ne!(default["output"], configured["output"]);
    pixels(Path::new(configured["output"].as_str().unwrap()), 6);
    for args in [
        vec!["preview", "--at", "0"],
        vec!["preview", "--sheet"],
        vec!["preview", "--boundaries"],
    ] {
        let mut args = args;
        args.extend(["--video-encoder", "h264_videotoolbox"]);
        assert!(cli(tmp.path(), &args)
            .unwrap_err()
            .to_string()
            .contains("ENCODING_OPTIONS_CONFLICT"));
    }
    assert_eq!(fs::read(tmp.path().join("cutcraft.json")).unwrap(), before);
    p["specVersion"] = json!("0.3.0");
    p["$schema"] = json!("https://cutstate.dev/schema/v0.3/project.schema.json");
    project::validate(&p).unwrap();
    let result = render::render_with_options(
        &p,
        tmp.path(),
        &sid,
        &tmp.path().join("legacy.mp4"),
        false,
        &EncodingOptions {
            video_bitrate: Some(4_000_000),
            ..Default::default()
        },
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(result["backend"], "ffmpeg-basic");
    pixels(Path::new(result["output"].as_str().unwrap()), 12);
}
#[test]
fn selected_hardware_encoder_actual_media() {
    // Opt-in on a machine with the requested hardware; CI does not pretend all
    // adapters have been exercised merely because FFmpeg lists them.
    let Ok(name) = std::env::var("CUTCRAFT_TEST_VIDEO_ENCODER") else {
        return;
    };
    let request = Cli::try_parse_from(["cutcraft", "render", "--video-encoder", &name]).unwrap();
    let Cmd::Render { encoding, .. } = request.command else {
        panic!()
    };
    assert_ne!(encoding.video_encoder, VideoEncoder::Libx264);
    let tmp = tempfile::tempdir().unwrap();
    let (p, sid) = fixture(tmp.path());
    let path = tmp.path().join("hardware.mp4");
    let result = render::render_with_options(
        &p,
        tmp.path(),
        &sid,
        &path,
        false,
        &encoding,
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(result["encoding"]["videoEncoder"], name);
    pixels(&path, 12);
    let full = render::preview_with_encoding(&p, tmp.path(), &sid, &encoding, &Progress::default())
        .unwrap();
    pixels(Path::new(full["output"].as_str().unwrap()), 12);
    let range = preview::export_with_encoding(
        &p,
        tmp.path(),
        &sid,
        &preview::ExportOptions {
            duration: Some(Time::new(1, 2).unwrap()),
            ..Default::default()
        },
        &encoding,
        &Progress::default(),
    )
    .unwrap();
    pixels(Path::new(range["output"].as_str().unwrap()), 6);
}

#[test]
fn unavailable_hardware_preserves_existing_output() {
    let listed = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    let names = String::from_utf8_lossy(&listed.stdout);
    let missing = [
        VideoEncoder::H264Videotoolbox,
        VideoEncoder::H264Nvenc,
        VideoEncoder::H264Qsv,
    ]
    .into_iter()
    .find(|e| !names.split_whitespace().any(|n| n == e.name()));
    let Some(video_encoder) = missing else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (p, sid) = fixture(tmp.path());
    let path = tmp.path().join("existing.mp4");
    fs::write(&path, b"preserve existing result").unwrap();
    let error = render::render_with_options(
        &p,
        tmp.path(),
        &sid,
        &path,
        true,
        &EncodingOptions {
            video_encoder,
            video_bitrate: None,
        },
        &Progress::default(),
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains(video_encoder.name()), "{message}");
    assert_eq!(fs::read(path).unwrap(), b"preserve existing result");
    assert!(!fs::read_dir(tmp.path())
        .unwrap()
        .any(|e| e.unwrap().file_type().unwrap().is_dir()));
}
