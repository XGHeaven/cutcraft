//! Actionable launch errors shared by every external media process.
use std::{
    error::Error,
    fmt, io,
    process::{Child, Command, Output},
};

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
        let error = Command::new("ffmpeg")
            .env("PATH", path.path())
            .media_spawn()
            .unwrap_err();
        let error = error.downcast_ref::<MediaToolError>().unwrap();
        assert_eq!(error.code(), "MEDIA_TOOL_NOT_FOUND");
        assert_eq!(error.executable, "ffmpeg");
    }
}
