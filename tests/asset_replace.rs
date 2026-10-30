use clap::Parser;
use cutcraft::{
    app::{self, Cli},
    media,
    project::{self, text},
    store::{self, Store},
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn available() -> bool {
    let yes = ["ffmpeg", "ffprobe"]
        .iter()
        .all(|p| Command::new(p).arg("-version").output().is_ok());
    assert!(
        yes || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none(),
        "FFmpeg required"
    );
    yes
}
fn ffmpeg(path: &Path, args: &[&str]) {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(args)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn av(path: &Path, color: &str, duration: &str, reversed: bool) {
    ffmpeg(
        path,
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color={color}:s=64x64:r=10:d={duration}"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:sample_rate=48000:duration={duration}"),
            "-map",
            if reversed { "1:a" } else { "0:v" },
            "-map",
            if reversed { "0:v" } else { "1:a" },
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
        ],
    );
}
fn run(dir: &Path, args: &[&str]) -> anyhow::Result<Value> {
    let cli = Cli::try_parse_from(
        ["cutcraft", "-p", dir.to_str().unwrap()]
            .into_iter()
            .chain(args.iter().copied()),
    )?;
    let cli = app::prepare(cli)?;
    // The same request must survive ApplicationHost transport.
    let cli = serde_json::from_slice(&serde_json::to_vec(&cli)?)?;
    app::ApplicationHost.execute(cli)
}
fn files(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir.join("assets"))
        .unwrap()
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn replacement_preserves_edits_ids_metadata_and_undo_and_refreshes_preview() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old.mkv");
    let new = tmp.path().join("新素材 blue.mkv");
    av(&old, "red", "2", false);
    av(&new, "blue", "3", true);
    let dir = tmp.path().join("project");
    drop(Store::init(&dir, "replace").unwrap());
    let imported = run(
        &dir,
        &[
            "import",
            old.to_str().unwrap(),
            "--managed",
            "--append",
            "--color-space",
            "rec709",
        ],
    )
    .unwrap();
    let aid = text(&imported["import"]["assetId"]);
    let store = Store::open(&dir).unwrap();
    let p = store.project().unwrap();
    let sid = text(&p["rootSequenceId"]);
    let mut asset = p["assets"][aid].clone();
    asset["name"] = json!("Keep label");
    asset["tags"] = json!(["voice"]);
    asset["metadata"] = json!({"note":"keep"});
    asset["extensions"] = json!({"test.plugin":{"flag":true}});
    asset["streams"][0]["extensions"] = json!({"test.plugin":{"stream":true}});
    let mut seq = p["sequences"][sid].clone();
    seq["format"]["width"] = json!(64);
    seq["format"]["height"] = json!(64);
    store.command(json!({"type":"batch","commands":[{"type":"asset.put","assetId":aid,"asset":asset},{"type":"sequence.put","sequenceId":sid,"sequence":seq}]}), false, false).unwrap();
    let before = store.project().unwrap();
    let history = store.history().unwrap();
    drop(store);
    let previous_file = media::local_path(&dir, text(&before["assets"][aid]["uri"])).unwrap();
    let original_files = files(&dir);
    let before_png = run(&dir, &["preview", "--at", "0", "--scale", "1"]).unwrap();
    let dry = run(
        &dir,
        &[
            "--dry-run",
            "asset",
            "replace",
            aid,
            new.to_str().unwrap(),
            "--color-space",
            "rec709",
        ],
    )
    .unwrap();
    assert_eq!(dry["dryRun"], true);
    assert_eq!(files(&dir), original_files);
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.project().unwrap(), before);
    assert_eq!(store.history().unwrap(), history);
    drop(store);
    let result = run(
        &dir,
        &[
            "asset",
            "replace",
            aid,
            new.to_str().unwrap(),
            "--color-space",
            "rec709",
        ],
    )
    .unwrap();
    assert_eq!(result["usageCount"], 2);
    let after = &result["project"];
    let updated = &after["assets"][aid];
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(after["sequences"], before["sequences"]);
    for key in ["name", "tags", "metadata", "extensions"] {
        assert_eq!(updated[key], before["assets"][aid][key]);
    }
    for (index, stream) in updated["streams"].as_array().unwrap().iter().enumerate() {
        assert_eq!(stream["id"], before["assets"][aid]["streams"][index]["id"]);
        assert_ne!(
            stream["index"],
            before["assets"][aid]["streams"][index]["index"]
        );
    }
    assert_eq!(
        updated["streams"][0]["extensions"],
        before["assets"][aid]["streams"][0]["extensions"]
    );
    assert_ne!(updated["fingerprint"], before["assets"][aid]["fingerprint"]);
    assert!(previous_file.is_file());
    assert_eq!(files(&dir).len(), original_files.len() + 1);
    media::validate(after, &dir).unwrap();
    let png = run(&dir, &["preview", "--at", "0", "--scale", "1"]).unwrap();
    assert_ne!(png, before_png);
    // Verify replacement reaches actual decoded pixels, including a warmed preview cache.
    let path = png["path"]
        .as_str()
        .or_else(|| png["output"].as_str())
        .unwrap_or_else(|| panic!("{png}"));
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            path,
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let px = &output.stdout;
    assert!(px[2] > 220 && px[0] < 30, "{px:?}");
    run(&dir, &["undo"]).unwrap();
    let store = Store::open(&dir).unwrap();
    let undone = store.project().unwrap();
    assert_eq!(undone["assets"], before["assets"]);
    assert_eq!(undone["sequences"], before["sequences"]);
    assert_eq!(&store.history().unwrap()[..history.len()], &history);
    assert_eq!(files(&dir).len(), original_files.len() + 1);
}

#[test]
fn failures_are_atomic_and_locked_tracks_require_override() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old.mkv");
    let short = tmp.path().join("short.mkv");
    av(&old, "red", "2", false);
    av(&short, "blue", "1", false);
    let dir = tmp.path().join("project");
    drop(Store::init(&dir, "failures").unwrap());
    let imported = run(
        &dir,
        &["import", old.to_str().unwrap(), "--managed", "--append"],
    )
    .unwrap();
    let aid = text(&imported["import"]["assetId"]);
    let before = fs::read(dir.join("cutcraft.json")).unwrap();
    let history = fs::read(dir.join("history.jsonl")).unwrap();
    let assets = files(&dir);
    let err = run(&dir, &["asset", "replace", aid, short.to_str().unwrap()]).unwrap_err();
    assert!(
        format!("{err:#}").contains("SOURCE_OUT_OF_BOUNDS"),
        "{err:#}"
    );
    assert_eq!(fs::read(dir.join("cutcraft.json")).unwrap(), before);
    assert_eq!(fs::read(dir.join("history.jsonl")).unwrap(), history);
    assert_eq!(files(&dir), assets);
    let store = Store::open(&dir).unwrap();
    let p = store.project().unwrap();
    let sid = text(&p["rootSequenceId"]);
    let mut seq = p["sequences"][sid].clone();
    seq["tracks"][0]["locked"] = json!(true);
    store
        .command(
            json!({"type":"sequence.put","sequenceId":sid,"sequence":seq}),
            false,
            false,
        )
        .unwrap();
    drop(store);
    let err = run(&dir, &["asset", "replace", aid, old.to_str().unwrap()]).unwrap_err();
    assert!(format!("{err:#}").contains("TRACK_LOCKED"));
    assert_eq!(files(&dir), assets);
    run(
        &dir,
        &[
            "--override-locks",
            "asset",
            "replace",
            aid,
            old.to_str().unwrap(),
            "--external",
        ],
    )
    .unwrap();
    assert_eq!(
        Store::open(&dir).unwrap().project().unwrap()["assets"][aid]["storage"],
        "external"
    );
    assert_eq!(files(&dir), assets);
}

#[test]
fn ambiguous_streams_require_explicit_bijection_and_concurrent_asset_changes_fail() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let audio = tmp.path().join("two.mka");
    ffmpeg(
        &audio,
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=duration=1",
            "-map",
            "0:a",
            "-map",
            "0:a",
            "-c:a",
            "pcm_s16le",
        ],
    );
    let dir = tmp.path().join("project");
    let store = Store::init(&dir, "streams").unwrap();
    let imported = media::import(&store, &audio, false).unwrap();
    let aid = text(&imported["assetId"]);
    drop(store);
    let err = run(&dir, &["asset", "replace", aid, audio.to_str().unwrap()]).unwrap_err();
    assert!(format!("{err:#}").contains("STREAM_MAP_REQUIRED"));
    for mapping in [
        vec!["stream_a0=stream_a0"],
        vec!["stream_a0=stream_a0", "stream_a1=stream_a0"],
        vec!["stream_a0=stream_a9", "stream_a1=stream_a1"],
    ] {
        let mut args = vec!["asset", "replace", aid, audio.to_str().unwrap()];
        for entry in mapping {
            args.extend(["--stream-map", entry]);
        }
        assert!(run(&dir, &args).is_err());
    }
    let result = run(
        &dir,
        &[
            "asset",
            "replace",
            aid,
            audio.to_str().unwrap(),
            "--stream-map",
            "stream_a0=stream_a1",
            "--stream-map",
            "stream_a1=stream_a0",
        ],
    )
    .unwrap();
    assert_eq!(result["project"]["assets"][aid]["streams"][0]["index"], 1);
    let p = result["project"].clone();
    let mapping = vec!["stream_a0=stream_a0".into(), "stream_a1=stream_a1".into()];
    let options = media::ReplaceOptions {
        managed: Some(true),
        color_space: None,
        stream_map: &mapping,
        dry_run: false,
    };
    let prepared = media::prepare_replace(&p, &dir, aid, &audio, &options).unwrap();
    let store = Store::open(&dir).unwrap();
    let mut asset = p["assets"][aid].clone();
    asset["name"] = json!("concurrent edit");
    store
        .command(
            json!({"type":"asset.put","assetId":aid,"asset":asset}),
            false,
            false,
        )
        .unwrap();
    assert!(format!(
        "{:#}",
        media::commit_replace(&store, prepared, false).unwrap_err()
    )
    .contains("ASSET_CHANGED"));
    assert!(files(&dir).is_empty());
    // An unrelated edit is retained rather than overwritten after preparation.
    let p = store.project().unwrap();
    drop(store);
    let prepared = media::prepare_replace(&p, &dir, aid, &audio, &options).unwrap();
    let store = Store::open(&dir).unwrap();
    store
        .command(
            json!({"type":"project.update","changes":{"name":"concurrent project name"}}),
            false,
            false,
        )
        .unwrap();
    let result = media::commit_replace(&store, prepared, false).unwrap();
    assert_eq!(result["project"]["name"], "concurrent project name");
}

#[test]
fn replacement_retains_supported_protocol_versions_and_history() {
    if !available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let audio = tmp.path().join("audio.wav");
    ffmpeg(&audio, &["-f", "lavfi", "-i", "sine=duration=1"]);
    for path in [
        "spec/examples/minimal.cutcraft.json",
        "spec/archive/0.3.0/examples/minimal.project.json",
        "spec/archive/0.4.0/examples/animated.cutcraft.json",
        "spec/archive/0.5.0/examples/image-sequence.cutcraft.json",
    ] {
        let original = store::read_json(Path::new(path)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cutcraft.json"),
            project::canonical(&original).unwrap(),
        )
        .unwrap();
        let store = Store::open(dir.path()).unwrap();
        let imported = media::import(&store, &audio, false).unwrap();
        let aid = text(&imported["assetId"]);
        let before = store.project().unwrap();
        let history = store.history().unwrap();
        drop(store);
        let result = run(
            dir.path(),
            &["asset", "replace", aid, audio.to_str().unwrap()],
        )
        .unwrap();
        for key in ["id", "specVersion", "$schema", "sequences"] {
            assert_eq!(result["project"][key], before[key]);
        }
        assert_eq!(result["event"]["eventVersion"], original["specVersion"]);
        run(dir.path(), &["undo"]).unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.project().unwrap()["assets"], before["assets"]);
        assert_eq!(&store.history().unwrap()[..history.len()], &history);
    }
}
