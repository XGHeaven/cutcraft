//! End-to-end demo using the shipped CLI for every project edit.
//! cargo build --release && cargo run --release --example visual_demo -- local-examples/visual-demo.cut
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
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
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(serde_json::from_slice(&output.stdout)?)
    };
    run(
        &[
            "init",
            dir.to_str().unwrap(),
            "--name",
            "Visual pipeline comparison",
        ],
        None,
    )?;
    let generated = tempfile::tempdir()?;
    let source = generated.path().join("source.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30:duration=2",
            "-c:v",
            "libx264",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
        ])
        .arg(&source)
        .status()?;
    ensure!(status.success(), "FFmpeg failed");
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
    let aid = imported["assetId"].as_str().unwrap();
    let cube = generated.path().join("warm.cube");
    let mut file = std::fs::File::create(&cube)?;
    writeln!(file, "TITLE \"Warm demo\"\nLUT_3D_SIZE 17")?;
    for b in 0..17 {
        for g in 0..17 {
            for r in 0..17 {
                let (r, g, b) = (r as f64 / 16., g as f64 / 16., b as f64 / 16.);
                writeln!(
                    file,
                    "{} {} {}",
                    (r * 0.9 + g * 0.2 + 0.05).min(1.),
                    (g * 0.85 + r * 0.1).min(1.),
                    b * 0.65
                )?;
            }
        }
    }
    drop(file);
    let imported_lut = run(&["import", cube.to_str().unwrap(), "--managed"], None)?;
    let lut = imported_lut["assetId"].as_str().unwrap();
    let p = run(&["inspect"], None)?;
    let sid = p["rootSequenceId"].as_str().unwrap();
    let mut seq = p["sequences"][sid].clone();
    seq["format"]["width"] = json!(960);
    seq["format"]["height"] = json!(180);
    run(
        &["command", "-"],
        Some(json!({"type":"sequence.put","sequenceId":sid,"sequence":seq})),
    )?;
    for (index, title) in ["Original", "Exposure + curves", "Warm 3D LUT"]
        .iter()
        .enumerate()
    {
        let track = run(&["track", "add", "--name", title], None)?;
        let tracks = track["project"]["sequences"][sid]["tracks"]
            .as_array()
            .unwrap();
        let tid = tracks.last().unwrap()["id"].as_str().unwrap();
        let added = run(
            &[
                "item",
                "add",
                "--track",
                tid,
                "--asset",
                aid,
                "--stream",
                "stream_v0",
                "--duration",
                "2",
            ],
            None,
        )?;
        let iid = added["project"]["sequences"][sid]["tracks"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["items"][0]["id"]
            .as_str()
            .unwrap();
        run(
            &[
                "item",
                "transform",
                iid,
                "--x",
                &((index as f64 + 0.5) / 3.).to_string(),
            ],
            None,
        )?;
        if index == 1 {
            run(
                &[
                    "item",
                    "effect",
                    iid,
                    "--kind",
                    "color-grade",
                    "--parameters",
                    r#"{"exposureEv":0.8,"saturation":0.7}"#,
                ],
                None,
            )?;
            run(
                &[
                    "item",
                    "effect",
                    iid,
                    "--kind",
                    "curves",
                    "--parameters",
                    r#"{"rgb":[{"x":0,"y":0.03},{"x":0.5,"y":0.6},{"x":1,"y":1}]}"#,
                ],
                None,
            )?;
        } else if index == 2 {
            run(&["item","effect",iid,"--kind","lut","--parameters",&json!({"assetId":lut,"strength":0.8,"inputColorSpace":"rec709","outputColorSpace":"rec709","interpolation":"tetrahedral"}).to_string()],None)?;
        }
    }
    println!(
        "{}",
        run(&["render", "--output", "output/comparison.mp4"], None)?
    );
    Ok(())
}
