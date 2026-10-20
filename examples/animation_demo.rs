//! cargo build --release && cargo run --release --example animation_demo -- local-examples/animation-demo.cut
//! All edits go through the CLI; generated test media leaves user footage untouched.
use anyhow::{ensure, Context, Result};
use cutcraft::id;
use serde_json::{json, Value};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
fn t(n: i64) -> Value {
    json!({"num":n,"den":1})
}
fn curve(kind: &str, keys: Value) -> Value {
    json!({"$animation":{"version":"1.0.0","valueType":kind,"timeReference":"owner","keyframes":keys}})
}
fn main() -> Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("provide a NEW project directory")?,
    );
    ensure!(!dir.exists(), "demo directory already exists");
    let exe = std::env::current_exe()?
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("cutcraft");
    let run = |args: &[&str], input: Option<Value>| -> Result<Value> {
        let mut child = Command::new(&exe)
            .arg("-p")
            .arg(&dir)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(serde_json::to_string(&input)?.as_bytes())?;
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output()?;
        ensure!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    let initial = run(
        &[
            "init",
            dir.to_str().unwrap(),
            "--name",
            "Animation, retime and transitions",
        ],
        None,
    )?;
    let p = &initial["project"];
    let sid = p["rootSequenceId"].as_str().unwrap();
    let mut seq = p["sequences"][sid].clone();
    seq["format"]["width"] = json!(320);
    seq["format"]["height"] = json!(180);
    seq["format"]["frameRate"] = t(24);
    seq["background"] = json!({"r":0.08,"g":0.08,"b":0.12,"a":1});
    let track = id("track");
    seq["tracks"] =
        json!([{"id":track,"name":"Demo","kind":"video","overlapPolicy":"forbid","items":[]}]);
    run(
        &["command", "-"],
        Some(json!({"type":"sequence.put","sequenceId":sid,"sequence":seq})),
    )?;
    let generated = tempfile::tempdir()?;
    let source = generated.path().join("source.mp4");
    let out = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=24:duration=3",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&source)
        .output()?;
    ensure!(out.status.success(), "FFmpeg demo source failed");
    let imported = run(
        &[
            "import",
            source.to_str().unwrap(),
            "--managed",
            "--color-space",
            "rec709",
        ],
        None,
    )?;
    let asset = imported["assetId"].as_str().unwrap();
    run(
        &[
            "item",
            "color",
            "--track",
            &track,
            "--duration",
            "2",
            "--color",
            "#f05a35",
        ],
        None,
    )?;
    run(
        &[
            "item",
            "add",
            "--track",
            &track,
            "--asset",
            asset,
            "--stream",
            "stream_v0",
            "--start",
            "2",
            "--duration",
            "4",
            "--source-duration",
            "2",
        ],
        None,
    )?;
    run(
        &[
            "item",
            "color",
            "--track",
            &track,
            "--start",
            "6",
            "--duration",
            "2",
            "--color",
            "#287cdc",
        ],
        None,
    )?;
    let snapshot = run(&["inspect"], None)?;
    let items = snapshot["sequences"][sid]["tracks"][0]["items"]
        .as_array()
        .unwrap();
    let a = items[0]["id"].as_str().unwrap();
    let b = items[1]["id"].as_str().unwrap();
    let c = items[2]["id"].as_str().unwrap();
    run(
        &[
            "item",
            "transform",
            a,
            "--scale-x",
            "0.35",
            "--scale-y",
            "0.35",
        ],
        None,
    )?;
    run(
        &[
            "item",
            "animate",
            a,
            "video.transform.position",
            "--file",
            "-",
        ],
        Some(curve(
            "vector2",
            json!([{"offset":t(0),"value":{"x":0.2,"y":0.5},"easing":{"type":"cubic-bezier","x1":0.42,"y1":0,"x2":0.58,"y2":1}},{"offset":t(2),"value":{"x":0.8,"y":0.5}}]),
        )),
    )?;
    run(
        &[
            "item",
            "animate",
            a,
            "video.transform.rotationDegrees",
            "--file",
            "-",
        ],
        Some(curve(
            "number",
            json!([{"offset":t(0),"value":-30},{"offset":t(2),"value":330}]),
        )),
    )?;
    let mapping = curve(
        "time",
        json!([{"offset":t(0),"value":t(0),"easing":{"type":"cubic-hermite","outRate":{"num":1,"den":2},"inRate":{"num":3,"den":2}}},{"offset":t(1),"value":t(1)},{"offset":t(2),"value":t(1)},{"offset":t(4),"value":t(0)}]),
    );
    std::fs::write(
        dir.join("time-map.json"),
        serde_json::to_vec_pretty(&mapping)?,
    )?;
    run(&["item", "time-map", b, "--file", "-"], Some(mapping))?;
    run(
        &[
            "transition",
            a,
            b,
            "--duration",
            "1/2",
            "--alignment",
            "after",
        ],
        None,
    )?;
    run(
        &[
            "transition",
            b,
            c,
            "--duration",
            "1/2",
            "--alignment",
            "before",
            "--kind",
            "dip-to-color",
            "--color",
            "#ffffff",
        ],
        None,
    )?;
    let result = run(&["render", "--output", "output/animation-demo.mp4"], None)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
