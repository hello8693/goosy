use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::LazyLock;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct AudioMetadata {
    pub duration_seconds: f64,
    pub title: Option<String>,
    pub lyrics: Option<String>,
}

fn probe_audio(song: &Path) -> Result<AudioMetadata> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:format_tags",
            "-of",
            "json",
        ])
        .arg(song)
        .output()
        .with_context(|| format!("run ffprobe for {}", song.display()))?;
    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            song.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse ffprobe JSON")?;
    let format = document
        .get("format")
        .context("ffprobe JSON has no format")?;
    let duration_seconds = format
        .get("duration")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .parse::<f64>()
        .with_context(|| format!("parse audio duration for {}", song.display()))?;
    let tags = format.get("tags").and_then(serde_json::Value::as_object);
    let find_tag = |names: &[&str]| {
        tags.and_then(|tags| {
            tags.iter()
                .find(|(key, _)| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
                .and_then(|(_, value)| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
    };
    Ok(AudioMetadata {
        duration_seconds,
        title: find_tag(&["title", "sort_name", "©nam"]),
        lyrics: find_tag(&["lyrics", "lyric", "©lyr"]),
    })
}

pub fn audio_metadata(song: &Path) -> Result<AudioMetadata> {
    probe_audio(song)
}

pub fn embedded_cover_image(song: &Path) -> Result<Option<Vec<u8>>> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(song)
        .args([
            "-map",
            "0:v:0?",
            "-an",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "pipe:1",
        ])
        .output()
        .with_context(|| format!("run ffmpeg to extract cover from {}", song.display()))?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    Ok(Some(output.stdout))
}

static FFMPEG_ENCODERS: LazyLock<Option<String>> = LazyLock::new(|| {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
});

fn encoder_available(name: &str) -> bool {
    FFMPEG_ENCODERS
        .as_deref()
        .is_some_and(|encoders| encoders.contains(name))
}

#[cfg(target_os = "macos")]
fn preferred_hardware_encoder() -> Option<&'static str> {
    encoder_available("h264_videotoolbox").then_some("h264_videotoolbox")
}

#[cfg(target_os = "windows")]
fn preferred_hardware_encoder() -> Option<&'static str> {
    ["h264_nvenc", "h264_amf", "h264_qsv"]
        .into_iter()
        .find(|encoder| windows_encoder_usable(encoder))
}

#[cfg(target_os = "windows")]
fn windows_encoder_usable(encoder: &str) -> bool {
    if !encoder_available(encoder) {
        return false;
    }
    // FFmpeg listing an encoder only proves that the build contains its wrapper.
    // Actually open it once so missing drivers (for example nvcuda.dll) fall
    // back to libx264 before the real raw-video pipe consumes any frames.
    Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:d=0.04",
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn preferred_hardware_encoder() -> Option<&'static str> {
    None
}

fn add_video_encoder(command: &mut Command, encoder: Option<&str>) {
    match encoder {
        Some("h264_videotoolbox") => command.args(["-c:v", "h264_videotoolbox", "-b:v", "8M"]),
        Some("h264_nvenc") => command.args(["-c:v", "h264_nvenc", "-preset", "p5", "-b:v", "8M"]),
        Some("h264_amf") => command.args(["-c:v", "h264_amf", "-quality", "quality", "-b:v", "8M"]),
        Some("h264_qsv") => command.args(["-c:v", "h264_qsv", "-b:v", "8M"]),
        _ => command.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"]),
    };
}

pub struct VideoWriter {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    command: String,
    _output: PathBuf,
}

impl VideoWriter {
    pub fn new(
        output: &Path,
        width: u32,
        height: u32,
        fps: u32,
        audio: Option<&Path>,
    ) -> Result<Self> {
        let mut command = Command::new("ffmpeg");
        command
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
            ])
            .args([
                "-s",
                &format!("{width}x{height}"),
                "-r",
                &fps.to_string(),
                "-i",
                "pipe:0",
            ]);
        if let Some(audio) = audio {
            command.arg("-i").arg(audio).args([
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-movflags",
                "+faststart",
                "-shortest",
            ]);
        } else {
            command.args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);
        }
        add_video_encoder(&mut command, preferred_hardware_encoder());
        command
            .arg(output)
            .stdin(Stdio::piped())
            // Never pipe FFmpeg stderr without a concurrent drain: on Windows
            // encoder diagnostics can fill the pipe before the first frame,
            // making the writer wait forever at GUI 0%.
            .stderr(Stdio::inherit());
        let command_display = format!("{command:?}");
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn ffmpeg: {command_display}"))?;
        let stdin = child.stdin.take().context("ffmpeg stdin was not piped")?;
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            command: command_display,
            _output: output.to_owned(),
        })
    }

    pub fn write_frame(&mut self, rgba: &[u8]) -> Result<()> {
        if let Err(error) = self
            .stdin
            .as_mut()
            .context("video writer is already finished")?
            .write_all(rgba)
        {
            self.stdin.take();
            let output = self
                .child
                .take()
                .context("ffmpeg child already finished")?
                .wait_with_output()
                .context("wait for failed ffmpeg")?;
            bail!(
                "write RGBA frame to ffmpeg: {error}\ncommand: {}\nstderr: {}",
                self.command,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.stdin.take();
        let output = self
            .child
            .take()
            .context("ffmpeg child already finished")?
            .wait_with_output()
            .context("wait for ffmpeg")?;
        if !output.status.success() {
            bail!(
                "ffmpeg failed ({})\ncommand: {}\nstderr: {}",
                output.status,
                self.command,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

const FRAME_QUEUE_CAPACITY: usize = 3;

/// Sends rendered frames to FFmpeg without making the render thread perform the
/// blocking pipe write. The queue is intentionally bounded to cap memory use and
/// preserve backpressure when encoding cannot keep up.
pub struct AsyncVideoWriter {
    sender: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    recycled: std::sync::mpsc::Receiver<Vec<u8>>,
    worker: Option<std::thread::JoinHandle<Result<()>>>,
    frame_size: usize,
}

impl AsyncVideoWriter {
    pub fn new(
        output: &Path,
        width: u32,
        height: u32,
        fps: u32,
        audio: Option<&Path>,
    ) -> Result<Self> {
        let writer = VideoWriter::new(output, width, height, fps, audio)?;
        let frame_size = width as usize * height as usize * 4;
        let (sender, frames) = std::sync::mpsc::sync_channel::<Vec<u8>>(FRAME_QUEUE_CAPACITY);
        let (recycle_sender, recycled) =
            std::sync::mpsc::sync_channel::<Vec<u8>>(FRAME_QUEUE_CAPACITY + 1);
        for _ in 0..=FRAME_QUEUE_CAPACITY {
            recycle_sender
                .send(vec![0; frame_size])
                .expect("recycle channel capacity matches initial buffers");
        }
        let worker = std::thread::Builder::new()
            .name("ffmpeg-writer".to_owned())
            .spawn(move || {
                let mut writer = writer;
                for frame in frames {
                    let write_result = writer.write_frame(&frame);
                    // The render thread drains this opportunistically while it is
                    // producing frames. During finish no receiver is active, so
                    // never let recycling block the encoder worker.
                    let _ = recycle_sender.try_send(frame);
                    write_result?;
                }
                writer.finish()
            })
            .context("spawn ffmpeg writer thread")?;
        Ok(Self {
            sender: Some(sender),
            recycled,
            worker: Some(worker),
            frame_size,
        })
    }

    pub fn submit_frame(&mut self, pixels: &mut Vec<u8>) -> Result<()> {
        if pixels.len() != self.frame_size {
            bail!(
                "invalid frame size: expected {}, got {}",
                self.frame_size,
                pixels.len()
            );
        }
        let frame = std::mem::take(pixels);
        let sender = self
            .sender
            .as_ref()
            .context("video writer is already finished")?;
        if let Err(error) = sender.send(frame) {
            *pixels = error.0;
            self.sender.take();
            bail!("send frame to ffmpeg writer: worker stopped")
        }
        *pixels = self
            .recycled
            .recv()
            .context("ffmpeg writer stopped before recycling a frame")?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.sender.take();
        let worker = self
            .worker
            .take()
            .context("ffmpeg writer already finished")?;
        match worker.join() {
            Ok(result) => result,
            Err(_) => bail!("ffmpeg writer thread panicked"),
        }
    }
}
