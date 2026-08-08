mod background;
mod easing;
mod geometry;
mod layout;
mod lrc;
mod lyrics_renderer;
mod renderer;
mod spring;
mod surface;
mod ttml;
mod video;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LyricFormat {
    Auto,
    Lrc,
    Ttml,
}

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
        background: Option<PathBuf>,
        #[arg(long)]
        no_audio: bool,
        #[arg(long, value_enum, default_value_t = LyricFormat::Auto)]
        format: LyricFormat,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Render {
            song,
            lyrics,
            output,
            width,
            height,
            fps,
            background,
            no_audio,
            format,
        } => render(
            song, lyrics, output, width, height, fps, background, no_audio, format,
        ),
    }
}

fn render(
    song: PathBuf,
    lyrics: PathBuf,
    output: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    background: Option<PathBuf>,
    no_audio: bool,
    format: LyricFormat,
) -> Result<()> {
    if width == 0 || height == 0 || fps == 0 {
        bail!("width, height, and fps must be positive");
    }
    let text =
        fs::read_to_string(&lyrics).with_context(|| format!("read lyrics {}", lyrics.display()))?;
    let is_ttml = match format {
        LyricFormat::Auto => {
            ttml::looks_like_ttml(&text)
                || lyrics
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(|extension| extension.eq_ignore_ascii_case("ttml"))
                    .unwrap_or(false)
        }
        LyricFormat::Lrc => false,
        LyricFormat::Ttml => true,
    };
    let lines = if is_ttml {
        ttml::parse_ttml(&text)?
    } else {
        lrc::parse_lrc(&text)?
    };
    let last_line_end_ms = lines.iter().map(|line| line.end_ms).max().unwrap_or(0);
    let audio_duration = if no_audio {
        0.0
    } else {
        video::audio_duration_seconds(&song)?
    };
    let duration_seconds = if no_audio {
        last_line_end_ms as f64 / 1_000.0 + 1.0
    } else {
        audio_duration
    };
    let total_frames = (duration_seconds * fps as f64).ceil() as u64;
    let background_layer = if let Some(path) = background.as_deref() {
        background::BackgroundRenderer::from_image_path(path)?
    } else {
        background::BackgroundRenderer::gradient()
    };
    let mut renderer =
        renderer::Renderer::with_background(width, height, fps, lines, background_layer)?;
    eprintln!("render backend: {}", renderer.backend_name());
    let mut writer = video::VideoWriter::new(
        &output,
        width,
        height,
        fps,
        (!no_audio).then_some(song.as_path()),
    )?;
    let mut next_progress = 0;
    let mut rgba = Vec::new();
    for frame in 0..total_frames {
        let t_ms = frame * 1_000 / fps as u64;
        renderer.render_frame_into(t_ms, &mut rgba)?;
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
