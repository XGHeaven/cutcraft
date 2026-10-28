use cutcraft::{
    cache::{Kind, Manager},
    store::Store,
};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .env("CUTCRAFT_RUNTIME_DIR", dir.join("runtime"))
        .arg("-p")
        .arg(dir)
        .arg("cache")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn cli_clean_respects_another_process_lease_and_preserves_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    drop(Store::init(dir, "cache").unwrap());
    let before = fs::read(dir.join("cutcraft.json")).unwrap();
    let history = fs::read(dir.join("history.jsonl")).unwrap();
    let m = Manager::new(dir);
    let path = m
        .directory(Kind::Frames)
        .unwrap()
        .join(format!("{}.png", "a".repeat(64)));
    let lease = m.entry(&path).unwrap();
    lease.publish(b"PNG").unwrap();
    let status = cli(dir, &["status"]);
    assert_eq!(status["busyBytes"], 3);
    assert_eq!(cli(dir, &["clean"])["removedEntries"], 0);
    assert!(path.exists());
    drop(lease);
    assert_eq!(cli(dir, &["clean", "--dry-run"])["removedEntries"], 1);
    assert!(path.exists());
    assert_eq!(cli(dir, &["clean"])["removedBytes"], 3);
    assert!(!path.exists());
    assert_eq!(cli(dir, &["clean"])["removedBytes"], 0);
    assert_eq!(fs::read(dir.join("cutcraft.json")).unwrap(), before);
    assert_eq!(fs::read(dir.join("history.jsonl")).unwrap(), history);
}
#[test]
fn lease_child() {
    let Some(dir) = std::env::var_os("CUTCRAFT_CACHE_LEASE_TEST") else {
        return;
    };
    let dir = Path::new(&dir);
    let m = Manager::new(dir);
    let path = m
        .directory(Kind::Decode)
        .unwrap()
        .join(format!("{}.raw", "b".repeat(64)));
    let entry = m.entry(&path).unwrap();
    fs::write(entry.path(), b"raw").unwrap();
    let scratch = m.scratch().unwrap();
    fs::write(scratch.path().join("raw"), b"incomplete").unwrap();
    fs::write(dir.join("child-ready"), b"ready").unwrap();
    // Parent kills this process to model a crash, not graceful guard cleanup.
    std::thread::sleep(Duration::from_secs(30));
    drop((entry, scratch));
}
#[test]
fn crash_releases_os_lease_and_stale_staging_is_reclaimed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    drop(Store::init(dir, "crash").unwrap());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_child", "--nocapture"])
        .env("CUTCRAFT_CACHE_LEASE_TEST", dir)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dir.join("child-ready").exists() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not acquire cache lease");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let report = cli(dir, &["clean"]);
    assert_eq!(report["busyBytes"], 13);
    assert_eq!(report["removedEntries"], 0);
    child.kill().unwrap();
    child.wait().unwrap();
    let report = cli(dir, &["clean"]);
    assert_eq!(report["removedBytes"], 13);
    assert_eq!(report["removedEntries"], 2);
}
