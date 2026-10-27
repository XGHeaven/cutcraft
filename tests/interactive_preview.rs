use cutcraft::{
    id, preview,
    progress::{CancellationToken, Progress},
    project,
    time::Time,
};
use serde_json::{json, Value};
use std::{fs, process::Command, sync::Arc};
fn fixture() -> (Value, String) {
    let mut p = project::new("interactive");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(64);
    seq["format"]["height"] = json!(48);
    seq["format"]["frameRate"] = Time::integer(12).json().unwrap();
    seq["tracks"] = json!([{"id":id("track"),"kind":"video","name":"Color","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":3,"den":1}}}]}]);
    (p, sid)
}
fn available() -> bool {
    let ok = Command::new("ffmpeg").arg("-version").output().is_ok();
    assert!(ok || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    ok
}
#[test]
fn png_and_fragmented_playback_preserve_revision_and_pixels() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (p, sid) = fixture();
    let original = p.clone();
    let progress = Progress::default();
    let frame = preview::frame(&p, dir.path(), &sid, &Time::new(1, 2).unwrap(), &progress).unwrap();
    assert!(frame.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(frame.metadata["revision"], p["revision"]);
    let png = dir.path().join("frame.png");
    fs::write(&png, &frame.bytes).unwrap();
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&png)
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
        .output()
        .unwrap();
    assert!(decoded.status.success());
    assert!(decoded
        .stdout
        .chunks_exact(3)
        .all(|c| c[0] > 245 && c[1] < 10 && c[2] < 10));
    let out = dir.path().join("session");
    let result =
        preview::playback(&p, dir.path(), &sid, &Time::integer(1), &out, &progress).unwrap();
    assert_eq!(result["startTime"], Time::integer(1).json().unwrap());
    let playlist = fs::read_to_string(out.join("stream.m3u8")).unwrap();
    assert!(playlist.contains("#EXT-X-MAP:URI=\"init.mp4\""));
    assert!(playlist.contains("#EXT-X-ENDLIST"));
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(out.join("stream.m3u8"))
        .args(["-an", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(decoded.stdout.len(), 64 * 48 * 3 * 24);
    assert!(decoded
        .stdout
        .chunks_exact(3)
        .all(|c| c[0] > 235 && c[1] < 15 && c[2] < 15));
    assert_eq!(p, original);
}
#[test]
fn cancellation_does_not_publish_completion() {
    let dir = tempfile::tempdir().unwrap();
    let (p, sid) = fixture();
    let cancel = CancellationToken::default();
    cancel.cancel();
    let progress = Progress::with_callback(
        cancel,
        Arc::new(|_| panic!("cancelled job emitted progress")),
    );
    assert!(
        preview::frame(&p, dir.path(), &sid, &Time::zero(), &progress)
            .unwrap_err()
            .to_string()
            .contains("JOB_CANCELLED")
    );
    assert!(!dir.path().join("cache").exists());
}

#[test]
fn thumbnails_and_native_channel_waveforms_are_backend_artifacts() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut p, _sid) = fixture();
    let source = dir.path().join("av.mp4");
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=64x48:rate=12:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(out.status.success());
    let aid = id("asset");
    p["assets"][&aid] = cutcraft::media::asset(&source, "av.mp4", false).unwrap();
    p["assets"][&aid]["streams"][0]["video"]["colorSpace"] = json!("rec709");
    let progress = Progress::default();
    let a = preview::thumbnail(&p, dir.path(), &aid, "stream_v0", &progress).unwrap();
    let b = preview::thumbnail(&p, dir.path(), &aid, "stream_v0", &progress).unwrap();
    assert_eq!(a.bytes, b.bytes);
    let waveform = preview::waveform(&p, dir.path(), &aid, "stream_a1", &progress).unwrap();
    assert_eq!(waveform["peaks"].as_array().unwrap().len(), 1024);
    assert!(waveform["peaks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_f64().unwrap() > 0.1));
}

#[test]
fn fractional_seek_preserves_zero_video_origin_on_unaligned_audio_grid() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    p["sequences"][&sid]["format"]["frameRate"] = Time::integer(23).json().unwrap();
    p["sequences"][&sid]["format"]["audioSampleRate"] = json!(22050);
    p["sequences"][&sid]["tracks"][0]["items"][0]["timelineRange"]["duration"] =
        Time::integer(1).json().unwrap();
    let origin = Time::new(1, 23).unwrap();
    let output = dir.path().join("session");
    let metadata =
        preview::playback(&p, dir.path(), &sid, &origin, &output, &Progress::default()).unwrap();
    assert_eq!(metadata["startTime"], origin.json().unwrap());
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(output.join("stream.m3u8"))
        .output()
        .unwrap();
    assert!(probe.status.success());
    let probe: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let video = probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["codec_type"] == "video")
        .unwrap();
    assert_eq!(video["start_time"], "0.000000");
    let frames = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(output.join("stream.m3u8"))
        .args(["-an", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
        .output()
        .unwrap();
    assert!(frames.status.success());
    assert_eq!(frames.stdout.len(), 64 * 48 * 3 * 22);
}

#[test]
fn long_playback_keeps_a_sliding_fragment_window() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    p["sequences"][&sid]["tracks"][0]["items"][0]["timelineRange"]["duration"] =
        Time::integer(15).json().unwrap();
    let output = dir.path().join("session");
    let metadata = preview::playback(
        &p,
        dir.path(),
        &sid,
        &Time::zero(),
        &output,
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(metadata["playlistWindowSegments"], 24);
    let playlist = fs::read_to_string(output.join("stream.m3u8")).unwrap();
    assert!(playlist.contains("#EXT-X-MEDIA-SEQUENCE:6"), "{playlist}");
    assert_eq!(playlist.lines().filter(|s| s.ends_with(".m4s")).count(), 24);
    assert!(output.join("init.mp4").is_file());
    assert!(!output.join("part-000000.m4s").exists());
    let retained = fs::read_dir(&output)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|v| v == "m4s"))
        .count();
    assert!(retained <= 26, "retained {retained} fragments");
}

#[test]
fn continuous_preview_scale_changes_pixels_and_cache_without_editing_project() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (p, sid) = fixture();
    let original = p.clone();
    let progress = Progress::default();
    for (scale, width, height) in [
        (1., 64u32, 48u32),
        (0.5, 32, 24),
        (0.25, 16, 12),
        (0.37, 22, 16),
        (0.001, 2, 2),
    ] {
        let frame =
            preview::frame_scaled(&p, dir.path(), &sid, &Time::zero(), &progress, Some(scale))
                .unwrap();
        assert_eq!(frame.metadata["width"], width);
        assert_eq!(frame.metadata["height"], height);
        // Inspect actual PNG IHDR, not just the response metadata.
        assert_eq!(
            u32::from_be_bytes(frame.bytes[16..20].try_into().unwrap()),
            width
        );
        assert_eq!(
            u32::from_be_bytes(frame.bytes[20..24].try_into().unwrap()),
            height
        );
        let cached =
            preview::frame_scaled(&p, dir.path(), &sid, &Time::zero(), &progress, Some(scale))
                .unwrap();
        assert_eq!(cached.bytes, frame.bytes);
    }
    let out = dir.path().join("scaled-playback");
    let meta = preview::playback_scaled(
        &p,
        dir.path(),
        &sid,
        &Time::integer(2),
        &out,
        &progress,
        Some(0.37),
    )
    .unwrap();
    assert_eq!(meta["width"], 22);
    assert_eq!(meta["height"], 16);
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(out.join("stream.m3u8"))
        .args(["-an", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
        .output()
        .unwrap();
    assert!(decoded.status.success());
    assert_eq!(decoded.stdout.len(), 22 * 16 * 3 * 12);
    assert!(decoded
        .stdout
        .chunks_exact(3)
        .all(|c| c[0] > 235 && c[1] < 15 && c[2] < 15));
    assert_eq!(p, original);
    for invalid in [0., -0.1, 1.01, f64::NAN, f64::INFINITY] {
        assert!(preview::frame_scaled(
            &p,
            dir.path(),
            &sid,
            &Time::zero(),
            &progress,
            Some(invalid)
        )
        .unwrap_err()
        .to_string()
        .contains("INVALID_PREVIEW_SCALE"));
    }
}
