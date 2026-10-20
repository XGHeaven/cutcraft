use cutcraft::{id, store::Store};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cli(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .arg("-p")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}
fn fixture(dir: &Path) -> String {
    let store = Store::init(dir, "progress").unwrap();
    let mut p = store.project().unwrap();
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    p["sequences"][&sid]["format"]["width"] = json!(160);
    p["sequences"][&sid]["format"]["height"] = json!(90);
    p["sequences"][&sid]["format"]["frameRate"] = json!({"num":2,"den":1});
    let tid = id("track");
    p["sequences"][&sid]["tracks"] =
        json!([{"id":tid,"kind":"video","name":"title","overlapPolicy":"forbid","items":[]}]);
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":p["sequences"][&sid]}),
            false,
            false,
        )
        .unwrap();
    tid
}
fn require_ffmpeg() -> bool {
    let yes = Command::new("ffmpeg").arg("-version").output().is_ok();
    assert!(yes || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    yes
}

#[test]
fn editable_multiline_title_and_structured_progress_keep_stdout_clean() {
    if !require_ffmpeg() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let tid = fixture(temp.path());
    let text = temp.path().join("title.txt");
    fs::write(&text, "Title\nsecond line").unwrap();
    let out = cli(
        temp.path(),
        &[
            "item",
            "text",
            "--track",
            &tid,
            "--duration",
            "1",
            "--text-file",
            text.to_str().unwrap(),
            "--font-size",
            "20",
            "--color",
            "#ffffff80",
            "--position",
            "center",
            "--align",
            "left",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let store = Store::open(temp.path()).unwrap();
    let p = store.project().unwrap();
    drop(store);
    let sid = p["rootSequenceId"].as_str().unwrap();
    assert_eq!(
        p["sequences"][sid]["tracks"][0]["items"][0]["parameters"]["text"],
        "Title\nsecond line"
    );
    let out = cli(temp.path(), &["preview", "--progress", "json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(Path::new(result["video"].as_str().unwrap()).exists());
    let events: Vec<Value> = String::from_utf8(out.stderr)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for phase in [
        "validate", "video", "audio", "encode", "finalize", "complete",
    ] {
        assert!(
            events.iter().any(|v| v["phase"] == phase),
            "missing {phase}: {events:?}"
        );
    }
    assert_eq!(events.last().unwrap()["status"], "completed");
    assert!(events
        .iter()
        .any(|v| v["phase"] == "video" && v["completed"] == 2 && v["total"] == 2));
    // Include actual source decoding, not just generated text/final encoding.
    let source = temp.path().join("source.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=32x24:r=2:d=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(generated.status.success());
    let imported = cli(
        temp.path(),
        &[
            "import",
            source.to_str().unwrap(),
            "--append",
            "--color-space",
            "rec709",
        ],
    );
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let out = cli(temp.path(), &["render", "--progress", "json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events: Vec<Value> = String::from_utf8(out.stderr)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for phase in ["probe", "decode_video", "decode_audio"] {
        assert!(
            events.iter().any(|v| v["phase"] == phase),
            "missing {phase}"
        );
    }
    assert_eq!(events.last().unwrap()["status"], "completed");
    for mode in ["none", "auto", "human"] {
        let out = cli(temp.path(), &["preview", "--progress", mode]);
        assert!(out.status.success());
        serde_json::from_slice::<Value>(&out.stdout).unwrap();
        if mode == "human" {
            assert!(String::from_utf8(out.stderr).unwrap().contains("[video]"));
        } else {
            assert!(out.stderr.is_empty());
        }
    }
}

#[test]
fn failure_progress_never_claims_completion_and_preserves_dependency_error() {
    let temp = tempfile::tempdir().unwrap();
    let tid = fixture(temp.path());
    assert!(cli(
        temp.path(),
        &[
            "item",
            "text",
            "--track",
            &tid,
            "--duration",
            "1",
            "--text",
            "X"
        ]
    )
    .status
    .success());
    let empty = temp.path().join("empty-path");
    fs::create_dir(&empty).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .env("PATH", empty)
        .arg("-p")
        .arg(temp.path())
        .args(["preview", "--progress", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let events: Vec<Value> = String::from_utf8(out.stderr)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!events
        .iter()
        .any(|v| v["status"] == "completed" || v["phase"] == "complete"));
    assert_eq!(
        events.last().unwrap()["error"]["code"],
        "MEDIA_TOOL_NOT_FOUND"
    );
}
