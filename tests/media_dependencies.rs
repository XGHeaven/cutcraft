use cutcraft::{id, store::Store};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn cli(project: &Path, path: &Path, args: &[&str], success: bool) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .env("PATH", path)
        .arg("-p")
        .arg(project)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        out.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if success {
        serde_json::from_slice(&out.stdout).unwrap()
    } else {
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        serde_json::from_slice(&out.stderr).unwrap()
    }
}

fn missing(value: &Value, executable: &str) {
    assert_eq!(value["error"]["code"], "MEDIA_TOOL_NOT_FOUND");
    assert_eq!(value["error"]["executable"], executable);
    assert!(value["error"]["message"]
        .as_str()
        .unwrap()
        .contains(executable));
    let hint = value["error"]["hint"].as_str().unwrap();
    assert!(
        hint.contains("PATH")
            && hint.contains("ffprobe -version")
            && hint.contains("ffmpeg -version")
    );
}

#[test]
fn missing_probe_is_actionable_and_json_operations_remain_available() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("empty-path");
    fs::create_dir(&path).unwrap();
    let project = temp.path().join("project.cut");
    cli(&project, &path, &["init", project.to_str().unwrap()], true);
    cli(&project, &path, &["inspect"], true);
    cli(&project, &path, &["validate"], true);
    cli(&project, &path, &["capabilities"], true);
    let snapshot = fs::read(project.join("cutcraft.json")).unwrap();
    let history = fs::read(project.join("history.jsonl")).unwrap();
    let source = temp.path().join("source.mp4");
    fs::write(&source, b"probe never starts").unwrap();
    missing(
        &cli(
            &project,
            &path,
            &["import", source.to_str().unwrap(), "--managed"],
            false,
        ),
        "ffprobe",
    );
    let frames = temp.path().join("frames");
    fs::create_dir(&frames).unwrap();
    fs::write(frames.join("frame_01.bmp"), b"BM0123456789012345").unwrap();
    missing(
        &cli(
            &project,
            &path,
            &[
                "import-sequence",
                frames.to_str().unwrap(),
                "--pattern",
                "frame_%02d.bmp",
                "--fps",
                "24",
                "--color-space",
                "srgb",
                "--managed",
            ],
            false,
        ),
        "ffprobe",
    );
    assert_eq!(fs::read(project.join("cutcraft.json")).unwrap(), snapshot);
    assert_eq!(fs::read(project.join("history.jsonl")).unwrap(), history);
    assert_eq!(fs::read_dir(project.join("assets")).unwrap().count(), 0);
}

#[test]
fn missing_encoder_is_actionable_for_preview_and_render_without_overwriting_output() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("empty-path");
    fs::create_dir(&path).unwrap();
    let project = temp.path().join("project.cut");
    let store = Store::init(&project, "missing encoder").unwrap();
    let p = store.project().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap();
    store.command(json!({"type":"track.put","sequenceId":sid,"track":{
        "id":id("track"),"kind":"video","name":"color","overlapPolicy":"forbid","items":[{
            "id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0",
            "parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},
            "timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":30}}
        }]
    }}), false, false).unwrap();
    drop(store);
    let snapshot = fs::read(project.join("cutcraft.json")).unwrap();
    missing(&cli(&project, &path, &["preview"], false), "ffmpeg");
    let output = project.join("output/existing.mp4");
    fs::write(&output, b"existing export").unwrap();
    missing(
        &cli(
            &project,
            &path,
            &[
                "render",
                "--output",
                output.to_str().unwrap(),
                "--overwrite",
            ],
            false,
        ),
        "ffmpeg",
    );
    assert_eq!(fs::read(&output).unwrap(), b"existing export");
    assert_eq!(fs::read(project.join("cutcraft.json")).unwrap(), snapshot);
}

#[cfg(unix)]
#[test]
fn permissions_and_failed_probe_are_not_reported_as_missing() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project.cut");
    drop(Store::init(&project, "permissions").unwrap());
    let path = temp.path().join("bin");
    fs::create_dir(&path).unwrap();
    let executable = path.join("ffprobe");
    fs::write(&executable, "#!/bin/sh\necho 'invalid input' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
    let source = temp.path().join("source.mp4");
    fs::write(&source, b"test").unwrap();
    let args = ["import", source.to_str().unwrap()];
    let denied = cli(&project, &path, &args, false);
    assert_eq!(denied["error"]["code"], "MEDIA_TOOL_PERMISSION_DENIED");
    assert_eq!(denied["error"]["executable"], "ffprobe");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let failed = cli(&project, &path, &args, false);
    assert!(failed["error"]["code"].is_null());
    assert!(failed["error"]["message"]
        .as_str()
        .unwrap()
        .contains("invalid input"));
}
