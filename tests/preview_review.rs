use cutcraft::{
    id,
    preview::ExportOptions,
    preview_review::{self, Options},
    progress::{CancellationToken, Progress},
    project,
    time::{range, Time},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

fn fixture() -> (Value, String) {
    let mut p = project::new("Review preview");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(64);
    seq["format"]["height"] = json!(48);
    seq["format"]["frameRate"] = Time::integer(30).json().unwrap();
    let items: Vec<_> = [[1.,0.,0.], [0.,0.,1.]].into_iter().enumerate().map(|(i,c)| json!({"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":c[0],"g":c[1],"b":c[2],"a":1}},"timelineRange":range(&Time::integer(i as i64),&Time::integer(1)).unwrap()})).collect();
    seq["tracks"] = json!([{"id":id("track"),"kind":"video","name":"V","overlapPolicy":"forbid","items":items}]);
    (p, sid)
}
fn available() -> bool {
    let ok = Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        ok || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none(),
        "FFmpeg required"
    );
    ok
}
fn decode(v: &Value) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            v["output"].as_str().unwrap(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn pixel(pixels: &[u8], result: &Value, cell: usize) -> [u8; 4] {
    let c = &result["cells"][cell];
    let x = c["x"].as_u64().unwrap() + c["width"].as_u64().unwrap() / 2;
    let y = c["y"].as_u64().unwrap() + c["height"].as_u64().unwrap() / 2;
    let i = ((y * result["width"].as_u64().unwrap() + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}
fn cli(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .env("CUTCRAFT_RUNTIME_DIR", dir.join("runtime"))
        .arg("-p")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}
fn run(p: &Value, sid: &str, dir: &Path, selection: &ExportOptions, options: &Options) -> Value {
    preview_review::export(p, dir, sid, selection, options, &Progress::default()).unwrap()
}
#[test]
fn sheets_and_boundary_pairs_render_actual_pixels_without_editing() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (p, _sid) = fixture();
    let original = project::canonical(&p).unwrap();
    fs::write(tmp.path().join("cutcraft.json"), &original).unwrap();
    let out = cli(
        tmp.path(),
        &[
            "preview",
            "--sheet",
            "--count",
            "4",
            "--columns",
            "2",
            "--progress",
            "json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["mode"], "sheet");
    assert_eq!(
        v["cells"][3]["time"],
        Time::new(3, 2).unwrap().json().unwrap()
    );
    assert_eq!(v["revision"], p["revision"]);
    let pixels = decode(&v);
    assert_eq!(pixel(&pixels, &v, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, &v, 1), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, &v, 2), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, &v, 3), [0, 0, 255, 255]);
    // The label band contains visible text, beyond the dark sheet background.
    let width = v["width"].as_u64().unwrap() as usize;
    assert!(pixels[48 * width * 4..92 * width * 4]
        .chunks_exact(4)
        .any(|p| p[0] > 200 && p[1] > 200 && p[2] > 200));
    let events: Vec<Value> = String::from_utf8(out.stderr)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        events.iter().filter(|e| e["status"] == "completed").count(),
        1
    );
    assert_eq!(events.last().unwrap()["status"], "completed");
    let out = cli(tmp.path(), &["preview", "--boundaries"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["boundaries"].as_array().unwrap().len(), 3);
    assert_eq!(v["boundaries"][1]["items"].as_array().unwrap().len(), 2);
    assert_eq!(v["cells"][0]["time"], Value::Null);
    assert_eq!(v["cells"][5]["time"], Value::Null);
    assert_eq!(
        v["cells"][2]["time"],
        Time::new(29, 30).unwrap().json().unwrap()
    );
    assert_eq!(v["cells"][3]["time"], Time::integer(1).json().unwrap());
    let pixels = decode(&v);
    assert_eq!(pixel(&pixels, &v, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, &v, 3), [0, 0, 255, 255]);
    assert_eq!(
        fs::read(tmp.path().join("cutcraft.json")).unwrap(),
        original
    );
    assert!(cutcraft::store::Store::open(tmp.path())
        .unwrap()
        .history()
        .unwrap()
        .is_empty());
}
#[test]
fn exact_off_grid_boundaries_ranges_versions_and_visibility() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    let options = Options {
        boundaries: true,
        ..Default::default()
    };
    p["sequences"][&sid]["format"]["frameRate"] = Time::new(30000, 1001).unwrap().json().unwrap();
    for version in ["0.1.0", "0.4.0", "0.5.0"] {
        p["specVersion"] = json!(version);
        p["$schema"] = if version == "0.1.0" {
            fixture().0["$schema"].clone()
        } else {
            json!(format!(
                "https://cutstate.dev/schema/v{}/project.schema.json",
                &version[..3]
            ))
        };
        let v = run(
            &p,
            &sid,
            tmp.path(),
            &ExportOptions {
                at: Some(Time::integer(1)),
                ..Default::default()
            },
            &options,
        );
        assert_eq!(
            v["cells"][0]["time"],
            Time::new(29 * 1001, 30000).unwrap().json().unwrap()
        );
        assert_eq!(
            v["cells"][1]["time"],
            Time::new(1001, 1000).unwrap().json().unwrap()
        );
        assert_eq!(v["cells"][0]["frameIndex"], 29);
        assert_eq!(v["cells"][1]["frameIndex"], 30);
    }
    let v = run(
        &p,
        &sid,
        tmp.path(),
        &ExportOptions {
            start: Some(Time::new(1, 2).unwrap()),
            duration: Some(Time::integer(1)),
            ..Default::default()
        },
        &options,
    );
    assert_eq!(v["boundaries"].as_array().unwrap().len(), 1);
    p["specVersion"] = json!("0.3.0");
    p["$schema"] = json!("https://cutstate.dev/schema/v0.3/project.schema.json");
    assert!(preview_review::export(
        &p,
        tmp.path(),
        &sid,
        &ExportOptions::default(),
        &options,
        &Progress::default()
    )
    .unwrap_err()
    .to_string()
    .contains("UNSUPPORTED_VERSION"));
    let (mut p, sid) = fixture();
    let mut hidden = p["sequences"][&sid]["tracks"][0].clone();
    hidden["id"] = json!(id("track"));
    hidden["enabled"] = json!(false);
    hidden["items"][0]["id"] = json!(id("item"));
    hidden["items"][1]["id"] = json!(id("item"));
    hidden["items"][0]["timelineRange"] = range(&Time::zero(), &Time::new(1, 2).unwrap()).unwrap();
    p["sequences"][&sid]["tracks"]
        .as_array_mut()
        .unwrap()
        .push(hidden);
    let v = run(&p, &sid, tmp.path(), &ExportOptions::default(), &options);
    assert_eq!(v["boundaries"].as_array().unwrap().len(), 3);
    p["sequences"][&sid]["tracks"][0]["enabled"] = json!(false);
    p["sequences"][&sid]["durationMode"] = json!("fixed");
    p["sequences"][&sid]["duration"] = Time::integer(2).json().unwrap();
    assert!(preview_review::export(
        &p,
        tmp.path(),
        &sid,
        &ExportOptions::default(),
        &options,
        &Progress::default()
    )
    .unwrap_err()
    .to_string()
    .contains("NO_BOUNDARIES"));
}
#[test]
fn rejects_invalid_options_and_cancellation_does_not_publish() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (p, sid) = fixture();
    fs::write(
        tmp.path().join("cutcraft.json"),
        project::canonical(&p).unwrap(),
    )
    .unwrap();
    for args in [
        vec!["preview", "--sheet", "--boundaries"],
        vec!["preview", "--sheet", "--count", "0"],
        vec!["preview", "--sheet", "--count", "129"],
        vec!["preview", "--columns", "2"],
        vec!["preview", "--sheet", "--columns", "17"],
        vec!["preview", "--sheet", "--at", "1"],
        vec!["preview", "--boundaries", "--at", "3"],
        vec!["preview", "--boundaries", "--at", "1", "--start", "0"],
        vec!["preview", "--sheet", "--duration", "0"],
    ] {
        assert!(!cli(tmp.path(), &args).status.success(), "{args:?}");
    }
    let options = Options {
        sheet: true,
        count: Some(2),
        ..Default::default()
    };
    let v = run(&p, &sid, tmp.path(), &ExportOptions::default(), &options);
    let path = Path::new(v["output"].as_str().unwrap());
    let original = fs::read(path).unwrap();
    let token = CancellationToken::default();
    let cancel = token.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let reporter = Progress::with_callback(
        token,
        Arc::new(move |event| {
            if event["phase"] == "finalize" {
                cancel.cancel();
            }
            observed.lock().unwrap().push(event);
        }),
    );
    assert!(preview_review::export(
        &p,
        tmp.path(),
        &sid,
        &ExportOptions::default(),
        &options,
        &reporter
    )
    .unwrap_err()
    .to_string()
    .contains("JOB_CANCELLED"));
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["status"] == "completed"));
}

#[test]
fn host_transport_and_limits_are_enforced_without_clap() {
    use clap::Parser;
    use cutcraft::app::{self, Cli};
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, sid) = fixture();
    fs::write(
        tmp.path().join("cutcraft.json"),
        project::canonical(&p).unwrap(),
    )
    .unwrap();
    let prepared = app::prepare(
        Cli::try_parse_from([
            "cutcraft",
            "-p",
            tmp.path().to_str().unwrap(),
            "preview",
            "--boundaries",
            "--at",
            "1",
        ])
        .unwrap(),
    )
    .unwrap();
    let mut transport = serde_json::to_value(prepared).unwrap();
    let result = app::ApplicationHost
        .execute(serde_json::from_value(transport.clone()).unwrap())
        .unwrap();
    assert_eq!(result["mode"], "boundaries");
    assert_eq!(result["cells"][0]["frameIndex"], 29);
    transport["command"]["Preview"]["count"] = json!(2);
    assert!(app::ApplicationHost
        .execute(serde_json::from_value(transport).unwrap())
        .unwrap_err()
        .to_string()
        .contains("require --sheet"));
    let options = Options {
        boundaries: true,
        ..Default::default()
    };
    let base = p["sequences"][&sid]["tracks"][0]["items"][0].clone();
    let items: Vec<_> = (0..65)
        .map(|i| {
            let mut item = base.clone();
            item["id"] = json!(id("item"));
            item["timelineRange"] = range(&Time::integer(i), &Time::integer(1)).unwrap();
            item
        })
        .collect();
    p["sequences"][&sid]["tracks"][0]["items"] = json!(items);
    assert!(preview_review::export(
        &p,
        tmp.path(),
        &sid,
        &ExportOptions::default(),
        &options,
        &Progress::default()
    )
    .unwrap_err()
    .to_string()
    .contains("TOO_MANY_BOUNDARIES"));
    let limited = run(
        &p,
        &sid,
        tmp.path(),
        &ExportOptions {
            start: Some(Time::integer(30)),
            duration: Some(Time::integer(1)),
            ..Default::default()
        },
        &options,
    );
    assert_eq!(limited["boundaries"].as_array().unwrap().len(), 2);
    let (mut p, sid) = fixture();
    p["sequences"][&sid]["format"]["width"] = json!(1920);
    p["sequences"][&sid]["format"]["height"] = json!(1080);
    assert!(preview_review::export(
        &p,
        tmp.path(),
        &sid,
        &ExportOptions {
            scale: Some(1.),
            ..Default::default()
        },
        &Options {
            sheet: true,
            count: Some(128),
            ..Default::default()
        },
        &Progress::default()
    )
    .unwrap_err()
    .to_string()
    .contains("SHEET_TOO_LARGE"));
    let p = project::new("empty");
    assert!(preview_review::export(
        &p,
        tmp.path(),
        p["rootSequenceId"].as_str().unwrap(),
        &ExportOptions::default(),
        &Options {
            sheet: true,
            ..Default::default()
        },
        &Progress::default()
    )
    .unwrap_err()
    .to_string()
    .contains("EMPTY_SEQUENCE"));
}

#[test]
fn interval_sampling_is_exact_exclusive_and_anchored_to_range_start() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (p, sid) = fixture();
    let original = project::canonical(&p).unwrap();
    fs::write(tmp.path().join("cutcraft.json"), &original).unwrap();
    let output = cli(
        tmp.path(),
        &[
            "preview",
            "--sheet",
            "--interval",
            "1/2",
            "--start",
            "1/4",
            "--duration",
            "3/2",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let times: Vec<_> = result["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cell| cell["time"].clone())
        .collect();
    assert_eq!(
        times,
        vec![
            Time::new(1, 4).unwrap().json().unwrap(),
            Time::new(3, 4).unwrap().json().unwrap(),
            Time::new(5, 4).unwrap().json().unwrap()
        ]
    );
    let pixels = decode(&result);
    assert_eq!(pixel(&pixels, &result, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, &result, 1), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, &result, 2), [0, 0, 255, 255]);
    // Non-divisible intervals keep the last sample before the end; long intervals keep only start.
    for (interval, expected) in [
        (
            Time::new(3, 5).unwrap(),
            vec![
                Time::zero(),
                Time::new(3, 5).unwrap(),
                Time::new(6, 5).unwrap(),
                Time::new(9, 5).unwrap(),
            ],
        ),
        (Time::integer(2), vec![Time::zero()]),
        (Time::integer(3), vec![Time::zero()]),
    ] {
        let result = run(
            &p,
            &sid,
            tmp.path(),
            &ExportOptions::default(),
            &Options {
                sheet: true,
                interval: Some(interval),
                ..Default::default()
            },
        );
        let times: Vec<_> = result["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| Time::from_json(&cell["time"]).unwrap())
            .collect();
        assert_eq!(times, expected);
    }
    assert_eq!(
        fs::read(tmp.path().join("cutcraft.json")).unwrap(),
        original
    );
    for args in [
        vec!["preview", "--sheet", "--interval", "0"],
        vec!["preview", "--sheet", "--interval", "-1"],
        vec!["preview", "--interval", "1"],
        vec!["preview", "--boundaries", "--interval", "1"],
        vec!["preview", "--sheet", "--interval", "1", "--count", "2"],
    ] {
        assert!(!cli(tmp.path(), &args).status.success(), "{args:?}");
    }
    let output = cli(
        tmp.path(),
        &["preview", "--sheet", "--interval", "1/9223372036854775807"],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("PREVIEW_TOO_MANY_THUMBNAILS"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn interval_host_transport_and_validation_preserve_exact_time() {
    use clap::Parser;
    use cutcraft::app::{self, Cli};
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (p, _sid) = fixture();
    fs::write(
        tmp.path().join("cutcraft.json"),
        project::canonical(&p).unwrap(),
    )
    .unwrap();
    let prepared = app::prepare(
        Cli::try_parse_from([
            "cutcraft",
            "-p",
            tmp.path().to_str().unwrap(),
            "preview",
            "--sheet",
            "--interval",
            "0.5",
        ])
        .unwrap(),
    )
    .unwrap();
    let transport = serde_json::to_value(prepared).unwrap();
    assert_eq!(
        transport["command"]["Preview"]["interval"],
        Time::new(1, 2).unwrap().json().unwrap()
    );
    let result = app::ApplicationHost
        .execute(serde_json::from_value(transport.clone()).unwrap())
        .unwrap();
    assert_eq!(result["cells"].as_array().unwrap().len(), 4);
    assert_eq!(
        result["cells"][3]["time"],
        Time::new(3, 2).unwrap().json().unwrap()
    );
    for (field, value, error) in [
        ("interval", Time::zero().json().unwrap(), "INVALID_INTERVAL"),
        (
            "interval",
            Time::integer(-1).json().unwrap(),
            "INVALID_INTERVAL",
        ),
        ("count", json!(2), "OPTIONS_CONFLICT"),
        ("sheet", json!(false), "require --sheet"),
    ] {
        let mut invalid = transport.clone();
        invalid["command"]["Preview"][field] = value;
        assert!(app::ApplicationHost
            .execute(serde_json::from_value(invalid).unwrap())
            .unwrap_err()
            .to_string()
            .contains(error));
    }
}
