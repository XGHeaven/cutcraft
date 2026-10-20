//! Optional stderr progress. Library calls are silent unless given a reporter.
use crate::media_process::MediaCommand;
use anyhow::{ensure, Result};
use serde_json::json;
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

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

    pub fn advance(&self, completed: u64) {
        if self.mode == Mode::None {
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
        if self.mode == Mode::None {
            return;
        }
        let state = self.state.borrow();
        let elapsed = state.started.elapsed().as_secs_f64();
        let eta = state.total.filter(|n| *n > 0).and_then(|n| {
            (state.completed > 0)
                .then(|| elapsed * (n - state.completed) as f64 / state.completed as f64)
        });
        let mut stderr = std::io::stderr().lock();
        if self.mode == Mode::Json {
            let _ = writeln!(
                stderr,
                "{}",
                json!({"type":"progress","phase":state.phase,"status":status,
                "completed":state.completed,"total":state.total,"unit":state.unit,
                "elapsedSeconds":self.started.elapsed().as_secs_f64(),"phaseElapsedSeconds":elapsed,
                "estimatedRemainingPhaseSeconds":eta})
            );
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
        if self.mode == Mode::None {
            let output = cmd.media_output()?;
            ensure!(
                output.status.success(),
                "MEDIA_PROCESS_FAILED: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        cmd.args(["-nostats", "-progress", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.media_spawn()?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let errors = std::thread::spawn(move || {
            let mut tail = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 65536 {
                    tail.drain(..tail.len() - 65536);
                }
            }
            tail
        });
        let read = (|| -> Result<()> {
            for line in BufReader::new(stdout).lines() {
                let line = line?;
                let field = if total.is_some() {
                    "frame="
                } else {
                    "out_time_us="
                };
                if let Some(n) = line.strip_prefix(field).and_then(|v| v.trim().parse().ok()) {
                    self.advance(n);
                }
            }
            Ok(())
        })();
        if read.is_err() {
            let _ = child.kill();
        }
        let status = child.wait();
        let tail = errors.join().unwrap_or_default();
        read?;
        ensure!(
            status?.success(),
            "MEDIA_PROCESS_FAILED: {}",
            String::from_utf8_lossy(&tail)
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
