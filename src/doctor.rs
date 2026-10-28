//! Local, project-independent diagnostics. Every external check is bounded.
use crate::{
    media_process::{self, MediaCommand, MediaToolError},
    progress::CancellationToken,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const ENCODERS: &[&str] = &["libx264", "aac", "ffv1", "png", "pcm_f32le"];
const FILTERS: &[&str] = &[
    "color",
    "drawtext",
    "lutrgb",
    "colorchannelmixer",
    "format",
    "scale",
    "setparams",
    "trim",
    "setpts",
    "settb",
    "setsar",
    "pad",
    "overlay",
    "anullsrc",
    "atrim",
    "aresample",
    "aformat",
    "volume",
    "adelay",
    "amix",
];
const BUILD_HINT: &str = "Use a full FFmpeg 6.1+ build with the listed components and ffprobe. Check PATH for conflicting installations; restart the editor service after changing PATH. See docs/doctor.md.";

fn timed<T>(seconds: u64, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let token = CancellationToken::default();
    let cancel = token.clone();
    let (done, wait) = mpsc::channel();
    let watchdog = thread::spawn(move || {
        if matches!(
            wait.recv_timeout(Duration::from_secs(seconds)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            cancel.cancel();
        }
    });
    let result = media_process::with_cancellation(token.clone(), work);
    let _ = done.send(());
    let _ = watchdog.join();
    ensure!(
        !token.is_cancelled(),
        "MEDIA_TOOL_TIMEOUT: external check exceeded {seconds} seconds"
    );
    result
}
fn excerpt(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(4096)
        .collect::<String>()
        .trim()
        .to_owned()
}
fn version(line: &str) -> Option<(u64, u64)> {
    let raw = line.split_whitespace().nth(2)?.trim_start_matches('n');
    let mut parts = raw.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some((major, minor.parse().ok()?))
}
fn candidates(tool: &str) -> Vec<PathBuf> {
    let filename = if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_owned()
    };
    let mut paths = Vec::new();
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = dir.join(&filename);
        if path.is_file() {
            let path = path.canonicalize().unwrap_or(path);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}
struct Report {
    checks: Vec<Value>,
    timeout: u64,
}
impl Report {
    fn record(&mut self, id: &str, status: &str, message: &str) {
        self.checks
            .push(json!({"id":id,"status":status,"message":message}));
    }
    fn error(&mut self, id: &str, error: &anyhow::Error, hint: &str) {
        let (code, hint) = if let Some(tool) = error.downcast_ref::<MediaToolError>() {
            (tool.code(), tool.hint())
        } else if error.to_string().contains("MEDIA_TOOL_TIMEOUT") {
            (
                "MEDIA_TOOL_TIMEOUT",
                "Check the executable or increase --timeout (maximum 120 seconds per check)."
                    .to_owned(),
            )
        } else {
            ("MEDIA_CHECK_FAILED", hint.to_owned())
        };
        self.checks.push(json!({"id":id,"status":"failed","code":code,"message":format!("{error:#}"),"hint":hint}));
    }
    fn command(&mut self, id: &str, command: &mut Command, hint: &str) -> Option<Output> {
        // No child may consume the caller's stdin, including wrappers on PATH.
        command.stdin(Stdio::null());
        match timed(self.timeout, || command.media_output()) {
            Ok(output) if output.status.success() => {
                self.record(id, "passed", "Command completed successfully");
                Some(output)
            }
            Ok(output) => {
                self.checks.push(json!({"id":id,"status":"failed","code":"MEDIA_PROCESS_FAILED","exitCode":output.status.code(),"message":excerpt(&output.stderr),"hint":hint}));
                None
            }
            Err(error) => {
                self.error(id, &error, hint);
                None
            }
        }
    }
    fn tool(&mut self, name: &str) -> bool {
        let id = format!("{name}.version");
        let output = self.command(&id, Command::new(name).arg("-version"), BUILD_HINT);
        let check = self.checks.last_mut().unwrap();
        check["executable"] = json!(name);
        check["pathCandidates"] = json!(candidates(name));
        let Some(output) = output else {
            return false;
        };
        let header = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        check["version"] = json!(header);
        if !header.starts_with(&format!("{name} version ")) {
            check["status"] = json!("failed");
            check["code"] = json!("MEDIA_TOOL_INVALID_VERSION");
            check["message"] =
                json!("Executable did not report the expected FFmpeg version header");
            check["hint"] = json!(BUILD_HINT);
            return false;
        }
        match version(&header) {
            Some(v) if v < (6, 1) => {
                check["status"] = json!("failed");
                check["code"] = json!("MEDIA_TOOL_VERSION_UNSUPPORTED");
                check["message"] = json!("Cutcraft requires FFmpeg/ffprobe 6.1 or newer");
                check["hint"] = json!(BUILD_HINT);
            }
            None => {
                check["status"] = json!("warning");
                check["message"] = json!("Cannot compare this build version with 6.1; inspect capability and smoke-test results");
            }
            _ => {
                check["message"] = json!("Version satisfies the 6.1+ requirement");
            }
        }
        true
    }
    fn capabilities(&mut self, kind: &str, required: &[&str], flag_width: usize) {
        let id = format!("ffmpeg.{kind}");
        let Some(output) = self.command(
            &id,
            Command::new("ffmpeg").args(["-hide_banner", &format!("-{kind}")]),
            BUILD_HINT,
        ) else {
            return;
        };
        let output = String::from_utf8_lossy(&output.stdout);
        let found: BTreeSet<_> = output
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let flags = words.next()?;
                let name = words.next()?;
                (flags.len() == flag_width
                    && flags.bytes().all(|c| c.is_ascii_alphabetic() || c == b'.'))
                .then_some(name)
            })
            .collect();
        let missing: Vec<_> = required
            .iter()
            .filter(|name| !found.contains(**name))
            .collect();
        let check = self.checks.last_mut().unwrap();
        check["required"] = json!(required);
        check["missing"] = json!(missing);
        if missing.is_empty() {
            check["message"] = json!("Required components are available");
        } else {
            check["status"] = json!("failed");
            check["code"] = json!("MEDIA_CAPABILITY_MISSING");
            check["message"] = json!("FFmpeg build is missing required components");
            check["hint"] = json!(BUILD_HINT);
        }
    }
    fn verify(&mut self, id: &str, valid: bool, message: &str) {
        if !valid {
            let check = self
                .checks
                .iter_mut()
                .rev()
                .find(|c| c["id"] == id)
                .unwrap();
            check["status"] = json!("failed");
            check["code"] = json!("MEDIA_OUTPUT_INVALID");
            check["message"] = json!(message);
            check["hint"] = json!(BUILD_HINT);
        }
    }
}
fn ffmpeg(scratch: &Path) -> Command {
    let mut cmd = Command::new("ffmpeg");
    cmd.current_dir(scratch)
        .args(["-nostdin", "-v", "error", "-y", "-filter_threads", "2"]);
    cmd
}
fn encode(scratch: &Path, lossless: bool) -> Command {
    let mut cmd = ffmpeg(scratch);
    cmd.args([
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgba64le",
        "-video_size",
        "64x48",
        "-framerate",
        "10",
        "-i",
        "video.rgba",
        "-f",
        "f32le",
        "-ar",
        "24000",
        "-ac",
        "1",
        "-i",
        "audio.f32",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-frames:v",
        "2",
        "-t",
        "0.2",
        "-threads",
        "2",
    ]);
    if lossless {
        cmd.args([
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "gbrap16le",
            "-c:a",
            "pcm_f32le",
            "lossless.nut",
        ]);
    } else {
        cmd.args(["-vf","scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709","-c:v","libx264","-preset","ultrafast","-c:a","aac","-movflags","+faststart","export.mp4"]);
    }
    cmd
}

pub fn run(timeout: u64) -> Result<Value> {
    ensure!(
        (1..=120).contains(&timeout),
        "DOCTOR_INVALID_TIMEOUT: expected 1..120 seconds"
    );
    let mut report = Report {
        checks: Vec::new(),
        timeout,
    };
    let encoder = report.tool("ffmpeg");
    let probe = report.tool("ffprobe");
    if encoder {
        report.capabilities("encoders", ENCODERS, 6);
        report.capabilities("filters", FILTERS, 3);
        let graph_option = match timed(timeout, media_process::graph_file_option) {
            Ok(option) => {
                report.record("ffmpeg.graphFile", "passed", option);
                Some(option)
            }
            Err(error) => {
                report.error("ffmpeg.graphFile", &error, BUILD_HINT);
                None
            }
        };
        let scratch = tempfile::tempdir().context("create doctor temporary directory")?;
        fs::write(scratch.path().join("label.txt"), "Cutcraft doctor")?;
        fs::write(scratch.path().join("graph.txt"), "color=c=black@0:s=64x48:r=10,format=rgba,drawtext=textfile=label.txt:expansion=none:fontsize=10:fontcolor=white,format=rgba64le[v]")?;
        let mut raw = Vec::with_capacity(64 * 48 * 8 * 2);
        for _ in 0..64 * 48 * 2 {
            raw.extend_from_slice(&[0, 0, 0, 0, 255, 255, 255, 255]);
        }
        fs::write(scratch.path().join("video.rgba"), raw)?;
        fs::write(scratch.path().join("audio.f32"), vec![0u8; 4800 * 4])?;
        if let Some(option) = graph_option {
            if let Some(out) = report.command(
                "smoke.textPng",
                ffmpeg(scratch.path()).args([
                    option,
                    "graph.txt",
                    "-map",
                    "[v]",
                    "-frames:v",
                    "1",
                    "-threads",
                    "2",
                    "-c:v",
                    "png",
                    "-pix_fmt",
                    "rgba64be",
                    "-f",
                    "image2pipe",
                    "pipe:1",
                ]),
                "Check drawtext, PNG support and a usable default font. See docs/doctor.md.",
            ) {
                report.verify(
                    "smoke.textPng",
                    out.stdout.starts_with(b"\x89PNG\r\n\x1a\n"),
                    "PNG encoder did not produce a PNG image",
                );
            }
        } else {
            report.record("smoke.textPng", "skipped", "Filter graph check failed");
        }
        if report
            .command(
                "smoke.lossless",
                &mut encode(scratch.path(), true),
                BUILD_HINT,
            )
            .is_some()
        {
            let valid =
                fs::metadata(scratch.path().join("lossless.nut")).is_ok_and(|m| m.len() > 0);
            report.verify(
                "smoke.lossless",
                valid,
                "Lossless encoder did not produce an intermediate file",
            );
        }
        let mp4 = report
            .command("smoke.mp4", &mut encode(scratch.path(), false), BUILD_HINT)
            .is_some();
        if mp4 {
            if probe {
                let mut command = Command::new("ffprobe");
                command.current_dir(scratch.path()).args([
                    "-v",
                    "error",
                    "-show_streams",
                    "-of",
                    "json",
                    "export.mp4",
                ]);
                if let Some(out) = report.command("smoke.probe", &mut command, BUILD_HINT) {
                    let valid = serde_json::from_slice::<Value>(&out.stdout)
                        .ok()
                        .is_some_and(|v| {
                            let streams = v["streams"].as_array();
                            streams.is_some_and(|s| {
                                s.iter().any(|s| {
                                    s["codec_type"] == "video"
                                        && s["codec_name"] == "h264"
                                        && s["width"] == 64
                                        && s["height"] == 48
                                }) && s
                                    .iter()
                                    .any(|s| s["codec_type"] == "audio" && s["codec_name"] == "aac")
                            })
                        });
                    report.verify(
                        "smoke.probe",
                        valid,
                        "ffprobe did not report the expected H.264/AAC streams and dimensions",
                    );
                }
            } else {
                report.record(
                    "smoke.probe",
                    "skipped",
                    "ffprobe could not start or report its version",
                );
            }
            if let Some(out) = report.command(
                "smoke.decodeVideo",
                ffmpeg(scratch.path()).args([
                    "-i",
                    "export.mp4",
                    "-map",
                    "0:v:0",
                    "-frames:v",
                    "1",
                    "-threads",
                    "2",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "pipe:1",
                ]),
                BUILD_HINT,
            ) {
                report.verify(
                    "smoke.decodeVideo",
                    out.stdout.len() == 64 * 48 * 3,
                    "Video decoder did not return one 64x48 RGB frame",
                );
            }
            if let Some(out) = report.command(
                "smoke.decodeAudio",
                ffmpeg(scratch.path()).args([
                    "-i",
                    "export.mp4",
                    "-map",
                    "0:a:0",
                    "-t",
                    "0.2",
                    "-ar",
                    "24000",
                    "-ac",
                    "1",
                    "-f",
                    "f32le",
                    "pipe:1",
                ]),
                BUILD_HINT,
            ) {
                report.verify(
                    "smoke.decodeAudio",
                    out.stdout.len() >= 4800 * 4 && out.stdout.len() % 4 == 0,
                    "Audio decoder did not return the expected PCM samples",
                );
            }
        } else {
            for id in ["smoke.probe", "smoke.decodeVideo", "smoke.decodeAudio"] {
                report.record(id, "skipped", "MP4 encoding failed");
            }
        }
    } else {
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
            report.record(
                id,
                "skipped",
                "ffmpeg could not start or report its version",
            );
        }
    }
    let mut summary = json!({"passed":0,"failed":0,"warning":0,"skipped":0});
    for check in &report.checks {
        let status = check["status"].as_str().unwrap();
        summary[status] = json!(summary[status].as_u64().unwrap() + 1);
    }
    Ok(
        json!({"ok":summary["failed"]==0,"command":"doctor","cliVersion":env!("CARGO_PKG_VERSION"),"platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},"timeoutSeconds":timeout,"checks":report.checks,"summary":summary}),
    )
}
