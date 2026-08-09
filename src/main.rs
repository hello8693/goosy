mod background;
mod cover_renderer;
mod easing;
mod geometry;
mod gui;
mod layout;
mod lrc;
mod lyrics_renderer;
mod renderer;
mod spring;
mod surface;
mod ttml;
mod video;
mod yrc;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LyricFormat {
    Auto,
    Lrc,
    Ttml,
    Yrc,
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
        #[arg(value_name = "LYRICS")]
        lyrics: Option<PathBuf>,
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
        cover: Option<PathBuf>,
        #[arg(long, visible_alias = "song-name")]
        title: Option<String>,
        #[arg(long)]
        no_embedded_cover: bool,
        #[arg(long)]
        no_audio: bool,
        #[arg(long, hide = true)]
        progress_events: bool,
        #[arg(long, value_enum, default_value_t = LyricFormat::Auto)]
        format: LyricFormat,
    },
    Gui,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Gui => gui::run().map_err(|error| anyhow::anyhow!(error.to_string())),
        Command::Render {
            song,
            lyrics,
            output,
            width,
            height,
            fps,
            background,
            cover,
            title,
            no_embedded_cover,
            no_audio,
            progress_events,
            format,
        } => render(
            song,
            lyrics,
            output,
            width,
            height,
            fps,
            background,
            cover,
            title,
            no_embedded_cover,
            no_audio,
            progress_events,
            format,
        ),
    }
}

fn render(
    song: PathBuf,
    lyrics: Option<PathBuf>,

    output: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    background: Option<PathBuf>,
    cover: Option<PathBuf>,
    title: Option<String>,
    no_embedded_cover: bool,
    no_audio: bool,
    progress_events: bool,
    format: LyricFormat,
) -> Result<()> {
    if width == 0 || height == 0 || fps == 0 {
        bail!("width, height, and fps must be positive");
    }
    let metadata = video::audio_metadata(&song)?;
    let title = title.or(metadata.title.clone());
    let text = if let Some(lyrics) = lyrics.as_deref() {
        fs::read_to_string(lyrics).with_context(|| format!("read lyrics {}", lyrics.display()))?
    } else {
        metadata
            .lyrics
            .clone()
            .context("no external lyrics path and no embedded lyrics metadata")?
    };
    let lines = match format {
        LyricFormat::Auto if ttml::looks_like_ttml(&text) => ttml::parse_ttml(&text)?,
        LyricFormat::Auto
            if yrc::looks_like_yrc(&text)
                || lyrics
                    .as_deref()
                    .and_then(|path| path.extension())
                    .and_then(|extension| extension.to_str())
                    .map(|extension| extension.eq_ignore_ascii_case("yrc"))
                    .unwrap_or(false) =>
        {
            yrc::parse_yrc(&text)?
        }
        LyricFormat::Auto | LyricFormat::Lrc => lrc::parse_lrc(&text)?,
        LyricFormat::Ttml => ttml::parse_ttml(&text)?,
        LyricFormat::Yrc => yrc::parse_yrc(&text)?,
    };
    let last_line_end_ms = lines.iter().map(|line| line.end_ms).max().unwrap_or(0);
    let audio_duration = if no_audio {
        0.0
    } else {
        metadata.duration_seconds
    };
    let duration_seconds = if no_audio {
        last_line_end_ms as f64 / 1_000.0 + 1.0
    } else {
        audio_duration
    };
    let total_frames = (duration_seconds * fps as f64).ceil() as u64;
    let embedded_cover = if cover.is_none() && !no_embedded_cover {
        video::embedded_cover_image(&song)?
    } else {
        None
    };
    let background_layer = if let Some(path) = background.as_deref() {
        background::BackgroundRenderer::from_image_path(path)?
    } else if let Some(path) = cover.as_deref() {
        background::BackgroundRenderer::from_image_path(path)?
    } else if let Some(bytes) = embedded_cover.as_deref() {
        background::BackgroundRenderer::from_image_bytes(bytes)?
    } else {
        background::BackgroundRenderer::dynamic()
    };
    let cover_layer = if let Some(path) = cover.as_deref() {
        Some(cover_renderer::CoverRenderer::from_path(path, title)?)
    } else if let Some(bytes) = embedded_cover.as_deref() {
        Some(cover_renderer::CoverRenderer::from_bytes(bytes, title)?)
    } else {
        None
    };
    let mut renderer =
        renderer::Renderer::with_scene(width, height, fps, lines, background_layer, cover_layer)?;
    eprintln!("render backend: {}", renderer.backend_name());
    let mut writer = video::VideoWriter::new(
        &output,
        width,
        height,
        fps,
        (!no_audio).then_some(song.as_path()),
    )?;
    let progress = ProgressBar::new(total_frames);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} {percent:>3}% ETA {eta}",
        )
        .context("configure render progress bar")?
        .progress_chars("━━╸"),
    );
    progress.set_message(format!("Rendering {}", output.display()));
    progress.enable_steady_tick(Duration::from_millis(100));
    let mut rgba = Vec::new();
    let render_started = Instant::now();
    let progress_interval = (fps as u64 / 10).max(1);
    for frame in 0..total_frames {
        let t_ms = frame * 1_000 / fps as u64;
        renderer.render_frame_into(t_ms, &mut rgba)?;
        writer.write_frame(&rgba)?;
        progress.inc(1);
        if progress_events && ((frame + 1) % progress_interval == 0 || frame + 1 == total_frames) {
            println!(
                "GOOSY_PROGRESS {} {} {:.3}",
                frame + 1,
                total_frames,
                render_started.elapsed().as_secs_f64()
            );
            std::io::stdout().flush()?;
        }
    }
    writer.finish()?;
    progress.finish_with_message(format!("Rendered {}", output.display()));
    Ok(())
}
