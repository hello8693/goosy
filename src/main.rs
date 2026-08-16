mod gui;

#[cfg(not(target_os = "windows"))]
use libgoosy::{
    LyricsStyle, background, cover_renderer, lrc, pdf_renderer, renderer, ttml, video, yrc,
};

use anyhow::Result;
#[cfg(not(target_os = "windows"))]
use anyhow::{Context, bail};
#[cfg(not(target_os = "windows"))]
use clap::ValueEnum;
use clap::{Parser, Subcommand};
#[cfg(not(target_os = "windows"))]
use indicatif::{ProgressBar, ProgressStyle};
#[cfg(not(target_os = "windows"))]
use std::fs;
#[cfg(not(target_os = "windows"))]
use std::io::Write;
#[cfg(not(target_os = "windows"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "windows"))]
use std::time::{Duration, Instant};

#[cfg(not(target_os = "windows"))]
#[derive(Clone, Copy, Debug, ValueEnum)]
enum LyricFormat {
    Auto,
    Lrc,
    Ttml,
    Yrc,
}
#[cfg(target_os = "windows")]
#[derive(Parser)]
#[command(name = "goosy", about = "GoosyRenderer GUI")]
struct WindowsCli {
    #[command(subcommand)]
    command: Option<WindowsCommand>,
}

#[cfg(target_os = "windows")]
#[derive(Subcommand)]
enum WindowsCommand {
    Gui,
}

#[cfg(target_os = "windows")]
fn main() -> Result<()> {
    match WindowsCli::parse().command.unwrap_or(WindowsCommand::Gui) {
        WindowsCommand::Gui => gui::run().map_err(|error| anyhow::anyhow!(error.to_string())),
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Parser, Debug)]
#[command(name = "goosy", about = "Render scrolling lyric videos")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
#[cfg(not(target_os = "windows"))]
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
        #[arg(long, default_value_t = 1.0)]
        font_scale: f32,
        #[arg(long, default_value_t = 0.5)]
        translation_font_scale: f32,
        #[arg(long, default_value_t = 0.7)]
        background_font_scale: f32,
        #[arg(long, default_value_t = 1.0)]
        line_height_scale: f32,
        #[arg(long, default_value_t = 1.0)]
        line_spacing_scale: f32,
        #[arg(long, default_value_t = 1.0)]
        translation_gap_scale: f32,
        #[arg(long, default_value_t = 1.0)]
        background_gap_scale: f32,
        #[arg(long, default_value_t = 1.0)]
        horizontal_padding_scale: f32,
        #[arg(long, help = "draw lyric containers, glyph boxes, and gaps")]
        debug_overlays: bool,
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
        #[arg(long)]
        no_translation: bool,
        #[arg(long)]
        no_background_vocal: bool,
        #[arg(long = "exclude-line", value_delimiter = ',')]
        exclude_lines: Vec<usize>,
        #[arg(long, hide = true)]
        progress_events: bool,
        #[arg(long, value_enum, default_value_t = LyricFormat::Auto)]
        format: LyricFormat,
    },
    Pdf {
        song: PathBuf,
        #[arg(value_name = "LYRICS")]
        lyrics: Option<PathBuf>,
        #[arg(short, long, default_value = "lyrics.pdf")]
        output: PathBuf,
        #[arg(long, visible_alias = "song-name")]
        title: Option<String>,
        #[arg(long)]
        no_translation: bool,
        #[arg(long)]
        no_background_vocal: bool,
        #[arg(long)]
        speed_printer: bool,
        #[arg(long = "exclude-line", value_delimiter = ',')]
        exclude_lines: Vec<usize>,
        #[arg(long, value_enum, default_value_t = LyricFormat::Auto)]
        format: LyricFormat,
    },
    Gui,
}

#[cfg(not(target_os = "windows"))]
fn main() -> Result<()> {
    let command = if std::env::args_os().nth(1).is_none() {
        Command::Gui
    } else {
        Cli::parse().command
    };
    match command {
        Command::Gui => gui::run().map_err(|error| anyhow::anyhow!(error.to_string())),
        Command::Pdf {
            song,
            lyrics,
            output,
            title,
            no_translation,
            no_background_vocal,
            speed_printer,
            exclude_lines,
            format,
        } => export_pdf(
            song,
            lyrics,
            output,
            title,
            no_translation,
            no_background_vocal,
            speed_printer,
            exclude_lines,
            format,
        ),
        Command::Render {
            song,
            lyrics,
            output,
            width,
            height,
            fps,
            font_scale,
            translation_font_scale,
            background_font_scale,
            line_height_scale,
            line_spacing_scale,
            translation_gap_scale,
            background_gap_scale,
            horizontal_padding_scale,
            debug_overlays,
            background,
            cover,
            title,
            no_embedded_cover,
            no_audio,
            progress_events,
            no_translation,
            no_background_vocal,
            exclude_lines,
            format,
        } => render(
            song,
            lyrics,
            output,
            width,
            height,
            fps,
            font_scale,
            translation_font_scale,
            background_font_scale,
            line_height_scale,
            line_spacing_scale,
            translation_gap_scale,
            background_gap_scale,
            horizontal_padding_scale,
            debug_overlays,
            background,
            cover,
            title,
            no_embedded_cover,
            no_audio,
            progress_events,
            no_translation,
            no_background_vocal,
            exclude_lines,
            format,
        ),
    }
}

#[cfg(not(target_os = "windows"))]
fn parse_lyrics_text(
    text: &str,
    lyrics: Option<&Path>,
    format: LyricFormat,
) -> Result<Vec<lrc::LyricLine>> {
    let extension = lyrics
        .and_then(Path::extension)
        .and_then(|extension| extension.to_str());
    match format {
        LyricFormat::Auto
            if ttml::looks_like_ttml(text)
                || extension.is_some_and(|value| value.eq_ignore_ascii_case("ttml")) =>
        {
            ttml::parse_ttml(text)
        }
        LyricFormat::Auto
            if yrc::looks_like_yrc(text)
                || extension.is_some_and(|value| value.eq_ignore_ascii_case("yrc")) =>
        {
            yrc::parse_yrc(text)
        }
        LyricFormat::Auto | LyricFormat::Lrc => lrc::parse_lrc(text),
        LyricFormat::Ttml => ttml::parse_ttml(text),

        LyricFormat::Yrc => yrc::parse_yrc(text),
    }
}

#[cfg(not(target_os = "windows"))]
fn emit_render_stage(enabled: bool, key: &str, message: &str) -> Result<()> {
    if enabled {
        println!("GOOSY_STAGE {key} {message}");
        std::io::stdout().flush()?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn render(
    song: PathBuf,
    lyrics: Option<PathBuf>,

    output: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    font_scale: f32,
    translation_font_scale: f32,
    background_font_scale: f32,
    line_height_scale: f32,
    line_spacing_scale: f32,
    translation_gap_scale: f32,
    background_gap_scale: f32,
    horizontal_padding_scale: f32,
    debug_overlays: bool,
    background: Option<PathBuf>,
    cover: Option<PathBuf>,
    title: Option<String>,
    no_embedded_cover: bool,
    no_audio: bool,
    progress_events: bool,
    no_translation: bool,
    no_background_vocal: bool,
    exclude_lines: Vec<usize>,
    format: LyricFormat,
) -> Result<()> {
    if width == 0 || height == 0 || fps == 0 {
        bail!("width, height, and fps must be positive");
    }
    emit_render_stage(progress_events, "probing-audio", "正在读取音频信息")?;
    let metadata = video::audio_metadata(&song)
        .with_context(|| format!("读取音频信息：{}", song.display()))?;
    emit_render_stage(progress_events, "loading-lyrics", "正在读取歌词")?;
    let title = title.or(metadata.title.clone());
    let text = if let Some(lyrics) = lyrics.as_deref() {
        fs::read_to_string(lyrics).with_context(|| format!("read lyrics {}", lyrics.display()))?
    } else {
        metadata
            .lyrics
            .clone()
            .context("no external lyrics path and no embedded lyrics metadata")?
    };
    let lines = parse_lyrics_text(&text, lyrics.as_deref(), format)?;
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
    emit_render_stage(progress_events, "loading-cover", "正在提取封面")?;
    let embedded_cover = if cover.is_none() && !no_embedded_cover {
        video::embedded_cover_image(&song)
            .with_context(|| format!("提取音频内嵌封面：{}", song.display()))?
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
    #[cfg(target_os = "windows")]
    ensure_windows_backend_is_healthy(width, height, progress_events)?;
    let cover_layer = if let Some(path) = cover.as_deref() {
        Some(cover_renderer::CoverRenderer::from_path(path, title)?)
    } else if let Some(bytes) = embedded_cover.as_deref() {
        Some(cover_renderer::CoverRenderer::from_bytes(bytes, title)?)
    } else {
        None
    };
    #[cfg(target_os = "windows")]
    ensure_windows_renderer_is_healthy(width, height, fps, progress_events)?;
    emit_render_stage(
        progress_events,
        "initializing-renderer",
        "正在初始化图形后端",
    )?;
    let mut renderer = renderer::Renderer::with_scene_options(
        width,
        height,
        fps,
        lines,
        background_layer,
        cover_layer,
        !no_translation,
        !no_background_vocal,
        LyricsStyle {
            font_scale,
            translation_font_scale,
            background_font_scale,
            line_height_scale,
            group_gap_scale: line_spacing_scale,
            translation_gap_scale,
            background_gap_scale,
            horizontal_padding_scale,
            debug_overlays,
        },
        &exclude_lines,
    )
    .context("初始化图形渲染器")?;
    eprintln!(
        "render backend: {} ({})",
        renderer.backend_name(),
        renderer.backend_details()
    );
    emit_render_stage(progress_events, "starting-ffmpeg", "正在启动 FFmpeg")?;
    let mut writer = video::AsyncVideoWriter::new(
        &output,
        width,
        height,
        fps,
        (!no_audio).then_some(song.as_path()),
    )
    .with_context(|| format!("启动 FFmpeg 输出：{}", output.display()))?;
    emit_render_stage(progress_events, "rendering-first-frame", "正在渲染首帧")?;
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
        renderer
            .render_frame_into(t_ms, &mut rgba)
            .with_context(|| format!("渲染第 {} 帧（{} ms）", frame + 1, t_ms))?;
        writer
            .submit_frame(&mut rgba)
            .with_context(|| format!("向 FFmpeg 提交第 {} 帧", frame + 1))?;
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
#[cfg(not(target_os = "windows"))]
fn export_pdf(
    song: PathBuf,
    lyrics: Option<PathBuf>,
    output: PathBuf,
    title: Option<String>,
    no_translation: bool,
    no_background_vocal: bool,
    speed_printer: bool,
    exclude_lines: Vec<usize>,
    format: LyricFormat,
) -> Result<()> {
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
    let lines = parse_lyrics_text(&text, lyrics.as_deref(), format)?;
    let pages = pdf_renderer::render_lyrics_pdf(
        &output,
        &lines,
        &pdf_renderer::PdfOptions {
            title,
            render_translation: !no_translation,
            render_background_vocal: !no_background_vocal,
            speed_printer,
            excluded_lines: exclude_lines,
        },
    )?;
    eprintln!("wrote {pages} PDF page(s): {}", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_ttml_extension_fallback() {
        let input = r#"<body><p begin="1s" end="2s">hello</p></body>"#;
        let lines =
            parse_lyrics_text(input, Some(Path::new("lyrics.ttml")), LyricFormat::Auto).unwrap();
        assert_eq!(lines[0].text, "hello");
    }
}
