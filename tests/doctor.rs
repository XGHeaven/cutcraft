use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn invoke(dir: &Path, path: Option<&Path>, extra: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cutcraft"));
    cmd.current_dir(dir)
        .env("CUTCRAFT_RUNTIME_DIR", dir.join("runtime"))
        .args(["doctor"]);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd.args(extra).output().unwrap()
}
fn report(out: Output, success: bool) -> Value {
    assert_eq!(
        out.status.code(),
        Some(if success { 0 } else { 1 }),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], success);
    assert_eq!(value["command"], "doctor");
    value
}
fn check<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap()
}
#[test]
fn missing_tools_report_both_failures_without_a_project_or_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bin");
    fs::create_dir(&path).unwrap();
    let value = report(invoke(tmp.path(), Some(&path), &[]), false);
    for name in ["ffmpeg", "ffprobe"] {
        let c = check(&value, &format!("{name}.version"));
        assert_eq!(c["code"], "MEDIA_TOOL_NOT_FOUND");
        assert!(c["hint"].as_str().unwrap().contains("PATH"));
    }
    assert_eq!(value["summary"]["failed"], 2);
    assert_eq!(check(&value, "smoke.textPng")["status"], "skipped");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
}
#[test]
fn real_environment_encodes_probes_and_decodes_without_changing_project() {
    let available = ["ffmpeg", "ffprobe"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    if !available {
        assert!(std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project.cut");
    drop(cutcraft::store::Store::init(&project, "doctor").unwrap());
    let before = fs::read(project.join("cutcraft.json")).unwrap();
    let history = fs::read(project.join("history.jsonl")).unwrap();
    let value = report(invoke(&project, None, &[]), true);
    for id in [
        "ffmpeg.encoders",
        "ffmpeg.filters",
        "ffmpeg.graphFile",
        "smoke.textPng",
        "smoke.lossless",
        "smoke.mp4",
        "smoke.probe",
        "smoke.decodeVideo",
        "smoke.decodeAudio",
    ] {
        assert_eq!(check(&value, id)["status"], "passed", "{value}");
    }
    assert_eq!(fs::read(project.join("cutcraft.json")).unwrap(), before);
    assert_eq!(fs::read(project.join("history.jsonl")).unwrap(), history);
    assert!(!project.join("runtime").exists());
    assert!(fs::read_dir(project.join("cache"))
        .unwrap()
        .next()
        .is_none());
}
#[test]
fn timeout_validation_is_enforced_on_cli_and_host() {
    use clap::Parser;
    use cutcraft::app::{self, Cli};
    for invalid in ["0", "121", "-1"] {
        assert!(Cli::try_parse_from(["cutcraft", "doctor", "--timeout", invalid]).is_err());
    }
    assert!(Cli::try_parse_from(["cutcraft", "doctor", "--timeout", "20"]).is_ok());
    let mut value =
        serde_json::to_value(Cli::try_parse_from(["cutcraft", "doctor"]).unwrap()).unwrap();
    value["command"]["Doctor"]["timeout"] = serde_json::json!(0);
    assert!(app::ApplicationHost
        .execute(serde_json::from_value(value).unwrap())
        .unwrap_err()
        .to_string()
        .contains("DOCTOR_INVALID_TIMEOUT"));
}

#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
#[cfg(unix)]
#[test]
fn permission_failure_and_timeout_are_distinct_and_do_not_stop_other_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bin");
    fs::create_dir(&path).unwrap();
    script(&path, "ffmpeg", "exit 0", 0o644);
    script(&path, "ffprobe", "echo broken >&2; exit 2", 0o755);
    let value = report(invoke(tmp.path(), Some(&path), &[]), false);
    assert_eq!(
        check(&value, "ffmpeg.version")["code"],
        "MEDIA_TOOL_PERMISSION_DENIED"
    );
    assert_eq!(
        check(&value, "ffprobe.version")["code"],
        "MEDIA_PROCESS_FAILED"
    );
    assert_eq!(check(&value, "ffprobe.version")["exitCode"], 2);
    script(&path, "ffmpeg", "while :; do :; done", 0o755);
    script(&path, "ffprobe", "echo 'ffprobe version 7.1'", 0o755);
    let began = std::time::Instant::now();
    let value = report(invoke(tmp.path(), Some(&path), &["--timeout", "1"]), false);
    assert_eq!(
        check(&value, "ffmpeg.version")["code"],
        "MEDIA_TOOL_TIMEOUT"
    );
    assert_eq!(check(&value, "ffprobe.version")["status"], "passed");
    assert!(began.elapsed() < std::time::Duration::from_secs(10));
}
#[cfg(unix)]
#[test]
fn incomplete_build_lists_missing_components_and_reports_smoke_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bin");
    fs::create_dir(&path).unwrap();
    script(
        &path,
        "ffmpeg",
        r#"case "$*" in
    '-version') echo 'ffmpeg version 5.1';;
    '-hide_banner -encoders') echo ' A..... aac AAC encoder';;
    '-hide_banner -filters') echo ' ... scale V->V scale';;
    '-hide_banner -h full') echo '-filter_complex_script filename';;
    *) echo 'encoder unavailable' >&2; exit 1;;
esac"#,
        0o755,
    );
    script(
        &path,
        "ffprobe",
        "echo 'ffprobe version N-12345-gabcdef'",
        0o755,
    );
    let value = report(invoke(tmp.path(), Some(&path), &[]), false);
    assert_eq!(
        check(&value, "ffmpeg.version")["code"],
        "MEDIA_TOOL_VERSION_UNSUPPORTED"
    );
    assert_eq!(check(&value, "ffprobe.version")["status"], "warning");
    for (id, missing) in [
        ("ffmpeg.encoders", "libx264"),
        ("ffmpeg.filters", "drawtext"),
    ] {
        assert_eq!(check(&value, id)["code"], "MEDIA_CAPABILITY_MISSING");
        assert!(check(&value, id)["missing"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(missing)));
    }
    assert_eq!(check(&value, "ffmpeg.graphFile")["status"], "passed");
    assert_eq!(check(&value, "smoke.textPng")["status"], "failed");
    assert_eq!(check(&value, "smoke.lossless")["status"], "failed");
    assert_eq!(check(&value, "smoke.mp4")["status"], "failed");
    assert_eq!(check(&value, "smoke.probe")["status"], "skipped");
}

#[test]
fn selected_encoder_is_encoded_probed_and_pixel_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let mut names = vec!["libx264".to_owned()];
    if let Ok(name) = std::env::var("CUTCRAFT_TEST_VIDEO_ENCODER") {
        names.push(name);
    }
    for name in names {
        let value = report(invoke(tmp.path(), None, &["--video-encoder", &name]), true);
        assert_eq!(value["selectedVideoEncoder"], name);
        for id in ["encoder.encode", "encoder.probe", "encoder.decode"] {
            assert_eq!(check(&value, id)["status"], "passed", "{value}");
        }
    }
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn advertised_encoder_with_broken_device_or_timeout_is_not_available() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bin");
    fs::create_dir(&path).unwrap();
    script(&path, "ffprobe", "echo 'ffprobe version 7.1'", 0o755);
    for fail in [
        "echo 'NVENC device unavailable' >&2; exit 1",
        "while :; do :; done",
    ] {
        let body = format!(
            r#"case "$*" in
'-version') echo 'ffmpeg version 7.1';;
'-hide_banner -encoders') printf ' V..... libx264\n A..... aac\n V..... ffv1\n V..... png\n A..... pcm_f32le\n V..... h264_nvenc\n';;
'-hide_banner -h full') echo '-filter_complex_script filename';;
*h264_nvenc*) {fail};;
*) exit 1;;
esac"#
        );
        script(&path, "ffmpeg", &body, 0o755);
        let value = report(
            invoke(
                tmp.path(),
                Some(&path),
                &["--video-encoder", "h264_nvenc", "--timeout", "1"],
            ),
            false,
        );
        assert_eq!(check(&value, "ffmpeg.encoders")["status"], "passed");
        assert_eq!(check(&value, "encoder.encode")["status"], "failed");
        assert_eq!(
            check(&value, "encoder.encode")["videoEncoder"],
            "h264_nvenc"
        );
        assert_eq!(
            check(&value, "encoder.encode")["code"],
            if fail.starts_with("while") {
                "MEDIA_TOOL_TIMEOUT"
            } else {
                "MEDIA_PROCESS_FAILED"
            }
        );
        assert_eq!(check(&value, "encoder.decode")["status"], "skipped");
    }
}
