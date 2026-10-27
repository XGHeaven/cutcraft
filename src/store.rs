//! Durable two-file transactions: an fsynced intent precedes snapshot/history
//! replacement. Every cooperating reader takes the same OS advisory lock and
//! finishes an interrupted transaction before observing the project.
use crate::{edit, project, schema};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn hash(value: &Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project::canonical(value)?)))
}
pub fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    sync_dir(parent)?;
    Ok(())
}
pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    // Windows does not expose a directory fsync equivalent through std. File
    // contents are flushed before the atomic replacement on every platform.
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub struct Store {
    pub dir: PathBuf,
    _lock: File,
}
impl Drop for Store {
    fn drop(&mut self) {
        // A concurrently spawned media child can briefly inherit the open file
        // description before exec closes it. Release our lock explicitly rather
        // than waiting for every inherited/duplicated descriptor to close.
        let _ = FileExt::unlock(&self._lock);
    }
}
impl Store {
    fn lock(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir.join(".cutcraft"))?;
        let mut lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(".cutcraft.lock"))?;
        lock.try_lock_exclusive().context(
            "PROJECT_LOCKED: another process holds .cutcraft.lock; retry after it finishes",
        )?;
        // Never unlink/replace this file: all processes must lock the same inode.
        // PID is diagnostic only; the OS lock is authoritative after crashes.
        lock.set_len(0)?;
        lock.write_all(&serde_json::to_vec(&json!({
            "pid":std::process::id(), "acquiredAt":crate::now(),
            "application":project::application()
        }))?)?;
        lock.sync_all()?;
        let store = Self {
            dir: dir.canonicalize()?,
            _lock: lock,
        };
        store.recover()?;
        Ok(store)
    }
    pub fn open(dir: &Path) -> Result<Self> {
        ensure!(!dir.join("project.json").exists(), "LEGACY_PROJECT: project.json requires explicit 0.2.1 migration; it will not be overwritten");
        ensure!(
            dir.join("cutcraft.json").is_file() || dir.join(".cutcraft/transaction.json").is_file(),
            "not a Cutcraft project: {}",
            dir.display()
        );
        Self::lock(dir)
    }
    pub fn init(dir: &Path, name: &str) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let store = Self::lock(dir)?;
        ensure!(
            !store.dir.join("cutcraft.json").exists() && !store.dir.join("project.json").exists(),
            "project already exists"
        );
        ensure!(
            !store.dir.join("history.jsonl").exists(),
            "history already exists; refusing to overwrite"
        );
        let project = project::new(name);
        project::validate(&project)?;
        for sub in ["assets", "cache", "output"] {
            fs::create_dir_all(store.dir.join(sub))?;
        }
        store.commit(&project, &[])?;
        Ok(store)
    }
    pub fn project(&self) -> Result<Value> {
        let project = read_json(&self.dir.join("cutcraft.json"))?;
        project::validate(&project)?;
        Ok(project)
    }
    pub fn history(&self) -> Result<Vec<Value>> {
        let path = self.dir.join("history.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        fs::read_to_string(path)?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let e: Value = serde_json::from_str(l)?;
                schema::validate("event", &e)?;
                let before = e["revisionBefore"].as_u64().unwrap();
                let after = e["revisionAfter"].as_u64().unwrap();
                ensure!(
                    if e["status"] == "applied" {
                        before.checked_add(1) == Some(after)
                    } else {
                        before == after
                    },
                    "invalid history revision increment"
                );
                Ok(e)
            })
            .collect()
    }
    pub fn apply(&self, op: &Value, override_locks: bool, dry_run: bool) -> Result<Value> {
        let project = self.project()?;
        schema::validate("operation", op)?;
        let mut history = self.history()?;
        ensure!(
            !history
                .iter()
                .any(|e| e["operationId"] == op["operationId"]),
            "DUPLICATE_OPERATION: use a fresh operationId"
        );
        match edit::apply(&project, op, override_locks) {
            Ok((next, event)) => {
                if !dry_run {
                    history.push(event.clone());
                    self.commit(&next, &history)?;
                }
                Ok(json!({"ok":true,"dryRun":dry_run,"event":event,"project":next}))
            }
            Err(error) => {
                if !dry_run {
                    if let Ok(event) = edit::rejection(&project, op, &error) {
                        history.push(event);
                        self.commit(&project, &history)?;
                    }
                }
                Err(error)
            }
        }
    }
    pub fn command(&self, command: Value, override_locks: bool, dry_run: bool) -> Result<Value> {
        let op = edit::operation(&self.project()?, command);
        self.apply(&op, override_locks, dry_run)
    }
    pub fn undo(&self, override_locks: bool, dry_run: bool) -> Result<Value> {
        let history = self.history()?;
        let last = history
            .iter()
            .rev()
            .find(|e| e["status"] == "applied")
            .context("no applied operation to undo")?;
        let project = self.project()?;
        ensure!(
            last["revisionAfter"] == project["revision"]
                && last["projectHashAfter"] == hash(&project)?,
            "UNDO_CONFLICT: snapshot differs from latest event"
        );
        let commands = last["inverseCommands"]
            .as_array()
            .filter(|a| !a.is_empty())
            .context("event has no inverse commands")?;
        let mut op = edit::operation(&project, json!({"type":"batch","commands":commands}));
        op["metadata"] = json!({"undoOf":last["eventId"]});
        self.apply(&op, override_locks, dry_run)
    }
    /// Shared, multi-step history. Legacy `undoOf` entries remain ordinary edits.
    pub fn history_state(&self) -> Result<Value> {
        let history = self.history()?;
        let (undo, redo) = history_stacks(&history);
        Ok(
            json!({"canUndo":!undo.is_empty(),"canRedo":!redo.is_empty(),
            "undoEventId":undo.last().map(|i| &history[*i]["eventId"]),
            "redoEventId":redo.last().map(|i| &history[*i]["eventId"])}),
        )
    }
    pub fn history_undo(&self, override_locks: bool, dry_run: bool) -> Result<Value> {
        self.history_action(false, override_locks, dry_run)
    }
    pub fn history_redo(&self, override_locks: bool, dry_run: bool) -> Result<Value> {
        self.history_action(true, override_locks, dry_run)
    }
    fn history_action(
        &self,
        redo_action: bool,
        override_locks: bool,
        dry_run: bool,
    ) -> Result<Value> {
        let history = self.history()?;
        let project = self.project()?;
        let mut previous: Option<&Value> = None;
        for event in history.iter().filter(|e| e["status"] == "applied") {
            if let Some(before) = previous {
                ensure!(
                    event["revisionBefore"] == before["revisionAfter"]
                        && event["projectHashBefore"] == before["projectHashAfter"],
                    "UNDO_CONFLICT: discontinuous project history"
                );
            }
            previous = Some(event);
        }
        let last = previous.context("no applied operation to undo")?;
        ensure!(
            last["revisionAfter"] == project["revision"]
                && last["projectHashAfter"] == hash(&project)?,
            "UNDO_CONFLICT: snapshot differs from latest event"
        );
        let (undo, redo) = history_stacks(&history);
        let index = if redo_action {
            redo.last()
        } else {
            undo.last()
        }
        .context(if redo_action {
            "NOTHING_TO_REDO"
        } else {
            "NOTHING_TO_UNDO"
        })?;
        let target = &history[*index];
        let commands = target["inverseCommands"]
            .as_array()
            .filter(|v| !v.is_empty())
            .context("event has no inverse commands")?;
        let mut operation = edit::operation(&project, json!({"type":"batch","commands":commands}));
        operation["metadata"] = json!({"cutcraft.history":{"version":1,
            "action":if redo_action {"redo"} else {"undo"},"targetEventId":target["eventId"]}});
        self.apply(&operation, override_locks, dry_run)
    }
    fn commit(&self, project: &Value, history: &[Value]) -> Result<()> {
        let mut log = String::new();
        for event in history {
            log.push_str(&serde_json::to_string(event)?);
            log.push('\n');
        }
        let transaction = json!({"project":project,"history":log});
        atomic_write(
            &self.dir.join(".cutcraft/transaction.json"),
            &project::canonical(&transaction)?,
        )?;
        self.recover()
    }
    fn recover(&self) -> Result<()> {
        let journal = self.dir.join(".cutcraft/transaction.json");
        if !journal.exists() {
            return Ok(());
        }
        let tx = read_json(&journal)?;
        project::validate(&tx["project"])?;
        let history = tx["history"]
            .as_str()
            .context("invalid transaction journal")?;
        for line in history.lines() {
            schema::validate("event", &serde_json::from_str::<Value>(line)?)?;
        }
        atomic_write(
            &self.dir.join("cutcraft.json"),
            &project::canonical(&tx["project"])?,
        )?;
        atomic_write(&self.dir.join("history.jsonl"), history.as_bytes())?;
        fs::remove_file(journal)?;
        sync_dir(&self.dir.join(".cutcraft"))?;
        Ok(())
    }
}

fn history_stacks(history: &[Value]) -> (Vec<usize>, Vec<usize>) {
    let mut undo: Vec<usize> = Vec::new();
    let mut redo: Vec<usize> = Vec::new();
    for (index, event) in history
        .iter()
        .enumerate()
        .filter(|(_, e)| e["status"] == "applied")
    {
        let marker = &event["metadata"]["cutcraft.history"];
        let action = marker["action"].as_str().unwrap_or("");
        let target = match action {
            "undo" => undo.last(),
            "redo" => redo.last(),
            _ => None,
        };
        let valid = marker["version"] == 1
            && target.is_some_and(|i| {
                marker["targetEventId"] == history[*i]["eventId"]
                    && event["command"]
                        == json!({"type":"batch","commands":history[*i]["inverseCommands"]})
            });
        if valid && action == "undo" {
            undo.pop();
            redo.push(index);
        } else if valid && action == "redo" {
            redo.pop();
            undo.push(index);
        } else {
            undo.push(index);
            redo.clear();
        }
    }
    (undo, redo)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropping_store_unlocks_even_while_a_duplicated_descriptor_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), "lock lifetime").unwrap();
        let inherited = store._lock.try_clone().unwrap();
        assert!(Store::open(tmp.path()).is_err());
        drop(store);
        let reopened = Store::open(tmp.path()).unwrap();
        drop(inherited);
        assert!(Store::open(tmp.path()).is_err());
        drop(reopened);
    }
}
