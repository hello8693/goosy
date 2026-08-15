pub mod background;
pub mod cover_renderer;
pub mod easing;
pub mod ffi;
pub mod geometry;
pub mod layout;
pub mod lrc;
pub mod lyrics_renderer;
pub mod pdf_renderer;
pub mod renderer;
pub mod spring;
pub mod surface;
pub mod ttml;
pub mod video;
pub mod yrc;

// Prefer the discrete adapter on Windows hybrid-GPU systems. NVIDIA Optimus and
// AMD PowerXpress inspect these exports before the GPU backend creates its device.
#[cfg(target_os = "windows")]
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: u32 = 1;

#[cfg(target_os = "windows")]
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: u32 = 1;

pub use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
pub use lrc::{BackgroundVocal, LyricLine, LyricWord};
pub use lyrics_renderer::LyricsStyle;
pub use pdf_renderer::{PdfOptions, render_lyrics_pdf};
pub use renderer::Renderer;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LyricFormat {
    #[default]
    Auto,
    Lrc,
    Ttml,
    Yrc,
}

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub song: PathBuf,
    pub lyrics: Option<PathBuf>,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub background: Option<PathBuf>,
    pub cover: Option<PathBuf>,
    pub title: Option<String>,
    pub no_embedded_cover: bool,
    pub no_audio: bool,
    pub render_translation: bool,
    pub lyrics_style: LyricsStyle,
    pub render_background_vocal: bool,
    pub excluded_lines: Vec<usize>,
    pub format: LyricFormat,
}

impl RenderOptions {
    pub fn new(song: impl Into<PathBuf>, output: impl Into<PathBuf>) -> Self {
        Self {
            song: song.into(),
            lyrics: None,
            output: output.into(),
            width: 1_920,
            height: 1_080,
            fps: 30,
            background: None,
            cover: None,
            title: None,
            no_embedded_cover: false,
            no_audio: false,
            render_translation: true,
            lyrics_style: LyricsStyle::default(),
            render_background_vocal: true,
            excluded_lines: Vec::new(),
            format: LyricFormat::Auto,
        }
    }
}

pub fn parse_lyrics(input: &str, format: LyricFormat) -> anyhow::Result<Vec<LyricLine>> {
    match format {
        LyricFormat::Auto if ttml::looks_like_ttml(input) => ttml::parse_ttml(input),
        LyricFormat::Auto if yrc::looks_like_yrc(input) => yrc::parse_yrc(input),
        LyricFormat::Auto | LyricFormat::Lrc => lrc::parse_lrc(input),
        LyricFormat::Ttml => ttml::parse_ttml(input),
        LyricFormat::Yrc => yrc::parse_yrc(input),
    }
}

pub fn render(options: &RenderOptions) -> anyhow::Result<()> {
    render_with_progress(options, |_, _, _| {})
}

pub fn render_with_progress<F>(options: &RenderOptions, mut on_progress: F) -> anyhow::Result<()>
where
    F: FnMut(u64, u64, f64),
{
    render_with_frame_progress(options, |done, total, elapsed, _pixels| {
        on_progress(done, total, elapsed);
    })
}

pub fn render_with_frame_progress<F>(options: &RenderOptions, mut on_frame: F) -> anyhow::Result<()>
where
    F: FnMut(u64, u64, f64, &[u8]),
{
    if options.width == 0 || options.height == 0 || options.fps == 0 {
        anyhow::bail!("width, height, and fps must be positive");
    }
    let metadata = video::audio_metadata(&options.song)?;
    let title = options.title.clone().or(metadata.title.clone());
    let text = if let Some(lyrics) = options.lyrics.as_deref() {
        fs::read_to_string(lyrics)
            .map_err(|error| anyhow::anyhow!("read lyrics {}: {error}", lyrics.display()))?
    } else {
        metadata.lyrics.clone().ok_or_else(|| {
            anyhow::anyhow!("no external lyrics path and no embedded lyrics metadata")
        })?
    };
    let lines = parse_lyrics(&text, options.format)?;
    let last_line_end_ms = lines.iter().map(|line| line.end_ms).max().unwrap_or(0);
    let duration_seconds = if options.no_audio {
        last_line_end_ms as f64 / 1_000.0 + 1.0
    } else {
        metadata.duration_seconds
    };
    let total_frames = (duration_seconds * options.fps as f64).ceil() as u64;
    let embedded_cover = if options.cover.is_none() && !options.no_embedded_cover {
        video::embedded_cover_image(&options.song)?
    } else {
        None
    };
    let background_layer = if let Some(path) = options.background.as_deref() {
        background::BackgroundRenderer::from_image_path(path)?
    } else if let Some(path) = options.cover.as_deref() {
        background::BackgroundRenderer::from_image_path(path)?
    } else if let Some(bytes) = embedded_cover.as_deref() {
        background::BackgroundRenderer::from_image_bytes(bytes)?
    } else {
        background::BackgroundRenderer::dynamic()
    };
    let cover_layer = if let Some(path) = options.cover.as_deref() {
        Some(cover_renderer::CoverRenderer::from_path(path, title)?)
    } else if let Some(bytes) = embedded_cover.as_deref() {
        Some(cover_renderer::CoverRenderer::from_bytes(bytes, title)?)
    } else {
        None
    };
    let mut renderer = Renderer::with_scene_options(
        options.width,
        options.height,
        options.fps,
        lines,
        background_layer,
        cover_layer,
        options.render_translation,
        options.render_background_vocal,
        options.lyrics_style,
        &options.excluded_lines,
    )?;
    let mut writer = video::AsyncVideoWriter::new(
        &options.output,
        options.width,
        options.height,
        options.fps,
        (!options.no_audio).then_some(options.song.as_path()),
    )?;
    let progress = ProgressBar::hidden();
    progress.set_style(ProgressStyle::default_bar());
    let mut rgba = Vec::new();
    let render_started = Instant::now();
    for frame in 0..total_frames {
        let t_ms = frame * 1_000 / options.fps as u64;
        renderer.render_frame_into(t_ms, &mut rgba)?;
        progress.inc(1);
        on_frame(
            frame + 1,
            total_frames,
            render_started.elapsed().as_secs_f64(),
            &rgba,
        );
        writer.submit_frame(&mut rgba)?;
    }
    writer.finish()?;
    progress.finish_and_clear();
    Ok(())
}

pub fn read_lyrics_file(
    path: impl AsRef<Path>,
    format: LyricFormat,
) -> anyhow::Result<Vec<LyricLine>> {
    let text = fs::read_to_string(path)?;
    parse_lyrics(&text, format)
}
