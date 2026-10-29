//! Runtime H.264 encoding settings; never part of the editing protocol.
use anyhow::{ensure, Result};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Command;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum VideoEncoder {
    #[default]
    Libx264,
    H264Videotoolbox,
    H264Nvenc,
    H264Qsv,
}
impl VideoEncoder {
    pub fn name(self) -> &'static str {
        match self {
            Self::Libx264 => "libx264",
            Self::H264Videotoolbox => "h264_videotoolbox",
            Self::H264Nvenc => "h264_nvenc",
            Self::H264Qsv => "h264_qsv",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Args, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EncodingOptions {
    /// H.264 encoder; hardware choices require a compatible FFmpeg build and device.
    #[arg(long, value_enum, default_value = "libx264")]
    pub video_encoder: VideoEncoder,
    /// Target video bitrate in bits/s (e.g. 8000000); hardware defaults: export 8M, preview 2M.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=1_000_000_000))]
    pub video_bitrate: Option<u64>,
}
impl EncodingOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.video_bitrate
                .is_none_or(|v| (1..=1_000_000_000).contains(&v)),
            "INVALID_VIDEO_BITRATE: expected 1..=1000000000 bits/s"
        );
        Ok(())
    }
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    fn bitrate(&self, preview: bool) -> Option<u64> {
        self.video_bitrate.or_else(|| {
            (self.video_encoder != VideoEncoder::Libx264).then_some(if preview {
                2_000_000
            } else {
                8_000_000
            })
        })
    }
    pub(crate) fn pixel_format(&self) -> &'static str {
        if self.video_encoder == VideoEncoder::H264Qsv {
            "nv12"
        } else {
            "yuv420p"
        }
    }
    pub(crate) fn apply(&self, cmd: &mut Command, preview: bool) {
        cmd.args([
            "-c:v",
            self.video_encoder.name(),
            "-threads",
            "2",
            "-pix_fmt",
            self.pixel_format(),
        ]);
        match self.video_encoder {
            VideoEncoder::Libx264 => {
                cmd.args(["-preset", if preview { "ultrafast" } else { "fast" }]);
                if self.video_bitrate.is_none() {
                    cmd.args(["-crf", if preview { "28" } else { "18" }]);
                }
            }
            VideoEncoder::H264Videotoolbox => {
                cmd.args(["-allow_sw", "0"]);
            }
            VideoEncoder::H264Nvenc => {
                cmd.args(["-preset", if preview { "p1" } else { "p4" }]);
            }
            VideoEncoder::H264Qsv => {
                cmd.args(["-preset", if preview { "veryfast" } else { "medium" }]);
            }
        }
        if let Some(bitrate) = self.bitrate(preview) {
            cmd.args(["-b:v", &bitrate.to_string()]);
        }
    }
    pub fn metadata(&self, preview: bool) -> Value {
        json!({"videoEncoder":self.video_encoder.name(),"videoBitrate":self.bitrate(preview),
            "rateControl":if self.bitrate(preview).is_some() { "bitrate" } else { "crf" },
            "crf":if self.bitrate(preview).is_none() { Some(if preview {28} else {18}) } else {None}})
    }
}
