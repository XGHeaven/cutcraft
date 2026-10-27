//! Actionable launch errors shared by every external media process.
use std::{
    error::Error,
    fmt, io,
    process::{Child, Command, Output},
    sync::OnceLock,
};

thread_local! { static CANCELLATION: std::cell::RefCell<Option<crate::progress::CancellationToken>> = const { std::cell::RefCell::new(None) }; }
/// Scope cancellation to a synchronous application worker, without affecting
/// independent commands executing on other threads.
pub fn with_cancellation<T>(
    token: crate::progress::CancellationToken,
    work: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<crate::progress::CancellationToken>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CANCELLATION.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let old = CANCELLATION.with(|slot| slot.replace(Some(token)));
    let _restore = Restore(old);
    work()
}
pub fn check_cancellation() -> anyhow::Result<()> {
    CANCELLATION.with(|slot| slot.borrow().as_ref().map_or(Ok(()), |token| token.check()))
}

/// Drain both pipes while the owner polls cancellation; always kill/reap on cancel.
pub(crate) fn output_cancellable(
    command: &mut Command,
    cancellation: &crate::progress::CancellationToken,
    mut line: impl FnMut(&str),
) -> anyhow::Result<Output> {
    use std::{
        io::{BufRead, BufReader, Read},
        process::Stdio,
        sync::mpsc,
        time::Duration,
    };
    cancellation.check()?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .media_spawn()?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (send, recv) = mpsc::sync_channel(128);
    let out = std::thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut reader = BufReader::new(stdout);
        let mut all = Vec::new();
        loop {
            let mut buf = Vec::new();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            let _ = send.try_send(String::from_utf8_lossy(&buf).trim_end().to_owned());
            all.extend_from_slice(&buf);
        }
        Ok(all)
    });
    let err = std::thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut tail = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stderr.read(&mut buf)?;
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buf[..n]);
            if tail.len() > 65536 {
                tail.drain(..tail.len() - 65536);
            }
        }
        Ok(tail)
    });
    let result = loop {
        while let Ok(value) = recv.try_recv() {
            line(&value);
        }
        if cancellation.is_cancelled() {
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = out
        .join()
        .map_err(|_| anyhow::anyhow!("MEDIA_READER_FAILED"))??;
    let stderr = err
        .join()
        .map_err(|_| anyhow::anyhow!("MEDIA_READER_FAILED"))??;
    while let Ok(value) = recv.try_recv() {
        line(&value);
    }
    cancellation.check()?;
    Ok(Output {
        status: result?,
        stdout,
        stderr,
    })
}

/// FFmpeg 9 removed the legacy graph-file option. Detect it once per process,
/// retaining FFmpeg 6.1 support and file-based graphs (Windows argv is bounded).
pub(crate) fn graph_file_option() -> anyhow::Result<&'static str> {
    static OPTION: OnceLock<&'static str> = OnceLock::new();
    if let Some(option) = OPTION.get() {
        return Ok(option);
    }
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-h", "full"])
        .media_output()?;
    anyhow::ensure!(output.status.success(), "FFMPEG_CAPABILITY_CHECK_FAILED");
    let help = String::from_utf8_lossy(&output.stdout);
    let option = if help.contains("-filter_complex_script ") {
        "-filter_complex_script"
    } else {
        anyhow::ensure!(
            help.contains("-filter_complex "),
            "FFMPEG_FILTER_GRAPH_UNSUPPORTED"
        );
        "-/filter_complex"
    };
    let _ = OPTION.set(option);
    Ok(option)
}

#[derive(Debug)]
pub struct MediaToolError {
    pub executable: String,
    source: io::Error,
}

impl MediaToolError {
    pub fn code(&self) -> &'static str {
        match self.source.kind() {
            io::ErrorKind::NotFound => "MEDIA_TOOL_NOT_FOUND",
            io::ErrorKind::PermissionDenied => "MEDIA_TOOL_PERMISSION_DENIED",
            _ => "MEDIA_TOOL_START_FAILED",
        }
    }

    pub fn hint(&self) -> String {
        if self.source.kind() != io::ErrorKind::NotFound {
            return format!(
                "Check that {} in PATH is executable and compatible with this OS/architecture. Run `{} -version` to verify.",
                self.executable, self.executable
            );
        }
        let install = if cfg!(target_os = "macos") {
            r#"Install FFmpeg with `brew install ffmpeg@7`, then run `export PATH="$(brew --prefix ffmpeg@7)/bin:$PATH"` (includes the drawtext filter)."#
        } else if cfg!(target_os = "linux") {
            "Install FFmpeg with your package manager (Ubuntu 24.04: `sudo apt-get update && sudo apt-get install ffmpeg`)."
        } else if cfg!(target_os = "windows") {
            "Install a Windows FFmpeg build with drawtext, libx264 and FFV1; add the directory containing ffmpeg.exe and ffprobe.exe to PATH."
        } else {
            "Install an FFmpeg distribution containing both ffmpeg and ffprobe."
        };
        format!(
            "{install} Cutcraft media commands require FFmpeg 6.1+; ensure both ffmpeg and ffprobe are in PATH. Verify with `ffmpeg -version` and `ffprobe -version`."
        )
    }
}

impl fmt::Display for MediaToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.source.kind() {
            io::ErrorKind::NotFound => "executable not found in PATH",
            io::ErrorKind::PermissionDenied => "permission denied when starting executable",
            _ => "could not start or collect output from executable",
        };
        write!(
            f,
            "{}: {}: {}. {}",
            self.code(),
            self.executable,
            reason,
            self.hint()
        )
    }
}

impl Error for MediaToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

pub(crate) trait MediaCommand {
    fn media_output(&mut self) -> anyhow::Result<Output>;
    fn media_spawn(&mut self) -> anyhow::Result<Child>;
}

impl MediaCommand for Command {
    fn media_output(&mut self) -> anyhow::Result<Output> {
        if let Some(token) = CANCELLATION.with(|slot| slot.borrow().clone()) {
            return output_cancellable(self, &token, |_| {});
        }
        self.output().map_err(|source| {
            MediaToolError {
                executable: self.get_program().to_string_lossy().into_owned(),
                source,
            }
            .into()
        })
    }

    fn media_spawn(&mut self) -> anyhow::Result<Child> {
        self.spawn().map_err(|source| {
            MediaToolError {
                executable: self.get_program().to_string_lossy().into_owned(),
                source,
            }
            .into()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piped_processes_use_the_same_dependency_error() {
        let path = tempfile::tempdir().unwrap();
        // Windows may search the parent PATH even when a child PATH is set.
        // A unique bare name also exercises PATH lookup before QEMU's spawn path.
        let executable = format!("missing-ffmpeg-{}", uuid::Uuid::now_v7());
        let error = Command::new(&executable)
            .env("PATH", path.path())
            .media_spawn()
            .unwrap_err();
        let error = error.downcast_ref::<MediaToolError>().unwrap();
        assert_eq!(error.code(), "MEDIA_TOOL_NOT_FOUND");
        assert_eq!(error.executable, executable);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn scoped_cancellation_kills_and_reaps_media_process() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            assert!(std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
            return;
        }
        let token = crate::progress::CancellationToken::default();
        let signal = token.clone();
        let killer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            signal.cancel();
        });
        let began = std::time::Instant::now();
        let error = with_cancellation(token, || {
            Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-re",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440",
                    "-f",
                    "null",
                    "-",
                ])
                .media_output()
        })
        .unwrap_err();
        killer.join().unwrap();
        assert!(error.to_string().contains("JOB_CANCELLED"));
        assert!(began.elapsed() < std::time::Duration::from_secs(3));
        check_cancellation().unwrap();
    }
}
