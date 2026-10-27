//! One local host per user. Project actors serialize transactions, not rendering.
use crate::{
    app, edit, project,
    store::{self, Store},
};
use anyhow::{bail, ensure, Context, Result};
use axum::{
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, watch, Mutex as AsyncMutex, Semaphore};
use tokio_stream::{
    wrappers::{BroadcastStream, WatchStream},
    StreamExt,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    instance_id: String,
    token: String,
    port: u16,
    pid: u32,
    #[serde(default = "default_host")]
    bind_host: IpAddr,
}
fn default_host() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}
pub fn parse_host(value: &str) -> std::result::Result<IpAddr, String> {
    if value.eq_ignore_ascii_case("localhost") {
        return Ok(default_host());
    }
    value
        .parse()
        .map_err(|_| "host must be an IP address or localhost".to_owned())
}
fn loopback(host: IpAddr) -> bool {
    host.is_loopback()
        || matches!(host, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback()))
}
impl Runtime {
    fn connect_host(&self) -> IpAddr {
        match self.bind_host {
            IpAddr::V4(ip) if ip.is_unspecified() => default_host(),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        }
    }
    fn url(&self) -> String {
        format!("http://{}", SocketAddr::new(self.connect_host(), self.port))
    }
    fn auth_required(&self) -> bool {
        !loopback(self.bind_host)
    }
    fn check_binding(&self, requested: Option<IpAddr>) -> Result<()> {
        ensure!(
            requested.is_none_or(|host| host == self.bind_host),
            "HOST_BIND_CONFLICT: service listens on {}; close all projects before changing --host",
            self.bind_host
        );
        Ok(())
    }
    fn accepts_authority(&self, authority: &str) -> bool {
        let Ok(authority) = authority.parse::<axum::http::uri::Authority>() else {
            return false;
        };
        if authority.port_u16() != Some(self.port) {
            return false;
        }
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        if host.eq_ignore_ascii_case("localhost") {
            return loopback(self.bind_host) || self.bind_host.is_unspecified();
        }
        let Ok(ip) = host.parse::<IpAddr>() else {
            return false;
        };
        if ip.is_unspecified() || ip.is_multicast() {
            return false;
        }
        if self.bind_host.is_unspecified() {
            return true;
        }
        ip == self.bind_host || (loopback(self.bind_host) && loopback(ip))
    }
}

struct Project {
    handle: String,
    path: PathBuf,
    gate: AsyncMutex<()>,
    closed: AtomicBool,
    busy: AtomicUsize,
    cached: Mutex<Value>,
    generations: Mutex<HashMap<String, u64>>,
    media_slots: Arc<Semaphore>,
}
struct Registry {
    projects: HashMap<String, Arc<Project>>,
    stopping: bool,
    opening: usize,
}
struct Receipt {
    fingerprint: String,
    result: watch::Receiver<Option<Value>>,
    sender: watch::Sender<Option<Value>>,
    created: Instant,
    bytes: usize,
}
struct Task {
    value: Mutex<Value>,
    cancellation: crate::progress::CancellationToken,
    directory: Option<PathBuf>,
    finished: watch::Sender<bool>,
    completed: Mutex<Option<Instant>>,
}
struct Host {
    runtime: Runtime,
    registry: Mutex<Registry>,
    receipts: Mutex<HashMap<String, Receipt>>,
    sessions: Mutex<HashSet<String>>,
    codes: Mutex<HashMap<String, Instant>>,
    tasks: Mutex<HashMap<String, Arc<Task>>>,
    events: broadcast::Sender<Value>,
    shutdown: watch::Sender<bool>,
    actor: Value,
}
type Shared = Arc<Host>;
type ApiResult = std::result::Result<Json<Value>, ApiError>;
fn api_value(value: Value) -> ApiResult {
    if value["ok"] == false {
        Err(ApiError(
            RemoteError {
                detail: value["error"].clone(),
            }
            .into(),
        ))
    } else {
        Ok(Json(value))
    }
}
struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let value = error_value(&self.0);
        let code = value["error"]["code"].as_str().unwrap_or("");
        let status = match code {
            "UNAUTHORIZED" => StatusCode::UNAUTHORIZED,
            "INVALID_ORIGIN" | "INVALID_HOST" => StatusCode::FORBIDDEN,
            "PROJECT_NOT_OPEN" | "TASK_NOT_FOUND" => StatusCode::NOT_FOUND,
            "PROJECT_BUSY" | "PROJECT_CLOSED" | "REVISION_CONFLICT" | "REQUEST_ID_REUSED"
            | "INSTANCE_CHANGED" => StatusCode::CONFLICT,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(value)).into_response()
    }
}
fn error_value(error: &anyhow::Error) -> Value {
    if let Some(remote) = error.downcast_ref::<RemoteError>() {
        return json!({"ok":false,"error":remote.detail});
    }
    let message = format!("{error:#}");
    let first = message.split([':', ' ']).next().unwrap_or("ERROR");
    let code = if first.len() > 2 && first.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
        first
    } else {
        "ERROR"
    };
    let mut result = json!({"ok":false,"error":{"code":code,"message":message}});
    if let Some(media) = error.downcast_ref::<crate::media_process::MediaToolError>() {
        result["error"]["code"] = json!(media.code());
        result["error"]["executable"] = json!(media.executable);
        result["error"]["hint"] = json!(media.hint());
    }
    result
}
#[derive(Debug)]
pub struct RemoteError {
    pub detail: Value,
}
impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.detail["message"]
                .as_str()
                .unwrap_or("remote request failed")
        )
    }
}
impl std::error::Error for RemoteError {}
fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    )
}
fn runtime_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CUTCRAFT_RUNTIME_DIR") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os(if cfg!(windows) {
        "LOCALAPPDATA"
    } else {
        "HOME"
    })
    .context("user runtime directory unavailable")?;
    Ok(PathBuf::from(home).join(".cutcraft").join("run"))
}
fn private_dir(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "runtime directory must not be a symlink"
        );
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        let who = Command::new("whoami")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()?;
        ensure!(
            who.status.success(),
            "cannot identify runtime directory owner"
        );
        let text = String::from_utf8(who.stdout)?;
        let sid = text
            .split(['"', ',', '\r', '\n'])
            .find(|part| part.starts_with("S-1-"))
            .context("cannot identify Windows user SID")?;
        for args in [
            vec!["/reset".to_owned()],
            vec![
                "/inheritance:r".to_owned(),
                "/grant:r".to_owned(),
                format!("*{sid}:(OI)(CI)F"),
            ],
        ] {
            let output = Command::new("icacls").arg(path).args(args).output()?;
            ensure!(
                output.status.success(),
                "cannot restrict runtime directory permissions: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    Ok(())
}
fn runtime_record() -> Option<Runtime> {
    serde_json::from_slice(&fs::read(runtime_dir().ok()?.join("host.json")).ok()?).ok()
}
fn runtime_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.join("host.lock"))?;
    file.try_lock_exclusive()
        .context("HOST_RUNNING: another host owns this user runtime")?;
    Ok(file)
}
fn alive_lock() -> bool {
    runtime_dir()
        .ok()
        .and_then(|p| runtime_lock(&p).ok())
        .is_none()
}
fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()?)
}
fn rpc(runtime: &Runtime, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let request = client()?
        .request(method.parse()?, format!("{}{path}", runtime.url()))
        .bearer_auth(&runtime.token);
    let response = if let Some(body) = body {
        request.json(body)
    } else {
        request
    }
    .send()?;
    let value: Value = response.json()?;
    if value["ok"] == false {
        return Err(RemoteError {
            detail: value["error"].clone(),
        }
        .into());
    }
    Ok(value)
}
fn ping(runtime: &Runtime) -> Result<()> {
    let status = rpc(runtime, "GET", "/api/v1/status", None)?;
    ensure!(
        status["apiVersion"] == 1 && status["instanceId"] == runtime.instance_id,
        "HOST_PROTOCOL_MISMATCH: incompatible host instance"
    );
    ensure!(status["stopping"] != true, "HOST_STOPPING");
    Ok(())
}
fn ensure_host(host: Option<IpAddr>) -> Result<Runtime> {
    if let Some(runtime) = runtime_record() {
        if ping(&runtime).is_ok() {
            runtime.check_binding(host)?;
            return Ok(runtime);
        }
        if rpc(&runtime, "GET", "/api/v1/status", None)
            .is_ok_and(|status| status["stopping"] == true)
        {
            let deadline = Instant::now() + Duration::from_secs(5);
            while alive_lock() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        ensure!(
            !alive_lock(),
            "HOST_UNAVAILABLE: active host is not responding; no local fallback"
        );
    }
    let dir = runtime_dir()?;
    private_dir(&dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("host.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("__serve")
        .arg("--host")
        .arg(host.unwrap_or_else(default_host).to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only async-signal-safe setsid runs between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200);
    }
    let mut child = command.spawn()?;
    for _ in 0..120 {
        if let Some(runtime) = runtime_record() {
            if ping(&runtime).is_ok() {
                runtime.check_binding(host)?;
                return Ok(runtime);
            }
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success() || alive_lock(),
                "host exited during startup; see {}",
                dir.join("host.log").display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("HOST_START_TIMEOUT: see {}", dir.join("host.log").display())
}
pub fn open(path: &Path, no_browser: bool, host: Option<IpAddr>) -> Result<Value> {
    let path = path.canonicalize()?;
    let runtime = ensure_host(host)?;
    let result = rpc(
        &runtime,
        "POST",
        "/api/v1/projects/open",
        Some(&json!({"path":path})),
    )?;
    if !no_browser {
        if let Some(url) = result["url"].as_str() {
            let _ = browser(url);
        }
    }
    Ok(result)
}
fn browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
pub fn close(path: &Path) -> Result<Value> {
    let Some(runtime) = runtime_record() else {
        return Ok(json!({"ok":true,"closed":false}));
    };
    if !alive_lock() {
        return Ok(json!({"ok":true,"closed":false}));
    }
    let path = path.canonicalize()?;
    rpc(
        &runtime,
        "POST",
        "/api/v1/close",
        Some(&json!({"path":path})),
    )
}
pub fn route(cli: &app::Cli) -> Result<Option<Value>> {
    let Some(runtime) = runtime_record() else {
        return Ok(None);
    };
    if !alive_lock() {
        return Ok(None);
    }
    let Ok(path) = cli.project.canonicalize() else {
        return Ok(None);
    };
    let projects = rpc(&runtime, "GET", "/api/v1/projects", None)
        .context("HOST_UNAVAILABLE: cannot determine project owner; no local fallback")?;
    if !projects["projects"].as_array().is_some_and(|list| {
        list.iter()
            .any(|p| p["path"] == path.to_string_lossy().as_ref())
    }) {
        return Ok(None);
    }
    let mut wire = serde_json::to_value(cli)?;
    wire["project"] = json!(path);
    let result = rpc(
        &runtime,
        "POST",
        "/api/v1/execute",
        Some(
            &json!({"instanceId":runtime.instance_id,"requestId":secret(),"cli":wire,"async":true}),
        ),
    )?;
    let Some(task_id) = result["taskId"].as_str() else {
        return Ok(Some(result));
    };
    let mut last_progress = Value::Null;
    loop {
        let response = rpc(&runtime, "GET", &format!("/api/v1/tasks/{task_id}"), None)?;
        let task = &response["task"];
        if task["progress"].is_object() && task["progress"] != last_progress {
            last_progress = task["progress"].clone();
            relay_progress(cli, &last_progress);
        }
        match task["status"].as_str() {
            Some("completed") => return Ok(Some(task["result"].clone())),
            Some("failed" | "cancelled") => {
                return Err(RemoteError {
                    detail: task["error"].clone(),
                }
                .into())
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}
fn relay_progress(cli: &app::Cli, value: &Value) {
    use std::io::{IsTerminal, Write};
    let mode = match cli.command {
        app::Cmd::Render { progress, .. } | app::Cmd::Preview { progress, .. } => progress,
        _ => app::ProgressMode::None,
    };
    let mut stderr = std::io::stderr().lock();
    match mode {
        app::ProgressMode::Json => {
            let _ = writeln!(stderr, "{value}");
        }
        app::ProgressMode::Human | app::ProgressMode::Auto
            if matches!(mode, app::ProgressMode::Human) || std::io::stderr().is_terminal() =>
        {
            let _ = writeln!(
                stderr,
                "[{}] {}/{} {} | {}",
                value["phase"].as_str().unwrap_or("working"),
                value["completed"],
                value["total"],
                value["unit"].as_str().unwrap_or("items"),
                value["status"].as_str().unwrap_or("running")
            );
        }
        _ => {}
    }
}
pub fn serve(host: IpAddr) -> Result<()> {
    let dir = runtime_dir()?;
    private_dir(&dir)?;
    let _lock = runtime_lock(&dir)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(host, 0)).await?;
        let record = Runtime {
            instance_id: secret(),
            token: secret(),
            port: listener.local_addr()?.port(),
            pid: std::process::id(),
            bind_host: host,
        };
        let (event_sender, _) = broadcast::channel(256);
        let (shutdown, receiver) = watch::channel(false);
        let state = Arc::new(Host {
            runtime: record.clone(),
            registry: Mutex::new(Registry {
                projects: HashMap::new(),
                stopping: false,
                opening: 0,
            }),
            receipts: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashSet::new()),
            codes: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            events: event_sender,
            shutdown,
            actor: json!({"type":"user","id":crate::id("actor"),"displayName":"Cutcraft Web"}),
        });
        store::atomic_write(&dir.join("host.json"), &serde_json::to_vec(&record)?)?;
        let app = Router::new()
            .route("/api/v1/status", get(status))
            .route("/api/v1/session", post(session))
            .route("/api/v1/projects", get(projects))
            .route("/api/v1/projects/open", post(open_project))
            .route("/api/v1/open", post(open_project))
            .route("/api/v1/close", post(close_path))
            .route("/api/v1/projects/{handle}/close", post(close_project))
            .route("/api/v1/projects/{handle}/snapshot", get(snapshot))
            .route("/api/v1/projects/{handle}/commands", post(command))
            .route("/api/v1/projects/{handle}/operations", post(operation))
            .route(
                "/api/v1/projects/{handle}/history/{action}",
                post(history_action),
            )
            .route("/api/v1/projects/{handle}/requests", post(project_request))
            .route(
                "/api/v1/projects/{handle}/preview/frame",
                get(preview_frame).post(preview_draft),
            )
            .route(
                "/api/v1/projects/{handle}/preview/playback",
                post(preview_playback),
            )
            .route(
                "/api/v1/projects/{handle}/assets/{asset}/thumbnail",
                get(thumbnail),
            )
            .route(
                "/api/v1/projects/{handle}/assets/{asset}/waveform",
                get(waveform),
            )
            .route("/api/v1/execute", post(execute))
            .route("/api/v1/tasks", get(tasks))
            .route("/api/v1/tasks/{task}", get(task))
            .route("/api/v1/tasks/{task}/cancel", post(cancel_task))
            .route("/api/v1/tasks/{task}/files/{file}", get(task_file))
            .route("/api/v1/events", get(events))
            .fallback(get(asset))
            .layer(middleware::from_fn_with_state(state.clone(), authenticate))
            .with_state(state.clone());
        tokio::spawn(refresh_loop(state.clone()));
        let mut shutdown = receiver;
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while !*shutdown.borrow() {
                    if shutdown.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await?;
        if runtime_record().is_some_and(|r| r.instance_id == record.instance_id) {
            let _ = fs::remove_file(dir.join("host.json"));
        }
        Ok::<_, anyhow::Error>(())
    })
}

async fn authenticate(State(state): State<Shared>, request: Request<Body>, next: Next) -> Response {
    fn check(state: &Shared, request: &Request<Body>) -> Result<()> {
        let headers = request.headers();
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        ensure!(
            state.runtime.accepts_authority(host),
            "INVALID_HOST: use the host URL returned by open"
        );
        if let Some(origin) = headers.get(header::ORIGIN) {
            ensure!(
                origin.to_str().ok() == Some(format!("http://{host}").as_str()),
                "INVALID_ORIGIN: cross-origin requests are forbidden"
            );
        }
        if !request.uri().path().starts_with("/api/") || request.uri().path() == "/api/v1/session" {
            return Ok(());
        }
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            == Some(format!("Bearer {}", state.runtime.token).as_str())
        {
            return Ok(());
        }
        if !state.runtime.auth_required() {
            return Ok(());
        }
        let cookie = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .find_map(|c| c.trim().strip_prefix("cutcraft_session="));
        ensure!(
            cookie.is_some_and(|cookie| state.sessions.lock().unwrap().contains(cookie)),
            "UNAUTHORIZED: open a fresh project URL to connect"
        );
        if request.method() != "GET" && request.method() != "HEAD" {
            ensure!(
                headers.contains_key(header::ORIGIN),
                "INVALID_ORIGIN: browser mutations require an Origin header"
            );
        }
        Ok(())
    }
    if let Err(error) = check(&state, &request) {
        return ApiError(error).into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response
}
async fn status(State(state): State<Shared>) -> Json<Value> {
    Json(
        json!({"ok":true,"instanceId":state.runtime.instance_id,"apiVersion":1,"bindHost":state.runtime.bind_host,"authenticationRequired":state.runtime.auth_required(),"stopping":state.registry.lock().unwrap().stopping}),
    )
}
async fn session(
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> std::result::Result<Response, ApiError> {
    if !state.runtime.auth_required() {
        return Ok(Json(json!({"ok":true,"authenticationRequired":false})).into_response());
    }
    let code = body["code"]
        .as_str()
        .context("UNAUTHORIZED: missing startup code")?;
    let issued = state
        .codes
        .lock()
        .unwrap()
        .remove(code)
        .context("UNAUTHORIZED: startup code invalid or already used")?;
    ensure_api(
        issued.elapsed() < Duration::from_secs(300),
        "UNAUTHORIZED: startup code expired",
    )?;
    let session = secret();
    state.sessions.lock().unwrap().insert(session.clone());
    let mut response =
        Json(json!({"ok":true,"instanceId":state.runtime.instance_id})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("cutcraft_session={session}; Path=/api/; HttpOnly; SameSite=Strict")
            .parse()
            .unwrap(),
    );
    Ok(response)
}
fn ensure_api(condition: bool, message: &str) -> std::result::Result<(), ApiError> {
    if condition {
        Ok(())
    } else {
        Err(ApiError(anyhow::anyhow!("{message}")))
    }
}
fn get_project(state: &Shared, handle: &str) -> Result<Arc<Project>> {
    state
        .registry
        .lock()
        .unwrap()
        .projects
        .get(handle)
        .cloned()
        .context("PROJECT_NOT_OPEN: project is not registered")
}
fn summary(project: &Project) -> Value {
    let cached = project.cached.lock().unwrap();
    json!({"handle":project.handle,"path":project.path,"name":cached["name"],"projectId":cached["id"],"revision":cached["revision"]})
}
async fn projects(State(state): State<Shared>) -> Json<Value> {
    let registry = state.registry.lock().unwrap();
    let mut list: Vec<Value> = registry.projects.values().map(|p| summary(p)).collect();
    list.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Json(json!({"projects":list}))
}
async fn open_project(State(state): State<Shared>, Json(body): Json<Value>) -> ApiResult {
    let path = PathBuf::from(body["path"].as_str().context("path is required")?).canonicalize()?;
    let existing = {
        let mut registry = state.registry.lock().unwrap();
        ensure_api(
            !registry.stopping,
            "INSTANCE_CHANGED: host is stopping; retry open after it exits",
        )?;
        let existing = registry.projects.values().find(|p| p.path == path).cloned();
        if existing.is_none() {
            registry.opening += 1;
        }
        existing
    };
    let entry = if let Some(entry) = existing {
        entry
    } else {
        let path2 = path.clone();
        let opened = tokio::task::spawn_blocking(move || open_store(&path2)?.project())
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
        let mut registry = state.registry.lock().unwrap();
        registry.opening -= 1;
        let snapshot = match opened {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if registry.projects.is_empty() && registry.opening == 0 {
                    registry.stopping = true;
                    schedule_stop(&state);
                }
                return Err(error.into());
            }
        };
        if let Some(existing) = registry.projects.values().find(|p| p.path == path).cloned() {
            existing
        } else {
            let entry = Arc::new(Project {
                handle: crate::id("project"),
                path,
                gate: AsyncMutex::new(()),
                closed: AtomicBool::new(false),
                busy: AtomicUsize::new(0),
                cached: Mutex::new(snapshot),
                generations: Mutex::new(HashMap::new()),
                media_slots: Arc::new(Semaphore::new(2)),
            });
            registry
                .projects
                .insert(entry.handle.clone(), entry.clone());
            entry
        }
    };
    let url = if state.runtime.auth_required() {
        let code = secret();
        state
            .codes
            .lock()
            .unwrap()
            .insert(code.clone(), Instant::now());
        format!(
            "{}/#code={}&project={}",
            state.runtime.url(),
            code,
            entry.handle
        )
    } else {
        format!("{}/?project={}", state.runtime.url(), entry.handle)
    };
    Ok(Json(
        json!({"ok":true,"project":summary(&entry),"handle":entry.handle,"url":url,"authenticationRequired":state.runtime.auth_required()}),
    ))
}
async fn close_path(State(state): State<Shared>, Json(body): Json<Value>) -> ApiResult {
    let path = PathBuf::from(body["path"].as_str().context("path is required")?).canonicalize()?;
    let entry = state
        .registry
        .lock()
        .unwrap()
        .projects
        .values()
        .find(|p| p.path == path)
        .cloned();
    if let Some(entry) = entry {
        finish_close(state, entry).await
    } else {
        Ok(Json(json!({"ok":true,"closed":false})))
    }
}
async fn close_project(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
) -> ApiResult {
    let entry = state
        .registry
        .lock()
        .unwrap()
        .projects
        .get(&handle)
        .cloned();
    if let Some(entry) = entry {
        finish_close(state, entry).await
    } else {
        Ok(Json(json!({"ok":true,"closed":false})))
    }
}
async fn finish_close(state: Shared, entry: Arc<Project>) -> ApiResult {
    ensure_api(
        entry.busy.load(Ordering::SeqCst) == 0,
        "PROJECT_BUSY: finish or cancel import/export jobs before closing",
    )?;
    let _gate = entry.gate.lock().await;
    ensure_api(
        entry.busy.load(Ordering::SeqCst) == 0,
        "PROJECT_BUSY: finish or cancel import/export jobs before closing",
    )?;
    entry.closed.store(true, Ordering::SeqCst);
    for task in state.tasks.lock().unwrap().values() {
        let value = task.value.lock().unwrap();
        if value["project"] == entry.handle && value["kind"] == "preview" {
            cancel_preview(task);
        }
    }
    let stopping = {
        let mut registry = state.registry.lock().unwrap();
        registry.projects.remove(&entry.handle);
        let stopping = registry.projects.is_empty() && registry.opening == 0;
        if stopping {
            registry.stopping = true;
        }
        stopping
    };
    let _ = state
        .events
        .send(json!({"type":"project.closed","project":entry.handle}));
    if stopping {
        schedule_stop(&state);
    }
    Ok(Json(json!({"ok":true,"closed":true,"stopping":stopping})))
}
fn schedule_stop(state: &Shared) {
    let _ = state.events.send(json!({"type":"server.closing"}));
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        state.shutdown.send_replace(true);
    });
}
fn open_store(path: &Path) -> Result<Store> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Store::open(path) {
            Err(error)
                if format!("{error:#}").contains("PROJECT_LOCKED") && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => return result,
        }
    }
}
async fn snapshot(State(state): State<Shared>, RoutePath(handle): RoutePath<String>) -> ApiResult {
    let entry = get_project(&state, &handle)?;
    let _gate = entry.gate.lock().await;
    ensure_api(
        !entry.closed.load(Ordering::SeqCst),
        "PROJECT_CLOSED: project was closed",
    )?;
    let path = entry.path.clone();
    let value = tokio::task::spawn_blocking(move || -> Result<Value> {
        let store = Store::open(&path)?;
        let project = store.project()?;
        let history = store.history()?;
        Ok(json!({"project":project,"history":store.history_state()?,"cursor":history.last().map(|v| &v["eventId"])}))
    }).await??;
    publish_snapshot(&state, &entry, &value["project"]);
    Ok(Json(value))
}
fn publish_snapshot(state: &Shared, entry: &Project, snapshot: &Value) {
    let mut cached = entry.cached.lock().unwrap();
    if *cached != *snapshot {
        *cached = snapshot.clone();
        for task in state.tasks.lock().unwrap().values() {
            let value = task.value.lock().unwrap();
            if value["project"] == entry.handle && value["kind"] == "preview" {
                cancel_preview(task);
            }
        }
        let _ = state.events.send(json!({"type":"project.changed","project":entry.handle,"revision":snapshot["revision"]}));
    }
}
async fn refresh_loop(state: Shared) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if *state.shutdown.borrow() {
            break;
        }
        prune_tasks(&state);
        for receipt in state.receipts.lock().unwrap().values_mut() {
            if receipt.bytes > 0 && receipt.created.elapsed() > Duration::from_secs(600) {
                receipt.bytes = 0;
                receipt.sender.send_replace(Some(expired_receipt()));
            }
        }
        let entries: Vec<_> = state
            .registry
            .lock()
            .unwrap()
            .projects
            .values()
            .cloned()
            .collect();
        for entry in entries {
            let Ok(_gate) = entry.gate.try_lock() else {
                continue;
            };
            if entry.closed.load(Ordering::SeqCst) {
                continue;
            }
            let path = entry.path.clone();
            if let Ok(Ok(snapshot)) =
                tokio::task::spawn_blocking(move || Store::open(&path)?.project()).await
            {
                publish_snapshot(&state, &entry, &snapshot);
            }
        }
    }
}
async fn deduplicate(
    state: &Shared,
    id: &str,
    body: &Value,
    run: impl std::future::Future<Output = Result<Value>> + Send + 'static,
) -> Result<Value> {
    ensure!(!id.is_empty() && id.len() <= 256, "REQUEST_ID_REQUIRED");
    let fingerprint = store::hash(body)?;
    let (mut receiver, sender) = {
        let mut receipts = state.receipts.lock().unwrap();
        if let Some(receipt) = receipts.get(id) {
            ensure!(
                receipt.fingerprint == fingerprint,
                "REQUEST_ID_REUSED: requestId belongs to different content"
            );
            (receipt.result.clone(), None)
        } else {
            ensure!(
                receipts.len() < 100_000,
                "REQUEST_CAPACITY: reopen the host before submitting more requests"
            );
            let (sender, receiver) = watch::channel(None);
            receipts.insert(
                id.into(),
                Receipt {
                    fingerprint,
                    result: receiver.clone(),
                    sender: sender.clone(),
                    created: Instant::now(),
                    bytes: 0,
                },
            );
            (receiver, Some(sender))
        }
    };
    if let Some(sender) = sender {
        let state = state.clone();
        let id = id.to_owned();
        return tokio::spawn(async move {
            let result = run.await.unwrap_or_else(|error| error_value(&error));
            let bytes = serde_json::to_vec(&result).map_or(usize::MAX, |bytes| bytes.len());
            let mut receipts = state.receipts.lock().unwrap();
            let mut used: usize = receipts.values().map(|r| r.bytes).sum();
            let mut candidates: Vec<_> = receipts
                .iter()
                .filter(|(_, r)| r.bytes > 0)
                .map(|(key, r)| (key.clone(), r.created))
                .collect();
            candidates.sort_by_key(|(_, created)| *created);
            for (key, _) in candidates {
                if used.saturating_add(bytes) <= 64 * 1024 * 1024 {
                    break;
                }
                let receipt = receipts.get_mut(&key).unwrap();
                used -= receipt.bytes;
                receipt.bytes = 0;
                receipt.sender.send_replace(Some(expired_receipt()));
            }
            if bytes <= 64 * 1024 * 1024 {
                receipts.get_mut(&id).unwrap().bytes = bytes;
                sender.send_replace(Some(result.clone()));
            } else {
                sender.send_replace(Some(expired_receipt()));
            }
            result
        })
        .await
        .context("REQUEST_OUTCOME_UNKNOWN: worker stopped before responding");
    }
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return Ok(result);
        }
        receiver
            .changed()
            .await
            .context("REQUEST_OUTCOME_UNKNOWN: original request ended without a receipt")?;
    }
}
fn expired_receipt() -> Value {
    json!({"ok":false,"error":{"code":"REQUEST_RESULT_EXPIRED","message":"REQUEST_RESULT_EXPIRED: request was already handled; refresh project/history instead of resubmitting"}})
}
fn web_context<T>(state: &Shared, run: impl FnOnce() -> T) -> T {
    project::with_context(
        json!({"id":"cutcraft.web","kind":"web","version":env!("CARGO_PKG_VERSION")}),
        state.actor.clone(),
        run,
    )
}
async fn mutation(state: Shared, entry: Arc<Project>, body: Value, mode: &str) -> Result<Value> {
    let request_id = body["requestId"]
        .as_str()
        .context("REQUEST_ID_REQUIRED")?
        .to_owned();
    let fingerprint = json!({"project":entry.handle,"mode":mode,"body":body});
    let run_state = state.clone();
    let mode = mode.to_owned();
    deduplicate(&state, &request_id, &fingerprint, async move {
        let state = run_state;
        let _gate = entry.gate.lock().await;
        ensure!(
            !entry.closed.load(Ordering::SeqCst),
            "PROJECT_CLOSED: project was closed"
        );
        let path = entry.path.clone();
        let state2 = state.clone();
        let value = tokio::task::spawn_blocking(move || {
            web_context(&state2, || -> Result<Value> {
                let store = Store::open(&path)?;
                let project = store.project()?;
                ensure!(
                    body["baseRevision"] == project["revision"],
                    "REVISION_CONFLICT: expected {}, received {}",
                    project["revision"],
                    body["baseRevision"]
                );
                match mode.as_str() {
                    "undo" => store.history_undo(false, false),
                    "redo" => store.history_redo(false, false),
                    "operation" => store.apply(&body["operation"], false, false),
                    _ => {
                        let operation = edit::operation(&project, body["command"].clone());
                        store.apply(&operation, false, false)
                    }
                }
            })
        })
        .await??;
        if value["project"].is_object() {
            publish_snapshot(&state, &entry, &value["project"]);
        }
        Ok(value)
    })
    .await
}
async fn command(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    api_value(
        mutation(
            state.clone(),
            get_project(&state, &handle)?,
            body,
            "command",
        )
        .await?,
    )
}
async fn operation(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
    Json(mut body): Json<Value>,
) -> ApiResult {
    if body["baseRevision"].is_null() {
        body["baseRevision"] = body["operation"]["baseRevision"].clone();
    }
    api_value(
        mutation(
            state.clone(),
            get_project(&state, &handle)?,
            body,
            "operation",
        )
        .await?,
    )
}
async fn history_action(
    State(state): State<Shared>,
    RoutePath((handle, action)): RoutePath<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResult {
    ensure_api(
        matches!(action.as_str(), "undo" | "redo"),
        "unknown history action",
    )?;
    api_value(mutation(state.clone(), get_project(&state, &handle)?, body, &action).await?)
}

fn command_kind(cli: &app::Cli) -> &'static str {
    match &cli.command {
        app::Cmd::Render { .. } | app::Cmd::Preview { .. } => "render",
        app::Cmd::Import { .. } | app::Cmd::ImportSequence { .. } => "import",
        _ => "command",
    }
}
fn execution_context<T>(state: &Shared, web: bool, run: impl FnOnce() -> T) -> T {
    if web {
        web_context(state, run)
    } else {
        let mut actor = state.actor.clone();
        actor["displayName"] = json!("Cutcraft CLI");
        project::with_context(
            json!({"id":"cutcraft.cli","kind":"cli","version":env!("CARGO_PKG_VERSION")}),
            actor,
            run,
        )
    }
}
async fn run_cli(
    state: Shared,
    entry: Arc<Project>,
    cli: app::Cli,
    web: bool,
    task: Option<Arc<Task>>,
) -> Result<Value> {
    let long = command_kind(&cli) != "command";
    let _gate = if long {
        None
    } else {
        Some(entry.gate.lock().await)
    };
    ensure!(
        !entry.closed.load(Ordering::SeqCst),
        "PROJECT_CLOSED: project was closed"
    );
    let state2 = state.clone();
    let task2 = task.clone();
    let result = tokio::task::spawn_blocking(move || {
        let progress = if let Some(task) = task2 {
            let state3 = state2.clone();
            let task3 = task.clone();
            crate::progress::Progress::with_callback(
                task.cancellation.clone(),
                Arc::new(move |progress| {
                    task3.value.lock().unwrap()["progress"] = progress;
                    publish_task(&state3, &task3);
                }),
            )
        } else {
            crate::progress::Progress::default()
        };
        execution_context(&state2, web, || app::execute_with_progress(cli, &progress))
    })
    .await?;
    let path = entry.path.clone();
    if let Ok(Ok(snapshot)) =
        tokio::task::spawn_blocking(move || Store::open(&path)?.project()).await
    {
        publish_snapshot(&state, &entry, &snapshot);
    }
    result
}
fn publish_task(state: &Shared, task: &Task) {
    let _ = state
        .events
        .send(json!({"type":"task.changed","task":*task.value.lock().unwrap()}));
}
fn create_task(
    state: &Shared,
    entry: &Project,
    kind: &str,
    directory: Option<PathBuf>,
) -> Result<(String, Arc<Task>)> {
    prune_tasks(state);
    ensure!(
        state
            .tasks
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.completed.lock().unwrap().is_none())
            .count()
            < 64,
        "TASK_CAPACITY: wait for active tasks to finish"
    );
    let id = crate::id("task");
    let (finished, _) = watch::channel(false);
    let task = Arc::new(Task {
        value: Mutex::new(
            json!({"id":id,"project":entry.handle,"kind":kind,"status":"queued","submittedRevision":entry.cached.lock().unwrap()["revision"]}),
        ),
        cancellation: crate::progress::CancellationToken::default(),
        directory,
        finished,
        completed: Mutex::new(None),
    });
    state.tasks.lock().unwrap().insert(id.clone(), task.clone());
    publish_task(state, &task);
    Ok((id, task))
}
fn prune_tasks(state: &Shared) {
    let mut tasks = state.tasks.lock().unwrap();
    let mut completed: Vec<_> = tasks
        .iter()
        .filter_map(|(id, task)| {
            task.completed
                .lock()
                .unwrap()
                .map(|when| (id.clone(), when))
        })
        .collect();
    completed.sort_by_key(|(_, when)| *when);
    let excess = completed.len().saturating_sub(256);
    let mut bytes: u64 = tasks
        .values()
        .filter_map(|task| task.directory.as_deref())
        .map(preview_bytes)
        .sum();
    for (index, (id, when)) in completed.into_iter().enumerate() {
        if index < excess || when.elapsed() > Duration::from_secs(1800) || bytes > 512 * 1024 * 1024
        {
            if let Some(task) = tasks.remove(&id) {
                if let Some(directory) = &task.directory {
                    bytes = bytes.saturating_sub(preview_bytes(directory));
                    let _ = fs::remove_dir_all(directory);
                }
            }
        }
    }
}
fn preview_bytes(directory: &Path) -> u64 {
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}
fn cancel_preview(task: &Task) {
    task.cancellation.cancel();
    if task.completed.lock().unwrap().is_some() {
        if let Some(directory) = &task.directory {
            let _ = fs::remove_dir_all(directory);
        }
    }
}
fn check_preview_budget(task: &Task) {
    let Some(directory) = &task.directory else {
        return;
    };
    let bytes = preview_bytes(directory);
    if bytes > 128 * 1024 * 1024 {
        task.value.lock().unwrap()["cancelReason"] = json!("PREVIEW_CACHE_LIMIT");
        task.cancellation.cancel();
    }
}
fn complete_task(state: &Shared, task: &Task, result: Result<Value>) {
    {
        let mut value = task.value.lock().unwrap();
        match result {
            Ok(result) => {
                value["status"] = json!("completed");
                if result["revision"].is_u64() {
                    value["revision"] = result["revision"].clone();
                }
                value["result"] = result;
            }
            Err(error) => {
                let detail = error_value(&error);
                value["status"] = json!(if detail["error"]["code"] == "JOB_CANCELLED" {
                    "cancelled"
                } else {
                    "failed"
                });
                value["error"] = detail["error"].clone();
            }
        }
    }
    publish_task(state, task);
    *task.completed.lock().unwrap() = Some(Instant::now());
    if task.cancellation.is_cancelled() {
        if let Some(directory) = &task.directory {
            let _ = fs::remove_dir_all(directory);
        }
    }
    task.finished.send_replace(true);
}
async fn start_cli_task(
    state: Shared,
    entry: Arc<Project>,
    cli: app::Cli,
    web: bool,
) -> Result<(String, Arc<Task>)> {
    let gate = entry.gate.lock().await;
    ensure!(
        !entry.closed.load(Ordering::SeqCst),
        "PROJECT_CLOSED: project was closed"
    );
    let (id, task) = create_task(&state, &entry, command_kind(&cli), None)?;
    entry.busy.fetch_add(1, Ordering::SeqCst);
    drop(gate);
    let task2 = task.clone();
    tokio::spawn(async move {
        task2.value.lock().unwrap()["status"] = json!("running");
        publish_task(&state, &task2);
        let result = run_cli(state.clone(), entry.clone(), cli, web, Some(task2.clone())).await;
        entry.busy.fetch_sub(1, Ordering::SeqCst);
        complete_task(&state, &task2, result);
    });
    Ok((id, task))
}
async fn wait_task(task: Arc<Task>) -> Result<Value> {
    let mut receiver = task.finished.subscribe();
    while !*receiver.borrow() {
        receiver.changed().await?;
    }
    let value = task.value.lock().unwrap();
    if value["status"] == "completed" {
        Ok(value["result"].clone())
    } else {
        Err(RemoteError {
            detail: value["error"].clone(),
        }
        .into())
    }
}
async fn execute(State(state): State<Shared>, Json(body): Json<Value>) -> ApiResult {
    ensure_api(
        body["instanceId"] == state.runtime.instance_id,
        "INSTANCE_CHANGED: request outcome unknown; do not retry mutations automatically",
    )?;
    let id = body["requestId"]
        .as_str()
        .context("REQUEST_ID_REQUIRED")?
        .to_owned();
    let cli: app::Cli = serde_json::from_value(body["cli"].clone())?;
    validate_remote_command(&cli)?;
    let path = cli.project.canonicalize()?;
    let entry = state
        .registry
        .lock()
        .unwrap()
        .projects
        .values()
        .find(|p| p.path == path)
        .cloned()
        .context("PROJECT_NOT_OPEN: project is not registered")?;
    let run_state = state.clone();
    let asynchronous = body["async"] == true;
    let result = deduplicate(&state, &id, &body, async move {
        let state = run_state;
        if command_kind(&cli) == "command" {
            run_cli(state.clone(), entry, cli, false, None).await
        } else {
            let (id, task) = start_cli_task(state.clone(), entry, cli, false).await?;
            if asynchronous {
                Ok(json!({"ok":true,"taskId":id}))
            } else {
                wait_task(task).await
            }
        }
    })
    .await?;
    api_value(result)
}
async fn project_request(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    let entry = get_project(&state, &handle)?;
    let id = body["requestId"]
        .as_str()
        .context("REQUEST_ID_REQUIRED")?
        .to_owned();
    let cli: app::Cli = serde_json::from_value(
        json!({"project":entry.path,"override_locks":false,"dry_run":false,"command":body["command"],"inputs":body.get("inputs").cloned().unwrap_or_else(||json!({}))}),
    )?;
    validate_remote_command(&cli)?;
    let fingerprint = json!({"project":handle,"body":body});
    let run_state = state.clone();
    let result = deduplicate(&state, &id, &fingerprint, async move {
        let state = run_state;
        if command_kind(&cli) == "command" {
            run_cli(state.clone(), entry, cli, true, None).await
        } else {
            let (id, _) = start_cli_task(state.clone(), entry, cli, true).await?;
            Ok(json!({"ok":true,"taskId":id}))
        }
    })
    .await?;
    api_value(result)
}
fn validate_remote_command(cli: &app::Cli) -> Result<()> {
    ensure!(
        !matches!(
            cli.command,
            app::Cmd::Open { .. }
                | app::Cmd::Close { .. }
                | app::Cmd::Serve { .. }
                | app::Cmd::Init { .. }
        ),
        "INVALID_REQUEST: lifecycle commands use dedicated endpoints"
    );
    fn needs_stdin(value: &Value) -> bool {
        match value {
            Value::Object(fields) => fields.iter().any(|(key, value)| {
                (matches!(key.as_str(), "file" | "files") && value == "-") || needs_stdin(value)
            }),
            Value::Array(values) => values.iter().any(needs_stdin),
            _ => false,
        }
    }
    ensure!(
        !needs_stdin(&serde_json::to_value(&cli.command)?)
            || cli.inputs.contains_key(Path::new("-")),
        "INVALID_REQUEST: stdin must be materialized by the client"
    );
    Ok(())
}
async fn tasks(State(state): State<Shared>) -> Json<Value> {
    let values: Vec<_> = state
        .tasks
        .lock()
        .unwrap()
        .values()
        .map(|t| t.value.lock().unwrap().clone())
        .collect();
    Json(json!({"tasks":values}))
}
fn get_task(state: &Shared, id: &str) -> Result<Arc<Task>> {
    state
        .tasks
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .context("TASK_NOT_FOUND")
}
async fn task(State(state): State<Shared>, RoutePath(id): RoutePath<String>) -> ApiResult {
    Ok(Json(
        json!({"task":*get_task(&state,&id)?.value.lock().unwrap()}),
    ))
}
async fn cancel_task(State(state): State<Shared>, RoutePath(id): RoutePath<String>) -> ApiResult {
    let task = get_task(&state, &id)?;
    cancel_preview(&task);
    Ok(Json(json!({"ok":true,"task":*task.value.lock().unwrap()})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrameQuery {
    scale: Option<f64>,
    sequence: Option<String>,
    at: Option<String>,
    revision: Option<u64>,
    generation: Option<u64>,
    client_id: Option<String>,
}
async fn preview_frame(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
    Query(query): Query<FrameQuery>,
) -> std::result::Result<Response, ApiError> {
    render_frame(state, handle, query, None).await
}
fn preview_scale(body: &Value) -> Result<Option<f64>> {
    let scale = body
        .get("scale")
        .map(|value| {
            value
                .as_f64()
                .context("INVALID_PREVIEW_SCALE: expected a number")
        })
        .transpose()?;
    crate::preview::validate_scale(scale)?;
    Ok(scale)
}
async fn preview_draft(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
    Json(body): Json<Value>,
) -> std::result::Result<Response, ApiError> {
    let at = crate::time::Time::from_json(&body["at"])?;
    let query = FrameQuery {
        scale: preview_scale(&body)?,
        sequence: body["sequence"].as_str().map(str::to_owned),
        at: Some(at.to_string()),
        revision: body["revision"].as_u64(),
        generation: body["generation"].as_u64(),
        client_id: body["clientId"].as_str().map(str::to_owned),
    };
    ensure_api(body["command"].is_object(), "draft command is required")?;
    render_frame(state, handle, query, Some(body["command"].clone())).await
}
fn accept_generation(entry: &Project, client: &str, generation: u64) -> Result<()> {
    ensure!(client.len() <= 256, "INVALID_CLIENT_ID");
    let mut generations = entry.generations.lock().unwrap();
    if let Some(previous) = generations.get(client) {
        ensure!(
            generation >= *previous,
            "STALE_PREVIEW: a newer generation was already requested"
        );
    }
    ensure!(
        generations.contains_key(client) || generations.len() < 4096,
        "CLIENT_CAPACITY: reopen project to start new preview sessions"
    );
    generations.insert(client.into(), generation);
    Ok(())
}
async fn media_slot(
    state: &Shared,
    entry: &Project,
    task: &Task,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    loop {
        if let Err(error) = task.cancellation.check() {
            complete_task(state, task, Err(anyhow::anyhow!("{error:#}")));
            return Err(error);
        }
        if let Ok(permit) = entry.media_slots.clone().try_acquire_owned() {
            return Ok(permit);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn run_preview<T: Send + 'static>(
    state: Shared,
    entry: Arc<Project>,
    task: Arc<Task>,
    work: impl FnOnce(crate::progress::CancellationToken) -> Result<T> + Send + 'static,
    describe: impl FnOnce(&T) -> Value + Send + 'static,
) -> Result<T> {
    tokio::spawn(async move {
        let _permit = media_slot(&state, &entry, &task).await?;
        task.value.lock().unwrap()["status"] = json!("running");
        let cancellation = task.cancellation.clone();
        let result = tokio::task::spawn_blocking(move || work(cancellation))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|v| v);
        match result {
            Ok(value) => {
                complete_task(&state, &task, Ok(describe(&value)));
                Ok(value)
            }
            Err(error) => {
                complete_task(
                    &state,
                    &task,
                    Err(RemoteError {
                        detail: error_value(&error)["error"].clone(),
                    }
                    .into()),
                );
                Err(error)
            }
        }
    })
    .await?
}
async fn render_frame(
    state: Shared,
    handle: String,
    query: FrameQuery,
    draft: Option<Value>,
) -> std::result::Result<Response, ApiError> {
    crate::preview::validate_scale(query.scale)?;
    let entry = get_project(&state, &handle)?;
    let gate = entry.gate.lock().await;
    ensure_api(!entry.closed.load(Ordering::SeqCst), "PROJECT_CLOSED")?;
    let path = entry.path.clone();
    let mut snapshot = tokio::task::spawn_blocking(move || open_store(&path)?.project()).await??;
    ensure_api(
        query
            .revision
            .is_none_or(|revision| snapshot["revision"] == revision),
        "REVISION_CONFLICT: preview revision is stale",
    )?;
    let client_id = query.client_id.unwrap_or_else(|| "default".into());
    accept_generation(&entry, &client_id, query.generation.unwrap_or(0))?;
    let revision = snapshot["revision"].to_string();
    if let Some(command) = draft {
        let operation = edit::operation(&snapshot, command);
        snapshot = edit::apply(&snapshot, &operation, false)?.0;
    }
    let sequence = query
        .sequence
        .unwrap_or_else(|| snapshot["rootSequenceId"].as_str().unwrap().to_owned());
    let at: crate::time::Time = query.at.unwrap_or_else(|| "0".into()).parse()?;
    for task in state.tasks.lock().unwrap().values() {
        let value = task.value.lock().unwrap();
        if value["project"] == handle
            && value["previewType"] == "frame"
            && value["clientId"] == client_id
        {
            cancel_preview(task);
        }
    }
    let (_, task) = create_task(&state, &entry, "preview", None)?;
    task.value.lock().unwrap()["previewType"] = json!("frame");
    task.value.lock().unwrap()["revision"] = snapshot["revision"].clone();
    task.value.lock().unwrap()["clientId"] = json!(client_id);
    drop(gate);
    let path = entry.path.clone();
    let frame = run_preview(
        state,
        entry,
        task,
        move |cancellation| {
            crate::preview::frame_scaled(
                &snapshot,
                &path,
                &sequence,
                &at,
                &crate::progress::Progress::with_cancellation(cancellation),
                query.scale,
            )
        },
        |frame| frame.metadata.clone(),
    )
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (
                axum::http::HeaderName::from_static("x-cutcraft-revision"),
                &revision,
            ),
            (
                axum::http::HeaderName::from_static("x-cutcraft-generation"),
                &query.generation.unwrap_or(0).to_string(),
            ),
        ],
        frame.bytes,
    )
        .into_response())
}
async fn preview_playback(
    State(state): State<Shared>,
    RoutePath(handle): RoutePath<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    let scale = preview_scale(&body)?;
    let entry = get_project(&state, &handle)?;
    let gate = entry.gate.lock().await;
    ensure_api(!entry.closed.load(Ordering::SeqCst), "PROJECT_CLOSED")?;
    let path = entry.path.clone();
    let snapshot = tokio::task::spawn_blocking(move || Store::open(&path)?.project()).await??;
    ensure_api(
        body["revision"].is_null() || body["revision"] == snapshot["revision"],
        "REVISION_CONFLICT: preview revision is stale",
    )?;
    let client_id = body["clientId"].as_str().unwrap_or("default");
    accept_generation(&entry, client_id, body["generation"].as_u64().unwrap_or(0))?;
    let sequence = body["sequence"]
        .as_str()
        .or_else(|| snapshot["rootSequenceId"].as_str())
        .context("missing sequence")?
        .to_owned();
    let start = if body["start"].is_null() {
        crate::time::Time::zero()
    } else {
        crate::time::Time::from_json(&body["start"])?
    };
    for task in state.tasks.lock().unwrap().values() {
        let value = task.value.lock().unwrap();
        if value["project"] == handle
            && value["previewType"] == "playback"
            && value["clientId"] == client_id
        {
            cancel_preview(task);
        }
    }
    let directory = entry
        .path
        .join("cache")
        .join("daemon-preview")
        .join(&state.runtime.instance_id)
        .join(secret());
    fs::create_dir_all(&directory)?;
    let (id, task) = create_task(&state, &entry, "preview", Some(directory.clone()))?;
    task.value.lock().unwrap()["previewType"] = json!("playback");
    task.value.lock().unwrap()["revision"] = snapshot["revision"].clone();
    task.value.lock().unwrap()["clientId"] = json!(client_id);
    drop(gate);
    tokio::spawn(async move {
        let _permit = match media_slot(&state, &entry, &task).await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        task.value.lock().unwrap()["status"] = json!("running");
        publish_task(&state, &task);
        let state2 = state.clone();
        let task2 = task.clone();
        let path = entry.path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let progress = crate::progress::Progress::with_callback(
                task2.cancellation.clone(),
                Arc::new(move |progress| {
                    task2.value.lock().unwrap()["progress"] = progress;
                    check_preview_budget(&task2);
                    publish_task(&state2, &task2);
                }),
            );
            crate::preview::playback_scaled(
                &snapshot, &path, &sequence, &start, &directory, &progress, scale,
            )
        })
        .await
        .map_err(anyhow::Error::from)
        .and_then(|v| v);
        complete_task(&state, &task, result);
    });
    Ok(Json(
        json!({"ok":true,"taskId":id,"generation":body["generation"]}),
    ))
}
async fn task_file(
    State(state): State<Shared>,
    RoutePath((id, file)): RoutePath<(String, String)>,
) -> std::result::Result<Response, ApiError> {
    let task = get_task(&state, &id)?;
    let directory = task
        .directory
        .as_ref()
        .context("task has no preview files")?;
    let segment = file
        .strip_prefix("part-")
        .and_then(|v| v.strip_suffix(".m4s"))
        .is_some_and(|v| v.len() == 6 && v.bytes().all(|b| b.is_ascii_digit()));
    ensure_api(
        matches!(file.as_str(), "init.mp4" | "stream.m3u8" | "metadata.json") || segment,
        "invalid preview file",
    )?;
    let bytes = match fs::read(directory.join(&file)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StatusCode::NOT_FOUND.into_response())
        }
        Err(error) => return Err(error.into()),
    };
    let mime = if file.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if file.ends_with(".json") {
        "application/json"
    } else {
        "video/mp4"
    };
    Ok(([(header::CONTENT_TYPE, mime)], bytes).into_response())
}
#[derive(Deserialize)]
struct AssetQuery {
    stream: String,
}
async fn asset_input(state: &Shared, handle: &str) -> Result<(Arc<Project>, Value, Arc<Task>)> {
    let entry = get_project(state, handle)?;
    let gate = entry.gate.lock().await;
    ensure!(!entry.closed.load(Ordering::SeqCst), "PROJECT_CLOSED");
    let path = entry.path.clone();
    let snapshot = tokio::task::spawn_blocking(move || Store::open(&path)?.project()).await??;
    let (_, task) = create_task(state, &entry, "preview", None)?;
    drop(gate);
    Ok((entry, snapshot, task))
}
async fn thumbnail(
    State(state): State<Shared>,
    RoutePath((handle, asset)): RoutePath<(String, String)>,
    Query(query): Query<AssetQuery>,
) -> std::result::Result<Response, ApiError> {
    let (entry, snapshot, task) = asset_input(&state, &handle).await?;
    let path = entry.path.clone();
    let frame = run_preview(
        state,
        entry,
        task,
        move |cancellation| {
            crate::preview::thumbnail(
                &snapshot,
                &path,
                &asset,
                &query.stream,
                &crate::progress::Progress::with_cancellation(cancellation),
            )
        },
        |frame| frame.metadata.clone(),
    )
    .await?;
    Ok(([(header::CONTENT_TYPE, "image/png")], frame.bytes).into_response())
}
async fn waveform(
    State(state): State<Shared>,
    RoutePath((handle, asset)): RoutePath<(String, String)>,
    Query(query): Query<AssetQuery>,
) -> ApiResult {
    let (entry, snapshot, task) = asset_input(&state, &handle).await?;
    let path = entry.path.clone();
    let value = run_preview(
        state,
        entry,
        task,
        move |cancellation| {
            crate::preview::waveform(
                &snapshot,
                &path,
                &asset,
                &query.stream,
                &crate::progress::Progress::with_cancellation(cancellation),
            )
        },
        Clone::clone,
    )
    .await?;
    Ok(Json(value))
}
async fn events(State(state): State<Shared>) -> impl IntoResponse {
    let changes = BroadcastStream::new(state.events.subscribe())
        .map(|event| event.unwrap_or_else(|_| json!({"type":"resync"})));
    let stopping = WatchStream::new(state.shutdown.subscribe())
        .filter(|closing| *closing)
        .map(|_| json!({"type":"server.closing"}));
    let stream = changes
        .merge(stopping)
        .take_while(|event| event["type"] != "server.closing")
        .map(|event| {
            Ok::<_, std::convert::Infallible>(
                Event::default().event("change").data(event.to_string()),
            )
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
async fn asset(request: Request<Body>) -> Response {
    let path = request.uri().path();
    if let Some((mime, bytes)) = crate::web::asset(path) {
        ([(header::CONTENT_TYPE, mime)], bytes).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

#[cfg(test)]
mod bind_tests {
    use super::*;
    #[test]
    fn auth_is_decided_by_binding_not_client_address() {
        for (address, required) in [
            ("localhost", false),
            ("127.0.0.2", false),
            ("::1", false),
            ("::ffff:127.0.0.1", false),
            ("0.0.0.0", true),
            ("::", true),
            ("192.168.1.7", true),
        ] {
            let runtime = Runtime {
                instance_id: String::new(),
                token: String::new(),
                port: 12345,
                pid: 0,
                bind_host: parse_host(address).unwrap(),
            };
            assert_eq!(runtime.auth_required(), required, "{address}");
            assert!(runtime.accepts_authority(&runtime.url()[7..]));
            assert!(!runtime.accepts_authority("evil.example:12345"));
            assert!(!runtime.accepts_authority("127.0.0.1:12346"));
        }
        assert!(parse_host("untrusted.example").is_err());
    }
}
