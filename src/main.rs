mod background;
mod easing;
mod layout;
mod lrc;
mod lyrics_renderer;
mod renderer;
mod spring;
mod video;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "goosy", about = "Render scrolling lyric videos")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Render {
        song: PathBuf,
        lyrics: PathBuf,
        #[arg(short, long, default_value = "out.mp4")]
        output: PathBuf,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        #[arg(long)]
        no_audio: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Render { song, lyrics, output, width, height, fps, no_audio } => {
            render(song, lyrics, output, width, height, fps, no_audio)
        }
    }
}

fn render(song: PathBuf, lyrics: PathBuf, output: PathBuf, width: u32, height: u32, fps: u32, no_audio: bool) -> Result<()> {
    if width == 0 || height == 0 || fps == 0 { bail!("width, height, and fps must be positive"); }
    let text = fs::read_to_string(&lyrics).with_context(|| format!("read lyrics {}", lyrics.display()))?;
    let lines = lrc::parse_lrc(&text)?;
    let last_line_end_ms = lines.iter().map(|line| line.end_ms).max().unwrap_or(0);
    let audio_duration = if no_audio { 0.0 } else { video::audio_duration_seconds(&song)? };
    let duration_seconds = if no_audio { last_line_end_ms as f64 / 1_000.0 + 1.0 } else { audio_duration };
    let total_frames = (duration_seconds * fps as f64).ceil() as u64;
    let mut renderer = renderer::Renderer::new(width, height, fps, lines)?;
    let mut writer = video::VideoWriter::new(&output, width, height, fps, (!no_audio).then_some(song.as_path()))?;
    let mut next_progress = 0;
    for frame in 0..total_frames {
        let t_ms = frame * 1_000 / fps as u64;
        let rgba = renderer.render_frame(t_ms)?;
        writer.write_frame(&rgba)?;
        let progress = ((frame + 1) * 100 / total_frames.max(1)) as u32;
        if progress >= next_progress {
            eprintln!("render: {progress}% ({}/{total_frames} frames)", frame + 1);
            next_progress = progress / 10 * 10 + 10;
        }
    }
    writer.finish()?;
    eprintln!("wrote {}", output.display());
    Ok(())
}
