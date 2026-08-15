use anyhow::Result;
#[cfg(target_os = "windows")]
use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use libgoosy::{LyricFormat, LyricsStyle, RenderControl, RenderOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
#[derive(Parser)]
#[command(
    name = "goosy-render-worker",
    about = "Isolated Goosy rendering worker"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
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
        #[arg(long, hide = true)]
        sample_start_ms: Option<u64>,
        #[arg(long, hide = true)]
        sample_duration_ms: Option<u64>,
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
        #[arg(long, hide = true)]
        preview_dir: Option<PathBuf>,
        #[arg(
            long,
            hide = true,
            default_value_t = 15,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        preview_interval: u64,
        #[arg(long, value_enum, default_value_t = WorkerLyricFormat::Auto)]
        format: WorkerLyricFormat,
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
        #[arg(long, value_enum, default_value_t = WorkerLyricFormat::Auto)]
        format: WorkerLyricFormat,
    },
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    BackendProbe {
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
    },
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    RendererProbe {
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        #[arg(long, default_value_t = 30)]
        fps: u32,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkerLyricFormat {
    Auto,
    Lrc,
    Ttml,
    Yrc,
}

impl From<WorkerLyricFormat> for LyricFormat {
    fn from(value: WorkerLyricFormat) -> Self {
        match value {
            WorkerLyricFormat::Auto => Self::Auto,
            WorkerLyricFormat::Lrc => Self::Lrc,
            WorkerLyricFormat::Ttml => Self::Ttml,
            WorkerLyricFormat::Yrc => Self::Yrc,
        }
    }
}

fn emit_stage(enabled: bool, key: &str, message: &str) -> Result<()> {
    if enabled {
        println!("GOOSY_STAGE {key} {message}");
        std::io::stdout().flush()?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn ensure_renderer_backend(width: u32, height: u32, fps: u32, progress_events: bool) -> Result<()> {
    let requested = std::env::var("GOOSY_RENDER_BACKEND")
        .unwrap_or_else(|_| "auto".to_owned())
        .trim()
        .to_ascii_lowercase();
    emit_stage(
        progress_events,
        "probing-render-backend",
        "正在隔离检测图形后端",
    )?;
    let executable = std::env::current_exe().context("locate rendering worker")?;
    let backend_probe = |backend: &str| -> Result<std::process::ExitStatus> {
        std::process::Command::new(&executable)
            .arg("backend-probe")
            .arg("--width")
            .arg(width.to_string())
            .arg("--height")
            .arg(height.to_string())
            .env("GOOSY_RENDER_BACKEND", backend)
            .status()
            .with_context(|| format!("run {backend} backend probe"))
    };
    let renderer_probe = |backend: &str| -> Result<std::process::ExitStatus> {
        emit_stage(
            progress_events,
            "probing-renderer-initialization",
            "正在隔离检测文本与首帧初始化",
        )?;
        std::process::Command::new(&executable)
            .arg("renderer-probe")
            .arg("--width")
            .arg(width.to_string())
            .arg("--height")
            .arg(height.to_string())
            .arg("--fps")
            .arg(fps.to_string())
            .env("GOOSY_RENDER_BACKEND", backend)
            .status()
            .with_context(|| format!("run {backend} renderer initialization probe"))
    };
    let automatic = requested == "auto";
    let requested_backend = requested.as_str();
    let candidates: &[&str] = if automatic {
        &["d3d12", "angle", "raster"]
    } else {
        std::slice::from_ref(&requested_backend)
    };
    for backend in candidates {
        let backend_status = backend_probe(backend)?;
        if !backend_status.success() {
            if !automatic {
                bail!("requested {backend} backend probe failed: {backend_status}");
            }
            eprintln!("goosy: {backend} backend probe failed ({backend_status}); trying fallback");
            continue;
        }
        let renderer_status = renderer_probe(backend)?;
        if !renderer_status.success() {
            if !automatic {
                bail!(
                    "requested {backend} renderer initialization probe failed: {renderer_status}"
                );
            }
            eprintln!(
                "goosy: {backend} renderer initialization probe failed ({renderer_status}); trying fallback"
            );
            continue;
        }
        // SAFETY: the worker is single-threaded before renderer construction.
        unsafe { std::env::set_var("GOOSY_RENDER_BACKEND", backend) };
        return Ok(());
    }
    bail!("all renderer backend probes failed")
}

#[cfg(not(target_os = "windows"))]
fn ensure_renderer_backend(
    _width: u32,
    _height: u32,
    _fps: u32,
    _progress_events: bool,
) -> Result<()> {
    Ok(())
}

const PREVIEW_MAX_WIDTH: u32 = 640;
const PREVIEW_MAX_HEIGHT: u32 = 360;

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    let scale = (PREVIEW_MAX_WIDTH as f64 / width.max(1) as f64)
        .min(PREVIEW_MAX_HEIGHT as f64 / height.max(1) as f64)
        .min(1.0);
    (
        (width as f64 * scale).round().max(1.0) as u32,
        (height as f64 * scale).round().max(1.0) as u32,
    )
}

fn downscale_rgba_nearest(
    source: &[u8],
    width: u32,
    height: u32,
    preview: &mut Vec<u8>,
) -> std::io::Result<(u32, u32)> {
    let source_size = width as usize * height as usize * 4;
    if source.len() != source_size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("expected {source_size} RGBA bytes, got {}", source.len()),
        ));
    }
    let (preview_width, preview_height) = preview_dimensions(width, height);
    preview.resize(preview_width as usize * preview_height as usize * 4, 0);
    for preview_y in 0..preview_height as usize {
        let source_y = preview_y * height as usize / preview_height as usize;
        for preview_x in 0..preview_width as usize {
            let source_x = preview_x * width as usize / preview_width as usize;
            let source_offset = (source_y * width as usize + source_x) * 4;
            let preview_offset = (preview_y * preview_width as usize + preview_x) * 4;
            preview[preview_offset..preview_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    Ok((preview_width, preview_height))
}

fn write_preview_frame(
    directory: &std::path::Path,
    frame: u64,
    source: &[u8],
    width: u32,
    height: u32,
    preview: &mut Vec<u8>,
) -> std::io::Result<(u32, u32)> {
    let dimensions = downscale_rgba_nearest(source, width, height, preview)?;
    std::fs::create_dir_all(directory)?;
    std::fs::write(directory.join(format!("{frame}.rgba")), preview)?;
    Ok(dimensions)
}

fn main() -> Result<()> {
    match Cli::parse().command {
        #[cfg(target_os = "windows")]
        Command::BackendProbe { width, height } => {
            libgoosy::surface::probe_selected_backend(width, height)?;
            Ok(())
        }
        #[cfg(target_os = "windows")]
        Command::RendererProbe { width, height, fps } => {
            libgoosy::renderer::probe_renderer_initialization(width, height, fps)?;
            Ok(())
        }
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
        } => {
            let metadata = libgoosy::video::audio_metadata(&song)?;
            let text = if let Some(path) = lyrics.as_deref() {
                std::fs::read_to_string(path)?
            } else {
                metadata.lyrics.ok_or_else(|| {
                    anyhow::anyhow!("no external lyrics path and no embedded lyrics metadata")
                })?
            };
            let lines = libgoosy::parse_lyrics(&text, format.into())?;
            let pages = libgoosy::render_lyrics_pdf(
                &output,
                &lines,
                &libgoosy::PdfOptions {
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
        Command::Render {
            song,
            lyrics,
            output,
            width,
            height,
            fps,
            font_scale,
            line_height_scale,
            line_spacing_scale,
            translation_gap_scale,
            background_gap_scale,
            horizontal_padding_scale,
            debug_overlays,
            sample_start_ms,
            sample_duration_ms,
            background,
            cover,
            title,
            no_embedded_cover,
            no_audio,
            no_translation,
            no_background_vocal,
            exclude_lines,
            progress_events,
            preview_dir,
            preview_interval,
            format,
        } => {
            ensure_renderer_backend(width, height, fps, progress_events)?;
            emit_stage(
                progress_events,
                "initializing-renderer",
                "正在初始化图形后端",
            )?;
            let options = RenderOptions {
                song,
                lyrics,
                output,
                width,
                height,
                fps,
                lyrics_style: LyricsStyle {
                    font_scale,
                    line_height_scale,
                    group_gap_scale: line_spacing_scale,
                    translation_gap_scale,
                    background_gap_scale,
                    horizontal_padding_scale,
                    debug_overlays,
                },
                background,
                cover,
                title,
                no_embedded_cover,
                no_audio,
                render_translation: !no_translation,
                render_background_vocal: !no_background_vocal,
                excluded_lines: exclude_lines,
                format: format.into(),
                sample_start_ms,
                sample_duration_ms,
            };
            let mut preview_dir = preview_dir;
            let mut preview_pixels = Vec::new();
            let (control_sender, control_receiver) = mpsc::channel();
            thread::spawn(move || {
                let stdin = std::io::stdin();
                for command in stdin.lock().lines().map_while(Result::ok) {
                    let _ = control_sender.send(command.trim().to_owned());
                }
            });
            let mut paused = false;
            libgoosy::render_with_frame_progress_control(
                &options,
                |done, total, elapsed, pixels| {
                    if progress_events {
                        println!("GOOSY_PROGRESS {done} {total} {elapsed:.3}");
                    }
                    let preview_due = done == 1 || done == total || done % preview_interval == 0;
                    if preview_due {
                        let result = preview_dir.as_deref().map(|directory| {
                            write_preview_frame(
                                directory,
                                done,
                                pixels,
                                width,
                                height,
                                &mut preview_pixels,
                            )
                        });
                        match result {
                            Some(Ok((preview_width, preview_height))) => {
                                println!("GOOSY_PREVIEW {done} {preview_width} {preview_height}");
                            }
                            Some(Err(error)) => {
                                eprintln!(
                                    "goosy: disable live preview after write failure: {error}"
                                );
                                preview_dir = None;
                            }
                            None => {}
                        }
                    }
                    if progress_events || preview_due {
                        let _ = std::io::stdout().flush();
                    }
                },
                || loop {
                    if paused {
                        match control_receiver.recv() {
                            Ok(command) if command == "resume" => paused = false,
                            Ok(command) if command == "stop" => return RenderControl::Stop,
                            Ok(_) => {}
                            Err(_) => return RenderControl::Stop,
                        }
                    } else {
                        match control_receiver.try_recv() {
                            Ok(command) if command == "pause" => paused = true,
                            Ok(command) if command == "stop" => return RenderControl::Stop,
                            Ok(_) | Err(mpsc::TryRecvError::Empty) => {
                                return RenderControl::Continue;
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                return RenderControl::Continue;
                            }
                        }
                    }
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{downscale_rgba_nearest, preview_dimensions};

    #[test]
    fn preview_dimensions_fit_within_bounds_and_preserve_aspect_ratio() {
        assert_eq!(preview_dimensions(1_920, 1_080), (640, 360));
        assert_eq!(preview_dimensions(1_080, 1_920), (203, 360));
        assert_eq!(preview_dimensions(320, 180), (320, 180));
    }

    #[test]
    fn nearest_preview_uses_pixels_from_the_expected_source_cells() {
        let source = vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let mut preview = Vec::new();
        let dimensions = downscale_rgba_nearest(&source, 2, 2, &mut preview).unwrap();

        assert_eq!(dimensions, (2, 2));
        assert_eq!(preview, source);
    }
}
