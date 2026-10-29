use cutcraft::{
    id,
    progress::{CancellationToken, Progress},
    project, render,
    time::Time,
};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
fn tree_max(path: &Path) -> u64 {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                tree_max(&e.path())
            } else {
                m.len()
            }
        })
        .max()
        .unwrap_or(0)
}
#[test]
fn streaming_export_avoids_raw_output_spool_and_cancellation_preserves_target() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut p = project::new("stream");
    let sid = p["rootSequenceId"].as_str().unwrap().to_owned();
    let seq = &mut p["sequences"][&sid];
    seq["format"]["width"] = json!(64);
    seq["format"]["height"] = json!(48);
    seq["format"]["frameRate"] = Time::integer(12).json().unwrap();
    seq["tracks"] = json!([{"id":id("track"),"kind":"video","name":"Color","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":3,"den":1}}}]}]);
    let output = dir.join("final.mp4");
    let max = Arc::new(AtomicU64::new(0));
    let observed = max.clone();
    let root = dir.to_owned();
    let progress = Progress::with_callback(
        CancellationToken::default(),
        Arc::new(move |_| {
            observed.fetch_max(tree_max(&root), Ordering::Relaxed);
        }),
    );
    let result = render::render_with_progress(&p, dir, &sid, &output, false, &progress).unwrap();
    assert_eq!(result["frames"], 36);
    // With generated media, only the small encoded result needs disk. A composed
    // raw video spool alone would be 884736 bytes, plus 1152000 bytes of PCM.
    assert!(
        max.load(Ordering::Relaxed) < 64 * 1024,
        "unexpected full-output spool"
    );
    let previous = fs::read(&output).unwrap();
    let cancel = CancellationToken::default();
    let control = cancel.clone();
    let completed = Arc::new(AtomicU64::new(0));
    let reported = completed.clone();
    let progress = Progress::with_callback(
        cancel,
        Arc::new(move |event| {
            if event["phase"] == "video" && event["completed"].as_u64().unwrap_or(0) > 0 {
                control.cancel();
            }
            if event["status"] == "completed" {
                reported.fetch_add(1, Ordering::Relaxed);
            }
        }),
    );
    let error = render::render_with_progress(&p, dir, &sid, &output, true, &progress).unwrap_err();
    assert!(error.to_string().contains("JOB_CANCELLED"), "{error:#}");
    assert_eq!(fs::read(&output).unwrap(), previous);
    assert_eq!(completed.load(Ordering::Relaxed), 0);
    assert_eq!(
        fs::read_dir(dir).unwrap().count(),
        1,
        "staging must be cleaned on success/cancel"
    );
}
