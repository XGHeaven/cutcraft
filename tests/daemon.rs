use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};

struct HostTest {
    root: tempfile::TempDir,
    runtime: std::path::PathBuf,
    project: std::path::PathBuf,
    path_override: Option<std::ffi::OsString>,
}
impl HostTest {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let project = root.path().join("project");
        let host = Self {
            root,
            runtime,
            project,
            path_override: None,
        };
        host.success(&[
            "init",
            host.project.to_str().unwrap(),
            "--name",
            "daemon test",
        ]);
        host
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cutcraft"));
        command
            .env("CUTCRAFT_RUNTIME_DIR", &self.runtime)
            .args(args);
        if let Some(path) = &self.path_override {
            command.env("PATH", path);
        }
        command.output().unwrap()
    }
    fn success(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn record(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.runtime.join("host.json")).unwrap()).unwrap()
    }
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.record()["port"])
    }
    fn request(&self, path: &str, body: Value) -> Value {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(self.url(path))
            .bearer_auth(self.record()["token"].as_str().unwrap())
            .json(&body)
            .send()
            .unwrap()
            .json()
            .unwrap()
    }
}
impl Drop for HostTest {
    fn drop(&mut self) {
        let _ = self.run(&["close", self.project.to_str().unwrap()]);
        let _ = &self.root;
    }
}

#[test]
fn local_host_routes_open_projects_and_replays_requests_without_changing_operations() {
    let host = HostTest::new();
    let opened = host.success(&["open", host.project.to_str().unwrap(), "--no-browser"]);
    let handle = opened["handle"].as_str().unwrap();
    let repeated = host.success(&["open", host.project.to_str().unwrap(), "--no-browser"]);
    assert_eq!(repeated["handle"], handle);
    let body = json!({"requestId":"rename-1","baseRevision":0,"command":{"type":"project.update","changes":{"name":"from web"}}});
    let endpoint = format!("/api/v1/projects/{handle}/commands");
    let first = host.request(&endpoint, body.clone());
    assert_eq!(first["ok"], true, "{first}");
    let replay = host.request(&endpoint, body.clone());
    assert_eq!(first, replay);
    let mut changed = body;
    changed["command"]["changes"]["name"] = json!("different");
    assert_eq!(
        host.request(&endpoint, changed)["error"]["code"],
        "REQUEST_ID_REUSED"
    );
    let inspected = host.success(&["-p", host.project.to_str().unwrap(), "inspect"]);
    assert_eq!(inspected["revision"], 1);
    assert_eq!(inspected["name"], "from web");
    assert_eq!(inspected["lastModifiedWith"]["id"], "cutcraft.web");
    let stale=host.request(&endpoint,json!({"requestId":"stale","baseRevision":0,"command":{"type":"project.update","changes":{"name":"stale"}}}));
    assert_eq!(stale["error"]["code"], "REVISION_CONFLICT");
    let undo = host.request(
        &format!("/api/v1/projects/{handle}/history/undo"),
        json!({"requestId":"undo","baseRevision":1}),
    );
    assert_eq!(undo["project"]["name"], "daemon test");
    let closed = host.success(&["close", host.project.to_str().unwrap()]);
    assert_eq!(closed["stopping"], true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.runtime.join("host.json").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!host.runtime.join("host.json").exists());
    assert_eq!(
        host.success(&["-p", host.project.to_str().unwrap(), "inspect"])["revision"],
        2
    );
}

#[test]
fn authentication_origin_single_use_code_and_multiple_projects() {
    let host = HostTest::new();
    let opened = host.success(&[
        "open",
        host.project.to_str().unwrap(),
        "--no-browser",
        "--host",
        "0.0.0.0",
    ]);
    assert_eq!(opened["authenticationRequired"], true);
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(host.url("/api/v1/projects"))
            .send()
            .unwrap()
            .status(),
        401
    );
    let code = opened["url"]
        .as_str()
        .unwrap()
        .split("#code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let session = client
        .post(host.url("/api/v1/session"))
        .json(&json!({"code":code}))
        .send()
        .unwrap();
    assert!(session.status().is_success());
    let cookie = session.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        client
            .post(host.url("/api/v1/session"))
            .json(&json!({"code":code}))
            .send()
            .unwrap()
            .status(),
        401
    );
    assert!(client
        .get(host.url("/api/v1/projects"))
        .header("cookie", &cookie)
        .send()
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        client
            .post(host.url("/api/v1/close"))
            .header("cookie", &cookie)
            .header("origin", "https://evil.example")
            .json(&json!({"path":host.project}))
            .send()
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(host.url("/api/v1/projects"))
            .header("host", "evil.example")
            .bearer_auth(host.record()["token"].as_str().unwrap())
            .send()
            .unwrap()
            .status(),
        403
    );
    // A browser using the machine's LAN IP must authenticate, then its own
    // same-origin authority works rather than being compared to loopback.
    let lan = format!("192.0.2.10:{}", host.record()["port"]);
    let endpoint = format!(
        "/api/v1/projects/{}/commands",
        opened["handle"].as_str().unwrap()
    );
    let command = json!({"requestId":"remote-cookie","baseRevision":0,"command":{"type":"project.update","changes":{"name":"Authenticated remote browser"}}});
    assert_eq!(
        client
            .post(host.url(&endpoint))
            .header("host", &lan)
            .header("origin", format!("http://{lan}"))
            .json(&command)
            .send()
            .unwrap()
            .status(),
        401
    );
    assert!(client
        .post(host.url(&endpoint))
        .header("host", &lan)
        .header("origin", format!("http://{lan}"))
        .header("cookie", &cookie)
        .json(&command)
        .send()
        .unwrap()
        .status()
        .is_success());
    let other = host.root.path().join("second");
    host.success(&["init", other.to_str().unwrap()]);
    host.success(&["open", other.to_str().unwrap(), "--no-browser"]);
    assert_eq!(
        host.success(&["close", other.to_str().unwrap()])["stopping"],
        false
    );
    let unopened = host.root.path().join("unopened");
    host.success(&["init", unopened.to_str().unwrap()]);
    assert!(host.success(&["-p", unopened.to_str().unwrap(), "inspect"])["id"].is_string());
    let lifecycle = json!({"instanceId":host.record()["instanceId"],"requestId":"bad","cli":{"project":host.project,"override_locks":false,"dry_run":false,"command":{"Serve":{"host":"127.0.0.1"}}}});
    assert_eq!(
        host.request("/api/v1/execute", lifecycle)["error"]["code"],
        "INVALID_REQUEST"
    );
    assert!(Path::new(&host.project).join("cutcraft.json").is_file());
}

#[test]
fn last_close_finishes_existing_and_late_event_streams() {
    let host = HostTest::new();
    host.success(&["open", host.project.to_str().unwrap(), "--no-browser"]);
    let record = host.record();
    let url = host.url("/api/v1/events");
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let existing = client
        .get(&url)
        .bearer_auth(record["token"].as_str().unwrap())
        .send()
        .unwrap();
    let closed = host.request("/api/v1/close", json!({"path":host.project}));
    assert_eq!(closed["stopping"], true);
    // The closing broadcast has already happened. This listener must also see
    // the persistent shutdown signal, rather than holding graceful shutdown open.
    if let Ok(late) = client
        .get(&url)
        .bearer_auth(record["token"].as_str().unwrap())
        .send()
    {
        let body = late.text().unwrap();
        assert_eq!(body.matches("server.closing").count(), 1, "{body}");
    }
    let body = existing.text().unwrap();
    assert_eq!(body.matches("server.closing").count(), 1, "{body}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while host.runtime.join("host.json").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!host.runtime.join("host.json").exists());
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", record["port"].as_u64().unwrap() as u16))
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn import_busy_close_preserves_open_project_and_allows_concurrent_edits() {
    use std::os::unix::fs::PermissionsExt;
    let mut host = HostTest::new();
    let available = Command::new("ffmpeg").arg("-version").output().is_ok();
    assert!(available || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    if !available {
        return;
    }
    let fixture = host.root.path().join("clip.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=32x32:d=0.1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let real_probe = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("ffprobe"))
        .find(|path| path.is_file())
        .unwrap();
    let bin = host.root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let gate = host.root.path().join("probe-gate");
    let started = host.root.path().join("probe-started");
    std::fs::write(&gate, b"wait").unwrap();
    struct Release(std::path::PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let release = Release(gate.clone());
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let wrapper = bin.join("ffprobe");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n: > {}\nwhile test -e {}; do sleep 0.05; done\nexec {} \"$@\"\n",
            quote(&started),
            quote(&gate),
            quote(&real_probe)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    host.path_override = Some(std::env::join_paths(paths).unwrap());
    let opened = host.success(&["open", host.project.to_str().unwrap(), "--no-browser"]);
    let handle = opened["handle"].as_str().unwrap();
    let request=host.request(&format!("/api/v1/projects/{handle}/requests"),json!({"requestId":"slow-import","command":{"Import":{"file":fixture,"color_space":null,"managed":false,"append":false}}}));
    let task = request["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("{request}"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(started.exists());
    let close = host.run(&["close", host.project.to_str().unwrap()]);
    assert!(!close.status.success());
    assert!(String::from_utf8_lossy(&close.stderr).contains("PROJECT_BUSY"));
    let edit=host.request(&format!("/api/v1/projects/{handle}/commands"),json!({"requestId":"during-import","baseRevision":0,"command":{"type":"project.update","changes":{"name":"edited during probe"}}}));
    assert_eq!(edit["ok"], true, "{edit}");
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let result: Value = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(host.url(&format!("/api/v1/tasks/{task}")))
            .bearer_auth(host.record()["token"].as_str().unwrap())
            .send()
            .unwrap()
            .json()
            .unwrap();
        if result["task"]["status"] == "completed" {
            break;
        }
        assert!(result["task"]["status"] != "failed", "{result}");
        assert!(Instant::now() < deadline, "{result}");
        std::thread::sleep(Duration::from_millis(30));
    }
    let project = host.success(&["-p", host.project.to_str().unwrap(), "inspect"]);
    assert_eq!(project["name"], "edited during probe");
    assert_eq!(project["revision"], 2);
    assert_eq!(project["assets"].as_object().unwrap().len(), 1);
}

#[test]
fn draft_preview_preserves_disk_and_rejects_old_generations() {
    let host = HostTest::new();
    let available = Command::new("ffmpeg").arg("-version").output().is_ok();
    assert!(available || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
    if !available {
        return;
    }
    let opened = host.success(&["open", host.project.to_str().unwrap(), "--no-browser"]);
    let handle = opened["handle"].as_str().unwrap();
    let before = std::fs::read(host.project.join("cutcraft.json")).unwrap();
    let project: Value = serde_json::from_slice(&before).unwrap();
    let history = std::fs::read(host.project.join("history.jsonl")).unwrap();
    let draft = json!({"scale":0.25,"sequence":project["rootSequenceId"],"at":{"num":0,"den":1},"revision":0,"generation":2,"clientId":"draft-test","command":{"type":"track.put","sequenceId":project["rootSequenceId"],"track":{"id":cutcraft::id("track"),"kind":"video","name":"Draft","overlapPolicy":"forbid","items":[{"id":cutcraft::id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}}]}}});
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    let response = client
        .post(host.url(&format!("/api/v1/projects/{handle}/preview/frame")))
        .bearer_auth(host.record()["token"].as_str().unwrap())
        .json(&draft)
        .send()
        .unwrap();
    let status = response.status();
    let revision = response.headers().get("x-cutcraft-revision").cloned();
    let bytes = response.bytes().unwrap();
    assert!(status.is_success(), "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(revision.unwrap(), "0");
    assert!(bytes.starts_with(b"\x89PNG"));
    let expected_width = (project["sequences"][project["rootSequenceId"].as_str().unwrap()]
        ["format"]["width"]
        .as_u64()
        .unwrap()
        / 4
        / 2
        * 2) as u32;
    assert_eq!(
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        expected_width
    );
    for endpoint in ["frame", "playback"] {
        let invalid = client
            .post(host.url(&format!("/api/v1/projects/{handle}/preview/{endpoint}")))
            .bearer_auth(host.record()["token"].as_str().unwrap())
            .json(&json!({"scale":0,"at":{"num":0,"den":1}}))
            .send()
            .unwrap();
        assert!(!invalid.status().is_success());
        assert!(invalid.text().unwrap().contains("INVALID_PREVIEW_SCALE"));
    }
    assert_eq!(
        std::fs::read(host.project.join("cutcraft.json")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(host.project.join("history.jsonl")).unwrap(),
        history
    );
    let stale=client.get(host.url(&format!("/api/v1/projects/{handle}/preview/frame?at=0&revision=0&generation=1&clientId=draft-test"))).bearer_auth(host.record()["token"].as_str().unwrap()).send().unwrap();
    let error: Value = stale.json().unwrap();
    assert_eq!(error["error"]["code"], "STALE_PREVIEW");
}

#[test]
fn loopback_needs_no_code_or_cookie_and_keeps_origin_and_binding_checks() {
    let host = HostTest::new();
    let opened = host.success(&["open", host.project.to_str().unwrap(), "--no-browser"]);
    assert_eq!(opened["authenticationRequired"], false);
    assert!(!opened["url"].as_str().unwrap().contains('#'));
    assert!(opened["url"].as_str().unwrap().contains("?project="));
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    for _ in 0..2 {
        let response = client.get(host.url("/api/v1/projects")).send().unwrap();
        assert!(response.status().is_success());
        assert!(response.headers().get("set-cookie").is_none());
    }
    let session = client
        .post(host.url("/api/v1/session"))
        .json(&json!({"code":"old-expired-code"}))
        .send()
        .unwrap();
    assert!(session.status().is_success());
    assert!(session.headers().get("set-cookie").is_none());
    let endpoint = format!(
        "/api/v1/projects/{}/commands",
        opened["handle"].as_str().unwrap()
    );
    let command = json!({"requestId":"without-cookie","baseRevision":0,"command":{"type":"project.update","changes":{"name":"Local browser"}}});
    assert_eq!(
        client
            .post(host.url(&endpoint))
            .header("origin", "https://evil.example")
            .json(&command)
            .send()
            .unwrap()
            .status(),
        403
    );
    let authority = format!("localhost:{}", host.record()["port"]);
    assert!(client
        .post(host.url(&endpoint))
        .header("host", &authority)
        .header("origin", format!("http://{authority}"))
        .json(&command)
        .send()
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        client
            .get(host.url("/api/v1/projects"))
            .header("host", format!("evil.example:{}", host.record()["port"]))
            .send()
            .unwrap()
            .status(),
        403
    );
    let same = host.success(&[
        "open",
        host.project.to_str().unwrap(),
        "--host",
        "localhost",
        "--no-browser",
    ]);
    assert_eq!(opened["url"], same["url"]);
    let conflict = host.run(&[
        "open",
        host.project.to_str().unwrap(),
        "--host",
        "0.0.0.0",
        "--no-browser",
    ]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("HOST_BIND_CONFLICT"));
    assert_eq!(host.record()["bindHost"], "127.0.0.1");
    assert_eq!(
        host.success(&["-p", host.project.to_str().unwrap(), "inspect"])["name"],
        "Local browser"
    );
}

#[test]
fn ipv6_loopback_urls_and_anonymous_requests_work() {
    let host = HostTest::new();
    let opened = host.success(&[
        "open",
        host.project.to_str().unwrap(),
        "--host",
        "::1",
        "--no-browser",
    ]);
    assert_eq!(opened["authenticationRequired"], false);
    assert!(opened["url"].as_str().unwrap().starts_with("http://[::1]:"));
    let base = opened["url"].as_str().unwrap().split("/?").next().unwrap();
    let response = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{base}/api/v1/projects"))
        .send()
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        host.success(&["close", host.project.to_str().unwrap()])["stopping"],
        true
    );
}
