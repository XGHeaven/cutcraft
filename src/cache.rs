//! Project-local disposable data. Keys and validity belong to the producer;
//! leases, publication, accounting and eviction belong here.
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

pub const TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
pub const SOURCE_MEMORY_BYTES: usize = 256 * 1024 * 1024;
pub const SOURCE_WINDOWS: usize = 4;
pub const IMAGE_MEMORY_BYTES: usize = 64 * 1024 * 1024;
pub const PLAYBACK_SESSION_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Frames,
    Text,
    Decode,
    Ranges,
    Sheets,
    FullPreview,
    Playback,
    Staging,
}
impl Kind {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Frames => "interactive-v1",
            Self::Text => "interactive-text-v2",
            Self::Decode => "decode-windows-v1",
            Self::Ranges => "preview-ranges-v1",
            Self::Sheets => "preview-sheets-v1",
            Self::FullPreview => "",
            Self::Playback => "daemon-preview",
            Self::Staging => "staging-v1",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Frames => "frames",
            Self::Text => "text",
            Self::Decode => "decode",
            Self::Ranges => "ranges",
            Self::Sheets => "sheets",
            Self::FullPreview => "fullPreview",
            Self::Playback => "playback",
            Self::Staging => "staging",
        }
    }
    fn limits(self) -> (u64, usize) {
        match self {
            Self::Frames | Self::Text | Self::Sheets => (64 * 1024 * 1024, 256),
            Self::Decode | Self::Playback => (512 * 1024 * 1024, usize::MAX),
            _ => (TOTAL_BYTES, usize::MAX),
        }
    }
}
const KINDS: [Kind; 8] = [
    Kind::Frames,
    Kind::Text,
    Kind::Decode,
    Kind::Ranges,
    Kind::Sheets,
    Kind::FullPreview,
    Kind::Playback,
    Kind::Staging,
];

pub fn capabilities() -> Value {
    let categories: BTreeMap<_, _> = KINDS.iter().map(|kind| {
        let (bytes, entries) = kind.limits();
        (kind.name(), json!({"limitBytes":bytes,"limitEntries":(entries != usize::MAX).then_some(entries)}))
    }).collect();
    json!({"commands":["cache status","cache clean","cache clean --dry-run"],"limitBytes":TOTAL_BYTES,
        "eviction":"least-recently-used unleased entries","crossProcessLeases":true,
        "protected":"current asset references, unknown files and symlinks",
        "playbackSessionLimitBytes":PLAYBACK_SESSION_BYTES,"categories":categories})
}

// Explicit unlock also handles a descriptor inherited briefly by a child before exec.
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
#[derive(Clone)]
pub struct Manager {
    dir: PathBuf,
    root: PathBuf,
}
pub struct Entry {
    manager: Manager,
    path: PathBuf,
    _lease: Lock,
}
pub struct Scratch {
    directory: tempfile::TempDir,
    _entry: Entry,
}
impl Scratch {
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}
struct Record {
    path: PathBuf,
    kind: Kind,
    files: Vec<PathBuf>,
    bytes: u64,
    accessed: SystemTime,
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn classify(relative: &Path) -> Option<(Kind, PathBuf)> {
    let parts: Vec<_> = relative.iter().map(|s| s.to_str()).collect::<Option<_>>()?;
    if parts.len() == 1 && parts[0].starts_with("preview-") && parts[0].contains("-640-24-v1-") {
        return Some((Kind::FullPreview, relative.to_owned()));
    }
    if parts.len() == 3 && parts[0] == "daemon-preview" && hex(parts[1], 64) && hex(parts[2], 64) {
        return Some((Kind::Playback, relative.to_owned()));
    }
    if parts.len() != 2 {
        return None;
    }
    let name = Path::new(parts[1]);
    let stem = name.file_stem()?.to_str()?;
    let ext = name.extension().and_then(|e| e.to_str()).unwrap_or("");
    let kind = match parts[0] {
        "interactive-v1"
            if (ext == "png" && hex(stem.strip_prefix("thumbnail-").unwrap_or(stem), 64))
                || (ext == "json"
                    && stem.strip_prefix("waveform-").is_some_and(|s| hex(s, 64))) =>
        {
            Kind::Frames
        }
        "interactive-text-v2" if ext == "rgba" && hex(stem, 64) => Kind::Text,
        "decode-windows-v1" if matches!(ext, "raw" | "pts" | "ready") && hex(stem, 64) => {
            return Some((Kind::Decode, relative.with_extension("raw")))
        }
        "preview-ranges-v1" if ext == "mp4" && hex(stem, 64) => Kind::Ranges,
        "preview-sheets-v1" if ext == "png" && hex(stem, 64) => Kind::Sheets,
        "staging-v1" if parts[1].starts_with("work-") => Kind::Staging,
        _ => return None,
    };
    Some((kind, relative.to_owned()))
}

impl Manager {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            root: dir.join("cache"),
        }
    }
    fn safe(&self, path: &Path) -> Result<()> {
        let relative = path
            .strip_prefix(&self.root)
            .context("CACHE_PATH_OUTSIDE_ROOT")?;
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
            "CACHE_PATH_INVALID"
        );
        let mut current = self.root.clone();
        for part in std::iter::once(None).chain(relative.iter().map(Some)) {
            if let Some(part) = part {
                current.push(part);
            }
            match fs::symlink_metadata(&current) {
                Ok(m) => ensure!(
                    !m.file_type().is_symlink(),
                    "CACHE_SYMLINK: {}",
                    current.display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    pub fn directory(&self, kind: Kind) -> Result<PathBuf> {
        let path = self.root.join(kind.directory());
        self.safe(&path)?;
        fs::create_dir_all(&path)?;
        Ok(path)
    }
    fn lock_file(&self, path: &Path) -> Result<File> {
        self.safe(path)?;
        fs::create_dir_all(path.parent().context("cache lock parent")?)?;
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?)
    }
    fn global(&self) -> Result<Lock> {
        let file = self.lock_file(&self.root.join(".manager/lock"))?;
        file.lock_exclusive()?;
        Ok(Lock(file))
    }
    fn lock_path(&self, path: &Path, extension: &str) -> Result<PathBuf> {
        let key = crate::store::hash(&json!(path.strip_prefix(&self.root)?.to_string_lossy()))?;
        Ok(self
            .root
            .join(".manager")
            .join(format!("{key}.{extension}")))
    }
    fn acquire(&self, path: &Path) -> Result<Entry> {
        self.safe(path)?;
        let file = self.lock_file(&self.lock_path(path, "lease")?)?;
        FileExt::lock_shared(&file)?;
        file.set_modified(SystemTime::now())?;
        Ok(Entry {
            manager: self.clone(),
            path: path.to_owned(),
            _lease: Lock(file),
        })
    }
    pub fn entry(&self, path: &Path) -> Result<Entry> {
        let (_, relative) =
            classify(path.strip_prefix(&self.root)?).context("CACHE_ENTRY_UNKNOWN")?;
        let _lock = self.global()?;
        let path = self.root.join(relative);
        let entry = self.acquire(&path)?;
        fs::create_dir_all(path.parent().context("cache parent")?)?;
        Ok(entry)
    }
    pub fn scratch(&self) -> Result<Scratch> {
        let _lock = self.global()?;
        let dir = self.directory(Kind::Staging)?;
        let directory = tempfile::Builder::new().prefix("work-").tempdir_in(dir)?;
        let entry = self.acquire(directory.path())?;
        Ok(Scratch {
            directory,
            _entry: entry,
        })
    }
    fn scan(&self) -> Result<Vec<Record>> {
        self.safe(&self.root)?;
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut candidates = BTreeSet::new();
        for top in children(&self.root)? {
            let Some(ty) = file_type(&top)? else {
                continue;
            };
            if !ty.is_dir() {
                continue;
            }
            let path = top.path();
            if classify(path.strip_prefix(&self.root)?).is_some() {
                candidates.insert(path);
                continue;
            }
            if !KINDS.iter().any(|kind| top.file_name() == kind.directory()) {
                continue;
            }
            for child in children(&path)? {
                let Some(ty) = file_type(&child)? else {
                    continue;
                };
                if ty.is_symlink() {
                    continue;
                }
                let path = child.path();
                if top.file_name() == "daemon-preview" && ty.is_dir() {
                    for session in children(&path)? {
                        let Some(ty) = file_type(&session)? else {
                            continue;
                        };
                        if ty.is_dir() {
                            candidates.insert(session.path());
                        }
                    }
                } else {
                    candidates.insert(path);
                }
            }
        }
        let mut records = BTreeMap::new();
        for candidate in candidates {
            let Some((kind, relative)) = classify(candidate.strip_prefix(&self.root)?) else {
                continue;
            };
            let path = self.root.join(relative);
            if records.contains_key(&path) {
                continue;
            }
            let files = if kind == Kind::Decode {
                ["raw", "pts", "ready"]
                    .iter()
                    .map(|ext| path.with_extension(ext))
                    .filter(|p| fs::symlink_metadata(p).is_ok())
                    .collect()
            } else if path.is_dir() {
                let Some(files) = tree_files(&path, kind)? else {
                    continue;
                };
                files
            } else {
                vec![path.clone()]
            };
            if files
                .iter()
                .any(|p| fs::symlink_metadata(p).map_or(true, |m| !m.is_file()))
            {
                continue;
            }
            let bytes = files
                .iter()
                .map(|p| {
                    fs::metadata(p).map(|m| m.len()).or_else(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(0)
                        } else {
                            Err(e)
                        }
                    })
                })
                .collect::<std::io::Result<Vec<_>>>()?
                .into_iter()
                .sum();
            let accessed = fs::metadata(self.lock_path(&path, "lease")?)
                .and_then(|m| m.modified())
                .or_else(|_| fs::metadata(&candidate).and_then(|m| m.modified()))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            records.insert(
                path.clone(),
                Record {
                    path,
                    kind,
                    files,
                    bytes,
                    accessed,
                },
            );
        }
        Ok(records.into_values().collect())
    }
    /// Automatic eviction only uses a fresh, locked snapshot. If an editor holds
    /// the project lock, defer eviction instead of deleting from a stale snapshot.
    pub fn trim(&self) -> Result<()> {
        // Library callers may render an in-memory project without a Store.
        if !self.dir.join("cutcraft.json").is_file() {
            return Ok(());
        }
        let store = match crate::store::Store::open(&self.dir) {
            Ok(store) => store,
            Err(e) if e.to_string().contains("PROJECT_LOCKED") => return Ok(()),
            Err(e) => return Err(e),
        };
        self.manage(&store.project()?, Mode::Trim, false, TOTAL_BYTES)?;
        Ok(())
    }
    pub fn status(&self, project: &Value) -> Result<Value> {
        self.manage(project, Mode::Status, false, TOTAL_BYTES)
    }
    pub fn clean(&self, project: &Value, dry_run: bool) -> Result<Value> {
        self.manage(project, Mode::Clean, dry_run, TOTAL_BYTES)
    }
    /// Remove a completed session through the same protection checks as clean.
    pub fn discard(&self, path: &Path) -> Result<()> {
        let store = match crate::store::Store::open(&self.dir) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        self.manage(&store.project()?, Mode::One(path), false, TOTAL_BYTES)?;
        Ok(())
    }
    fn manage(&self, project: &Value, mode: Mode<'_>, dry_run: bool, budget: u64) -> Result<Value> {
        let _lock = if self.root.exists() {
            Some(self.global()?)
        } else {
            None
        };
        let mut records = self.scan()?;
        records.sort_by_key(|r| r.accessed);
        let mut protected = Vec::new();
        if let Some(assets) = project["assets"].as_object() {
            for asset in assets.values() {
                if let Ok(path) =
                    crate::media::local_path(&self.dir, crate::project::text(&asset["uri"]))
                {
                    // Canonicalization also protects assets that reach cache through an alias.
                    protected.push(path.canonicalize().unwrap_or(path));
                }
            }
        }
        let root = if self.root.exists() {
            self.root.canonicalize()?
        } else {
            self.root.clone()
        };
        let mut totals = BTreeMap::<Kind, (u64, usize)>::new();
        for r in &records {
            let n = totals.entry(r.kind).or_default();
            n.0 += r.bytes;
            n.1 += 1;
        }
        let mut total: u64 = records.iter().map(|r| r.bytes).sum();
        let before = total;
        let mut removed = (0u64, 0usize);
        let mut busy = 0;
        let mut protected_bytes = 0;
        let mut reclaimable = 0;
        let mut categories = serde_json::Map::new();
        for kind in KINDS {
            let (bytes, entries) = totals.get(&kind).copied().unwrap_or_default();
            let (limit, count) = kind.limits();
            categories.insert(kind.name().into(), json!({"bytes":bytes,"entries":entries,"busyEntries":0,"protectedEntries":0,"limitBytes":limit,"limitEntries":(count != usize::MAX).then_some(count)}));
        }
        for r in &records {
            let canonical = root.join(r.path.strip_prefix(&self.root)?);
            let is_protected = protected
                .iter()
                .any(|p| p.starts_with(&canonical) || canonical.starts_with(p))
                || r.files.iter().any(|f| {
                    protected
                        .iter()
                        .any(|p| root.join(f.strip_prefix(&self.root).unwrap()) == *p)
                });
            if is_protected {
                protected_bytes += r.bytes;
                increment(&mut categories, r.kind, "protectedEntries");
                continue;
            }
            let lease_path = self.lock_path(&r.path, "lease")?;
            let existed = lease_path.exists();
            let file = self.lock_file(&lease_path)?;
            // Inspecting legacy entries must not make them appear recently used.
            if !existed {
                file.set_modified(r.accessed)?;
            }
            match file.try_lock_exclusive() {
                Ok(()) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    busy += r.bytes;
                    increment(&mut categories, r.kind, "busyEntries");
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            let _entry_lock = Lock(file);
            reclaimable += r.bytes;
            let (bytes, count) = totals[&r.kind];
            let (limit, max_count) = r.kind.limits();
            let remove = match mode {
                Mode::Status => false,
                Mode::Clean => true,
                Mode::One(p) => p == r.path,
                Mode::Trim => total > budget || bytes > limit || count > max_count,
            };
            if remove {
                if !dry_run {
                    // Never follow links, including ones introduced since the scan.
                    self.safe(&r.path)?;
                    if r.path.is_dir() {
                        remove_if_present(fs::remove_dir_all(&r.path))?;
                    } else {
                        for path in &r.files {
                            self.safe(path)?;
                            remove_if_present(fs::remove_file(path))?;
                        }
                    }
                }
                total -= r.bytes;
                removed.0 += r.bytes;
                removed.1 += 1;
                let n = totals.get_mut(&r.kind).unwrap();
                n.0 -= r.bytes;
                n.1 -= 1;
            }
        }
        let mut over_budget = total > budget;
        for kind in KINDS {
            let (bytes, count) = totals.get(&kind).copied().unwrap_or_default();
            let (limit, max_count) = kind.limits();
            let exceeded = bytes > limit || count > max_count;
            categories[kind.name()]["remainingBytes"] = json!(bytes);
            categories[kind.name()]["overBudget"] = json!(exceeded);
            over_budget |= exceeded;
        }
        Ok(
            json!({"ok":true,"bytes":before,"entries":records.len(),"reclaimableBytes":reclaimable,"busyBytes":busy,"protectedBytes":protected_bytes,"removedBytes":removed.0,"removedEntries":removed.1,"remainingBytes":total,"dryRun":dry_run,"limitBytes":budget,"overBudget":over_budget,"categories":categories}),
        )
    }
}
fn increment(categories: &mut serde_json::Map<String, Value>, kind: Kind, key: &str) {
    categories[kind.name()][key] = json!(categories[kind.name()][key].as_u64().unwrap() + 1);
}
enum Mode<'a> {
    Status,
    Clean,
    Trim,
    One(&'a Path),
}
// Producers may publish/remove temporary files while a status scan runs.
fn remove_if_present(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
fn children(path: &Path) -> std::io::Result<Vec<fs::DirEntry>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}
fn file_type(entry: &fs::DirEntry) -> std::io::Result<Option<fs::FileType>> {
    match entry.file_type() {
        Ok(ty) => Ok(Some(ty)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn tree_files(dir: &Path, kind: Kind) -> Result<Option<Vec<PathBuf>>> {
    let mut files = Vec::new();
    for child in children(dir)? {
        let Some(ty) = file_type(&child)? else {
            continue;
        };
        if ty.is_symlink() {
            return Ok(None);
        }
        if ty.is_dir() && kind == Kind::Staging {
            let Some(nested) = tree_files(&child.path(), kind)? else {
                return Ok(None);
            };
            files.extend(nested);
        } else if ty.is_file() {
            let name = child.file_name();
            let name = name.to_string_lossy();
            let name = name.strip_suffix(".tmp").unwrap_or(&name);
            let allowed = match kind {
                Kind::FullPreview => name == "preview.mp4",
                Kind::Playback => {
                    matches!(name, "init.mp4" | "stream.m3u8" | "metadata.json")
                        || name
                            .strip_prefix("part-")
                            .and_then(|s| s.strip_suffix(".m4s"))
                            .is_some_and(|s| s.len() == 6 && s.bytes().all(|b| b.is_ascii_digit()))
                }
                Kind::Staging => true,
                _ => false,
            };
            if !allowed {
                return Ok(None);
            }
            files.push(child.path());
        } else {
            return Ok(None);
        }
    }
    Ok(Some(files))
}
impl Entry {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn touch(&self) -> Result<()> {
        self._lease.0.set_modified(SystemTime::now())?;
        Ok(())
    }
    pub fn read(&self) -> Result<Vec<u8>> {
        let bytes = fs::read(&self.path)?;
        self.touch()?;
        Ok(bytes)
    }
    /// Serialize same-key builders across processes, while shared leases protect readers.
    pub fn build_lock(&self) -> Result<impl Drop> {
        let file = self
            .manager
            .lock_file(&self.manager.lock_path(&self.path, "build")?)?;
        file.lock_exclusive()?;
        Ok(Lock(file))
    }
    pub fn publish(&self, bytes: &[u8]) -> Result<()> {
        self.manager.safe(&self.path)?;
        crate::store::atomic_write(&self.path, bytes)?;
        self.finish()
    }
    pub fn publish_file(&self, source: &Path) -> Result<()> {
        self.manager.safe(&self.path)?;
        let mut staged = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        std::io::copy(&mut File::open(source)?, &mut staged)?;
        staged.as_file().sync_all()?;
        staged.persist(&self.path).map_err(|e| e.error)?;
        crate::store::sync_dir(self.path.parent().unwrap())?;
        self.finish()
    }
    pub fn publish_window(&self, raw: &Path, pts: &Path) -> Result<()> {
        let ready = self.path.with_extension("ready");
        for path in [&self.path, &self.path.with_extension("pts"), &ready] {
            self.manager.safe(path)?;
        }
        if ready.exists() {
            fs::remove_file(&ready)?;
        }
        fs::rename(raw, &self.path)?;
        fs::rename(pts, self.path.with_extension("pts"))?;
        crate::store::atomic_write(&ready, b"ready")?;
        self.finish()
    }
    pub fn finish(&self) -> Result<()> {
        self.touch()?;
        self.manager.trim()
    }
    pub fn bytes(&self) -> u64 {
        fn size(path: &Path) -> u64 {
            let Ok(m) = fs::symlink_metadata(path) else {
                return 0;
            };
            if m.is_file() {
                m.len()
            } else if m.is_dir() {
                fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.ok())
                    .map(|e| size(&e.path()))
                    .sum()
            } else {
                0
            }
        }
        size(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifact(manager: &Manager, kind: Kind, n: usize, ext: &str) -> PathBuf {
        let path = manager
            .directory(kind)
            .unwrap()
            .join(format!("{n:064x}.{ext}"));
        fs::write(&path, [0; 4]).unwrap();
        path
    }
    fn age(manager: &Manager, path: &Path, seconds: u64) {
        let entry = manager.entry(path).unwrap();
        entry
            ._lease
            .0
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    }
    #[test]
    fn global_lru_access_and_pinned_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manager::new(tmp.path());
        let p = json!({});
        let a = artifact(&m, Kind::Frames, 1, "png");
        let b = artifact(&m, Kind::Ranges, 2, "mp4");
        let c = artifact(&m, Kind::Sheets, 3, "png");
        age(&m, &a, 1);
        age(&m, &b, 2);
        age(&m, &c, 3);
        // A hit, rather than file creation time, determines recency.
        m.entry(&a).unwrap().read().unwrap();
        let held = m.entry(&c).unwrap();
        let result = m.manage(&p, Mode::Trim, false, 8).unwrap();
        assert_eq!(result["removedBytes"], 4);
        assert!(!b.exists());
        assert!(a.exists() && c.exists());
        let result = m.manage(&p, Mode::Trim, false, 0).unwrap();
        assert_eq!(result["busyBytes"], 4);
        assert_eq!(result["overBudget"], true);
        assert!(c.exists());
        drop(held);
        m.clean(&p, false).unwrap();
        assert!(!c.exists());
    }
    #[test]
    fn category_count_budget_and_legacy_inspection_preserve_age() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manager::new(tmp.path());
        let first = artifact(&m, Kind::Frames, 0, "png");
        File::options()
            .write(true)
            .open(&first)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        for n in 1..257 {
            artifact(&m, Kind::Frames, n, "png");
        }
        let before = m.status(&json!({})).unwrap();
        assert_eq!(before["entries"], 257);
        assert_eq!(
            fs::metadata(m.lock_path(&first, "lease").unwrap())
                .unwrap()
                .modified()
                .unwrap(),
            SystemTime::UNIX_EPOCH
        );
        let result = m
            .manage(&json!({}), Mode::Trim, false, TOTAL_BYTES)
            .unwrap();
        assert_eq!(result["removedEntries"], 1);
        assert!(!first.exists());
    }
    #[test]
    fn namespace_byte_limit_and_locked_project_defer_eviction() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::store::Store::init(tmp.path(), "cache").unwrap();
        let m = Manager::new(tmp.path());
        let path = artifact(&m, Kind::Text, 1, "rgba");
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(65 * 1024 * 1024)
            .unwrap();
        let report = m.status(&store.project().unwrap()).unwrap();
        assert_eq!(report["overBudget"], true);
        assert_eq!(report["categories"]["text"]["overBudget"], true);
        m.trim().unwrap();
        assert!(path.exists());
        drop(store);
        m.trim().unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn absent_cache_status_has_complete_shape_without_creating_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manager::new(tmp.path());
        let report = m.status(&json!({})).unwrap();
        assert_eq!(report["remainingBytes"], 0);
        assert_eq!(report["categories"].as_object().unwrap().len(), KINDS.len());
        assert!(!m.root.exists());
    }
    #[test]
    fn bundle_and_staging_are_atomic_cleanup_units() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manager::new(tmp.path());
        let raw = artifact(&m, Kind::Decode, 1, "raw");
        fs::write(raw.with_extension("pts"), b"pts").unwrap();
        fs::write(raw.with_extension("ready"), b"ready").unwrap();
        let lease = m.entry(&raw).unwrap();
        let scratch = m.scratch().unwrap();
        fs::write(scratch.path().join("x"), b"large").unwrap();
        let report = m.clean(&json!({}), false).unwrap();
        assert_eq!(report["removedEntries"], 0);
        assert_eq!(report["busyBytes"], 17);
        drop(lease);
        let report = m.clean(&json!({}), true).unwrap();
        assert_eq!(report["removedEntries"], 1);
        assert!(raw.exists());
        m.clean(&json!({}), false).unwrap();
        for ext in ["raw", "pts", "ready"] {
            assert!(!raw.with_extension(ext).exists());
        }
        let scratch_path = scratch.path().to_owned();
        drop(scratch);
        assert!(!scratch_path.exists());
    }
    #[test]
    fn assets_unknown_files_and_directory_contents_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manager::new(tmp.path());
        let pinned = artifact(&m, Kind::Frames, 1, "png");
        let loose = artifact(&m, Kind::Frames, 2, "png");
        let unknown = m.root.join("interactive-v1/original.png");
        fs::write(&unknown, b"media").unwrap();
        let full = m.root.join("preview-0.1.0-640-24-v1-abc-seq");
        fs::create_dir_all(&full).unwrap();
        fs::write(full.join("preview.mp4"), b"movie").unwrap();
        fs::write(full.join("notes.txt"), b"keep").unwrap();
        let p =
            json!({"assets":{"asset":{"uri":url::Url::from_file_path(&pinned).unwrap().as_str()}}});
        let result = m.clean(&p, false).unwrap();
        assert_eq!(result["protectedBytes"], 4);
        assert!(pinned.exists() && unknown.exists() && full.join("notes.txt").exists());
        assert!(!loose.exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_linked_asset_aliases_are_preserved() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let m = Manager::new(tmp.path());
        let pinned = artifact(&m, Kind::Frames, 1, "png");
        let alias = tmp.path().join("asset.png");
        symlink(&pinned, &alias).unwrap();
        let linked = m.root.join(format!("interactive-v1/{:064x}.png", 2));
        symlink(&pinned, &linked).unwrap();
        let p = json!({"assets":{"asset":{"uri":"asset.png"}}});
        m.clean(&p, false).unwrap();
        assert!(pinned.exists());
        assert!(linked.is_symlink());
        assert!(m.entry(&linked).is_err());
        let raw = artifact(&m, Kind::Decode, 3, "raw");
        symlink("missing", raw.with_extension("pts")).unwrap();
        m.clean(&json!({}), false).unwrap();
        assert!(raw.exists());
        assert!(raw.with_extension("pts").is_symlink());
    }
}
