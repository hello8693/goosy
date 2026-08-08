use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

pub fn audio_duration_seconds(song: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
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
    let value = String::from_utf8(output.stdout).context("ffprobe output is not UTF-8")?;
    value
        .trim()
        .parse::<f64>()
        .with_context(|| format!("parse audio duration: {:?}", value.trim()))
}

#[cfg(target_os = "macos")]
fn video_toolbox_available() -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("h264_videotoolbox"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn video_toolbox_available() -> bool {
    false
}

fn add_video_encoder(command: &mut Command, use_hardware: bool) {
    if use_hardware {
        command.args(["-c:v", "h264_videotoolbox", "-b:v", "8M"]);
    } else {
        command.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"]);
    }
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
        add_video_encoder(&mut command, video_toolbox_available());
        command
            .arg(output)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped());
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
