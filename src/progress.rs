//! Optional stderr progress. Library calls are silent unless given a reporter.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    io::Write,
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Shared by a job owner and its worker. Cancellation never publishes output.
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    parent: Option<Arc<CancellationToken>>,
}
impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
    }
    pub(crate) fn child(&self) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: Some(Arc::new(self.clone())),
        }
    }
    pub fn check(&self) -> Result<()> {
        ensure!(!self.is_cancelled(), "JOB_CANCELLED");
        Ok(())
    }
}
pub type ProgressCallback = Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    None,
    Human,
    Json,
}

#[derive(Clone)]
struct State {
    phase: &'static str,
    unit: &'static str,
    completed: u64,
    total: Option<u64>,
    started: Instant,
    emitted: Instant,
}

pub struct Progress {
    mode: Mode,
    started: Instant,
    state: RefCell<State>,
    cancellation: CancellationToken,
    callback: Option<ProgressCallback>,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new(Mode::None)
    }
}

impl Progress {
    pub fn new(mode: Mode) -> Self {
        let now = Instant::now();
        Self {
            mode,
            cancellation: CancellationToken::default(),
            callback: None,
            started: now,
            state: RefCell::new(State {
                phase: "starting",
                unit: "items",
                completed: 0,
                total: None,
                started: now,
                emitted: now,
            }),
        }
    }

    pub fn with_callback(cancellation: CancellationToken, callback: ProgressCallback) -> Self {
        Self {
            cancellation,
            callback: Some(callback),
            ..Self::default()
        }
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn with_cancellation(cancellation: CancellationToken) -> Self {
        Self {
            cancellation,
            ..Self::default()
        }
    }
    pub fn check_cancelled(&self) -> Result<()> {
        self.cancellation.check()
    }
    pub(crate) fn output(&self, command: &mut Command) -> Result<Output> {
        crate::media_process::output_cancellable(command, &self.cancellation, |_| {})
    }

    pub fn start(&self, phase: &'static str, total: Option<u64>, unit: &'static str) {
        let now = Instant::now();
        *self.state.borrow_mut() = State {
            phase,
            unit,
            completed: 0,
            total,
            started: now,
            emitted: now,
        };
        self.emit("running");
    }

    /// Restore the parent phase after a nested decoder without restarting it
    /// for every rendered frame (which would flood SSE and reset the ETA).
    pub(crate) fn advance_phase(
        &self,
        phase: &'static str,
        total: Option<u64>,
        unit: &'static str,
        completed: u64,
    ) {
        if self.state.borrow().phase != phase {
            self.start(phase, total, unit);
        }
        self.advance(completed);
    }

    pub fn advance(&self, completed: u64) {
        if self.mode == Mode::None && self.callback.is_none() {
            return;
        }
        let mut state = self.state.borrow_mut();
        let completed = state.total.map_or(completed, |n| completed.min(n));
        let changed = completed > state.completed;
        state.completed = state.completed.max(completed);
        let emit = changed
            && (state.emitted.elapsed() >= Duration::from_millis(500)
                || state.total == Some(state.completed));
        if emit {
            state.emitted = Instant::now();
        }
        drop(state);
        if emit {
            self.emit("running");
        }
    }

    pub(crate) fn preserve(&self) -> PhaseGuard<'_> {
        PhaseGuard {
            reporter: self,
            state: self.state.borrow().clone(),
        }
    }

    pub fn finish(&self) {
        let now = Instant::now();
        *self.state.borrow_mut() = State {
            phase: "complete",
            unit: "jobs",
            completed: 1,
            total: Some(1),
            started: now,
            emitted: now,
        };
        self.emit("completed");
    }

    fn emit(&self, status: &str) {
        if self.mode == Mode::None && self.callback.is_none() {
            return;
        }
        let state = self.state.borrow();
        let elapsed = state.started.elapsed().as_secs_f64();
        let eta = state.total.filter(|n| *n > 0).and_then(|n| {
            (state.completed > 0)
                .then(|| elapsed * (n - state.completed) as f64 / state.completed as f64)
        });
        let value = json!({"type":"progress","phase":state.phase,"status":status,
            "completed":state.completed,"total":state.total,"unit":state.unit,
            "elapsedSeconds":self.started.elapsed().as_secs_f64(),"phaseElapsedSeconds":elapsed,
            "estimatedRemainingPhaseSeconds":eta});
        if let Some(callback) = &self.callback {
            callback(value.clone());
        }
        if self.mode == Mode::None {
            return;
        }
        let mut stderr = std::io::stderr().lock();
        if self.mode == Mode::Json {
            let _ = writeln!(stderr, "{}", value);
        } else {
            let total = state.total.map_or_else(|| "?".into(), |n| n.to_string());
            let eta = eta.map_or_else(|| "unknown".into(), |n| format!("~{n:.1}s"));
            let _ = writeln!(
                stderr,
                "[{}] {}/{} {} | elapsed {:.1}s | phase ETA {} | {}",
                state.phase,
                state.completed,
                total,
                state.unit,
                self.started.elapsed().as_secs_f64(),
                eta,
                status
            );
        }
    }

    /// Read FFmpeg's progress pipe while concurrently draining stderr. Diagnostics
    /// are bounded; error paths kill/reap the process and never signal completion.
    pub(crate) fn ffmpeg(
        &self,
        cmd: &mut Command,
        phase: &'static str,
        total: Option<u64>,
    ) -> Result<()> {
        self.start(
            phase,
            total,
            if total.is_some() {
                "frames"
            } else {
                "media_microseconds"
            },
        );
        cmd.args(["-nostats", "-progress", "pipe:1"]);
        let output = crate::media_process::output_cancellable(cmd, &self.cancellation, |line| {
            let field = if total.is_some() {
                "frame="
            } else {
                "out_time_us="
            };
            if let Some(n) = line.strip_prefix(field).and_then(|v| v.trim().parse().ok()) {
                self.advance(n);
            }
        })?;
        ensure!(
            output.status.success(),
            "MEDIA_PROCESS_FAILED: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(total) = total {
            self.advance(total);
        }
        Ok(())
    }
}

pub(crate) struct PhaseGuard<'a> {
    reporter: &'a Progress,
    state: State,
}
impl Drop for PhaseGuard<'_> {
    fn drop(&mut self) {
        *self.reporter.state.borrow_mut() = self.state.clone();
        self.reporter.emit("running");
    }
}
