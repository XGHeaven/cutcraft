//! Bounded raw media transport shared by offline export and browser playback.
//! Independent A/V producers are necessary: FFmpeg may read inputs in either
//! order, so a single producer blocked on audio must not starve video (or vice versa).
use crate::{
    progress::{CancellationToken, Progress},
    time::Time,
};
use anyhow::{ensure, Result};
use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) fn listener() -> Result<TcpListener> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}
pub(crate) fn accept(
    listener: TcpListener,
    cancel: &CancellationToken,
    halt: &AtomicBool,
) -> Result<Option<TcpStream>> {
    loop {
        cancel.check()?;
        if halt.load(Ordering::Acquire) {
            return Ok(None);
        }
        match listener.accept() {
            Ok((socket, _)) => {
                socket.set_write_timeout(Some(Duration::from_millis(100)))?;
                return Ok(Some(socket));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(e.into()),
        }
    }
}
pub(crate) fn write(
    socket: &mut TcpStream,
    data: &[u8],
    cancel: &CancellationToken,
    halt: &AtomicBool,
) -> Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        cancel.check()?;
        ensure!(!halt.load(Ordering::Acquire), "ENCODER_STOPPED");
        match socket.write(&data[offset..]) {
            Ok(0) => anyhow::bail!("ENCODER_INPUT_CLOSED"),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
/// One blocking writer per input. Socket backpressure bounds producer buffering
/// to one generated frame / audio block plus OS buffers; no whole-output spool.
pub(crate) struct Writer {
    socket: TcpStream,
    cancel: CancellationToken,
    halt: Arc<AtomicBool>,
}
impl Writer {
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        write(&mut self.socket, bytes, &self.cancel, &self.halt)
    }
}
pub(crate) struct Format {
    pub width: usize,
    pub height: usize,
    pub fps: Time,
    pub rate: i64,
    pub channels: usize,
    pub frames: i64,
}
impl Format {
    fn inputs(&self, video: u16, audio: u16) -> Command {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-threads",
            "2",
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-fpsprobesize",
            "0",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba64le",
            "-video_size",
            &format!("{}x{}", self.width, self.height),
            "-framerate",
            &self.fps.expression(),
            "-i",
            &format!("tcp://127.0.0.1:{video}"),
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-f",
            "f32le",
            "-ar",
            &self.rate.to_string(),
            "-ac",
            &self.channels.to_string(),
            "-i",
            &format!("tcp://127.0.0.1:{audio}"),
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
        ]);
        cmd
    }
}

pub(crate) fn mp4<V, A>(
    format: &Format,
    output: &Path,
    preview: bool,
    encoding: &crate::encoding::EncodingOptions,
    progress: &Progress,
    video: V,
    audio: A,
) -> Result<()>
where
    V: FnOnce(&mut Writer, &Progress) -> Result<()> + Send,
    A: FnOnce(&mut Writer, &Progress) -> Result<()> + Send,
{
    encoding.validate()?;
    encode(
        format.frames,
        progress,
        |vp, ap| {
            let mut cmd = format.inputs(vp, ap);
            cmd.args(["-filter_threads", "2", "-vf", &format!("scale=out_color_matrix=bt709:out_range=tv,format={},setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709", encoding.pixel_format())]);
            encoding.apply(&mut cmd, preview);
            cmd.args(["-c:a", "aac", "-movflags", "+faststart"])
                .arg(output);
            cmd
        },
        video,
        audio,
    )
}

fn encode<V, A>(
    frames: i64,
    progress: &Progress,
    command: impl FnOnce(u16, u16) -> Command,
    video: V,
    audio: A,
) -> Result<()>
where
    V: FnOnce(&mut Writer, &Progress) -> Result<()> + Send,
    A: FnOnce(&mut Writer, &Progress) -> Result<()> + Send,
{
    progress.check_cancelled()?;
    // Bind both before starting anything, so a setup failure leaves no threads.
    let video_listener = listener()?;
    let audio_listener = listener()?;
    let mut command = command(
        video_listener.local_addr()?.port(),
        audio_listener.local_addr()?.port(),
    );
    let control = progress.cancellation().child();
    let encoder_control = progress.cancellation().child();
    let halt = Arc::new(AtomicBool::new(false));
    let (send, receive) = mpsc::sync_channel(128);
    let reporter = |token: CancellationToken| {
        let send = send.clone();
        Progress::with_callback(
            token,
            Arc::new(move |event| {
                let _ = send.try_send(event);
            }),
        )
    };
    let vp = reporter(control.clone());
    let ap = reporter(control.clone());
    let ep = reporter(encoder_control.clone());
    let results = thread::scope(|scope| {
        struct Stop<'a>(&'a CancellationToken, &'a CancellationToken);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.cancel();
                self.1.cancel();
            }
        }
        let _stop = Stop(&control, &encoder_control);
        let video_thread = scope.spawn(|| producer(video_listener, video, vp, &control, &halt));
        let audio_thread = scope.spawn(|| producer(audio_listener, audio, ap, &control, &halt));
        let producer_control = &control;
        let encoder_halt = &halt;
        let encoder_thread = scope.spawn(move || {
            let result = guarded(producer_control, || {
                ep.ffmpeg(&mut command, "encode", Some(frames as u64))
            });
            encoder_halt.store(true, Ordering::Release);
            producer_control.cancel(); // Also wake producers if the encoder exits before opening inputs.
            result
        });
        let mut failure_since = None;
        while !video_thread.is_finished()
            || !audio_thread.is_finished()
            || !encoder_thread.is_finished()
        {
            // Let FFmpeg report an early failure before cancellation replaces
            // its diagnostic with JOB_CANCELLED. Bound the grace period so a
            // producer failure still stops an encoder waiting for more input.
            if control.is_cancelled()
                && failure_since.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_millis(100)
            {
                encoder_control.cancel();
            }
            if let Ok(event) = receive.recv_timeout(Duration::from_millis(20)) {
                progress.relay(event);
            }
        }
        while let Ok(event) = receive.try_recv() {
            progress.relay(event);
        }
        [
            video_thread.join(),
            audio_thread.join(),
            encoder_thread.join(),
        ]
        .map(|result| result.unwrap_or_else(|_| Err(anyhow::anyhow!("MEDIA_STREAM_WORKER_FAILED"))))
    });
    progress.check_cancelled()?;
    let [video, audio, encoded] = results;
    let producers_complete = video.is_ok() && audio.is_ok();
    let mut transport_error = None;
    for result in [video, audio] {
        if let Err(error) = result {
            if error.to_string().contains("JOB_CANCELLED") {
                continue;
            }
            if error.downcast_ref::<std::io::Error>().is_none()
                && !error.to_string().starts_with("ENCODER_")
            {
                return Err(error);
            }
            transport_error.get_or_insert(error);
        }
    }
    if let Err(error) = &encoded {
        if !error.to_string().contains("JOB_CANCELLED") {
            return encoded;
        }
    }
    if let Some(error) = transport_error {
        return Err(error);
    }
    encoded?;
    ensure!(
        producers_complete,
        "ENCODER_STOPPED: inputs were not fully consumed"
    );
    Ok(())
}
fn guarded(control: &CancellationToken, work: impl FnOnce() -> Result<()>) -> Result<()> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("MEDIA_STREAM_WORKER_FAILED")));
    if result.is_err() {
        control.cancel();
    }
    result
}
fn producer<W>(
    listener: TcpListener,
    work: W,
    progress: Progress,
    control: &CancellationToken,
    halt: &Arc<AtomicBool>,
) -> Result<()>
where
    W: FnOnce(&mut Writer, &Progress) -> Result<()>,
{
    guarded(control, || {
        crate::media_process::with_cancellation(control.clone(), || {
            let socket = accept(listener, control, halt)?
                .ok_or_else(|| anyhow::anyhow!("ENCODER_STOPPED"))?;
            let mut writer = Writer {
                socket,
                cancel: control.clone(),
                halt: halt.clone(),
            };
            work(&mut writer, &progress)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    fn available() -> bool {
        let yes = Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(yes || std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
        yes
    }
    fn fixture() -> Format {
        Format {
            width: 16,
            height: 16,
            fps: Time::integer(12),
            rate: 48000,
            channels: 2,
            frames: 90,
        }
    }
    fn deadline<T>(progress: &Progress, work: impl FnOnce() -> T) -> T {
        let (send, recv) = mpsc::channel();
        let token = progress.cancellation();
        let watchdog = thread::spawn(move || {
            if recv.recv_timeout(Duration::from_secs(10)).is_err() {
                token.cancel();
            }
        });
        let result = work();
        let _ = send.send(());
        watchdog.join().unwrap();
        result
    }
    #[test]
    fn encoder_starts_before_producer_finishes_and_raw_bytes_match_file_input() {
        if !available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture();
        let encoded_frames = Arc::new(AtomicU64::new(0));
        let observed = encoded_frames.clone();
        let progress = Progress::with_callback(
            CancellationToken::default(),
            Arc::new(move |event| {
                if event["phase"] == "encode" {
                    observed.fetch_max(event["completed"].as_u64().unwrap_or(0), Ordering::Relaxed);
                }
            }),
        );
        let frames: Vec<Vec<u8>> = (0..f.frames)
            .map(|n| {
                let rgb = if n % 3 == 0 {
                    [50000u16, 0, 0, 65535]
                } else if n % 3 == 1 {
                    [0, 50000, 0, 65535]
                } else {
                    [0, 0, 50000, 65535]
                };
                (0..f.width * f.height)
                    .flat_map(|_| rgb.iter().flat_map(|c| c.to_le_bytes()))
                    .collect()
            })
            .collect();
        let samples = f.frames * f.rate / 12;
        let pcm: Vec<u8> = (0..samples)
            .flat_map(|n| {
                let value =
                    ((n as f64 * 440. * std::f64::consts::TAU / f.rate as f64).sin() * 0.25) as f32;
                [value, -value].into_iter().flat_map(f32::to_le_bytes)
            })
            .collect();
        let streamed = tmp.path().join("stream.mp4");
        deadline(&progress, || {
            mp4(
                &f,
                &streamed,
                true,
                &crate::encoding::EncodingOptions::default(),
                &progress,
                |writer, _| {
                    for (n, frame) in frames.iter().enumerate() {
                        writer.write(frame)?;
                        if n == 63 {
                            let until = Instant::now() + Duration::from_secs(4);
                            while encoded_frames.load(Ordering::Relaxed) == 0
                                && Instant::now() < until
                            {
                                thread::sleep(Duration::from_millis(10));
                            }
                            ensure!(
                                encoded_frames.load(Ordering::Relaxed) > 0,
                                "encoder waited for the whole timeline"
                            );
                        }
                    }
                    Ok(())
                },
                |writer, _| {
                    for block in pcm.chunks(4096 * f.channels * 4) {
                        writer.write(block)?;
                    }
                    Ok(())
                },
            )
        })
        .unwrap();
        // Independent file-fed baseline with the same color/codec settings.
        let raw_video = tmp.path().join("baseline.rgba");
        let raw_audio = tmp.path().join("baseline.f32");
        std::fs::write(&raw_video, frames.concat()).unwrap();
        std::fs::write(&raw_audio, &pcm).unwrap();
        let baseline = tmp.path().join("baseline.mp4");
        let output = Command::new("ffmpeg").args(["-v","error","-f","rawvideo","-pixel_format","rgba64le","-video_size","16x16","-framerate","12","-i"]).arg(raw_video)
            .args(["-f","f32le","-ar","48000","-ac","2","-i"]).arg(raw_audio)
            .args(["-filter_threads","2","-vf","scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:colorspace=bt709:color_primaries=bt709:color_trc=bt709","-c:v","libx264","-threads","2","-preset","ultrafast","-crf","28","-c:a","aac"]).arg(&baseline).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for args in [
            vec!["-an", "-pix_fmt", "rgb24", "-f", "rawvideo"],
            vec!["-vn", "-f", "f32le"],
        ] {
            let decode = |path: &Path| {
                let out = Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(path)
                    .args(&args)
                    .arg("pipe:1")
                    .output()
                    .unwrap();
                assert!(out.status.success());
                out.stdout
            };
            let actual = decode(&streamed);
            let expected = decode(&baseline);
            assert!(!actual.is_empty());
            assert_eq!(actual, expected);
            if args[0] == "-an" {
                assert_eq!(actual.len(), f.frames as usize * 16 * 16 * 3);
            }
        }
    }
    #[test]
    fn absent_encoder_and_early_exit_do_not_leave_acceptors_waiting() {
        let p = Progress::default();
        let error = deadline(&p, || {
            encode(
                1,
                &p,
                |_, _| Command::new("cutcraft-deliberately-missing-encoder"),
                |_, _| Ok(()),
                |_, _| Ok(()),
            )
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("MEDIA_TOOL_NOT_FOUND"),
            "{error:#}"
        );
        if !available() {
            return;
        }
        let error = deadline(&p, || {
            encode(
                1,
                &p,
                |_, _| {
                    let mut c = Command::new("ffmpeg");
                    c.arg("-version");
                    c
                },
                |_, _| Ok(()),
                |_, _| Ok(()),
            )
        })
        .unwrap_err();
        assert!(error.to_string().contains("ENCODER_STOPPED"), "{error:#}");
    }
    #[test]
    fn producer_failure_stops_other_input_and_preserves_root_error() {
        if !available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let p = Progress::default();
        let error = deadline(&p, || {
            mp4(
                &fixture(),
                &tmp.path().join("fail.mp4"),
                true,
                &crate::encoding::EncodingOptions::default(),
                &p,
                |_, _| anyhow::bail!("SOURCE_INDEX_INVALID: test producer"),
                |writer, _| loop {
                    writer.write(&[0; 4096])?;
                },
            )
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("SOURCE_INDEX_INVALID"),
            "{error:#}"
        );
        assert!(
            !p.cancellation().is_cancelled(),
            "internal failure must not cancel the caller's token"
        );
    }
    #[test]
    fn cancellation_interrupts_socket_backpressure() {
        let listener = listener().unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let cancel = CancellationToken::default();
        let control = cancel.clone();
        let halt = AtomicBool::new(false);
        let mut socket = accept(listener, &cancel, &halt).unwrap().unwrap();
        let stop = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            control.cancel();
        });
        let error = write(&mut socket, &vec![0; 32 * 1024 * 1024], &cancel, &halt).unwrap_err();
        assert!(error.to_string().contains("JOB_CANCELLED"), "{error:#}");
        stop.join().unwrap();
        drop(client);
    }
}
