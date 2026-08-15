use eframe::egui;
use libgoosy::{
    LyricFormat, LyricsStyle, RenderControl, RenderOptions, background, cover_renderer,
    geometry::FrameGeometry, layout::Layout, lrc, lyrics_renderer::LyricsRenderer, video,
};
use rfd::FileDialog;
use std::collections::VecDeque;
use std::io::Write;
#[cfg(target_os = "windows")]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderLifecycle {
    Idle,
    Running,
    Paused,
    EmergencyStopped,
}

struct InternalRenderTask {
    control: mpsc::Sender<String>,
    done: mpsc::Receiver<anyhow::Result<()>>,
    thread: Option<JoinHandle<()>>,
}

const DEFAULT_PREVIEW_INTERVAL: u64 = 15;
static PREVIEW_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const RENDER_TWO_COLUMN_MIN_WIDTH: f32 = 760.0;

fn render_page_uses_columns(available_width: f32) -> bool {
    available_width >= RENDER_TWO_COLUMN_MIN_WIDTH
}

fn style_percent_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::Slider::new(value, range).suffix("%").show_value(true));
    });
}

const STYLE_PREVIEW_MAX_WIDTH: u32 = 640;
const STYLE_PREVIEW_MAX_HEIGHT: u32 = 360;
const STYLE_PREVIEW_TIME_MS: u64 = 2_500;

#[derive(Clone, Debug, Eq, PartialEq)]
struct StylePreviewScene {
    width: u32,
    height: u32,
    title: String,
    cover_path: Option<PathBuf>,
    embedded_cover_audio: Option<PathBuf>,
    render_translation: bool,
    render_background_vocal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StylePreviewKey {
    style: [u32; 7],
    scene: StylePreviewScene,
}

fn style_preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    let scale = (STYLE_PREVIEW_MAX_WIDTH as f64 / width.max(1) as f64)
        .min(STYLE_PREVIEW_MAX_HEIGHT as f64 / height.max(1) as f64)
        .min(1.0);
    (
        (width as f64 * scale).round().max(1.0) as u32,
        (height as f64 * scale).round().max(1.0) as u32,
    )
}

fn style_preview_lines() -> Vec<lrc::LyricLine> {
    vec![
        lrc::LyricLine {
            start_ms: 0,
            end_ms: 1_500,
            text: "上一行歌词 · Previous line".to_owned(),
            translation: None,
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: None,
            words: Vec::new(),
        },
        lrc::LyricLine {
            start_ms: 1_500,
            end_ms: 4_500,
            text: "正在演唱的歌词，用于预览换行与行高".to_owned(),
            translation: Some("Live style preview for spacing and line height".to_owned()),
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: Some(lrc::BackgroundVocal {
                start_ms: 2_000,
                end_ms: 3_500,
                text: "伴唱歌词 · Background vocal".to_owned(),
                translation: None,
                words: vec![lrc::LyricWord {
                    start_ms: 2_000,
                    end_ms: 3_500,
                    text: "伴唱歌词 · Background vocal".to_owned(),
                }],
            }),
            words: Vec::new(),
        },
        lrc::LyricLine {
            start_ms: 4_500,
            end_ms: 6_500,
            text: "下一行歌词 · Next line".to_owned(),
            translation: None,
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: None,
            words: Vec::new(),
        },
    ]
}

fn load_style_preview_cover(scene: &StylePreviewScene) -> Result<Option<Vec<u8>>, String> {
    if let Some(path) = scene.cover_path.as_deref() {
        return std::fs::read(path)
            .map(Some)
            .map_err(|error| format!("读取预览封面 {} 失败：{error}", path.display()));
    }
    let Some(audio) = scene.embedded_cover_audio.as_deref() else {
        return Ok(None);
    };
    video::embedded_cover_image(audio).map_err(|error| error.to_string())
}

fn render_style_preview(
    style: LyricsStyle,
    scene: &StylePreviewScene,
    cover_bytes: Option<&[u8]>,
) -> Result<egui::ColorImage, String> {
    use skia_safe::{AlphaType, ColorType, IPoint, ImageInfo, surfaces};

    let output_width = scene.width.max(1);
    let output_height = scene.height.max(1);
    let preview_dimensions = style_preview_dimensions(output_width, output_height);
    let preview_size = (preview_dimensions.0 as i32, preview_dimensions.1 as i32);
    let geometry = FrameGeometry::for_frame(output_width, output_height);
    let lines = style_preview_lines();
    let renderer = LyricsRenderer::new_with_options(
        &lines,
        geometry.lyrics,
        scene.render_translation,
        scene.render_background_vocal,
        style,
    )
    .map_err(|error| error.to_string())?;
    let mut layout = Layout::new(
        &lines,
        geometry.lyrics.height,
        renderer.group_heights(),
        renderer.group_gap(),
        renderer.interlude_slot_height(),
    );
    layout.update(&lines, STYLE_PREVIEW_TIME_MS, 30);
    let mut background = if let Some(bytes) = cover_bytes {
        background::BackgroundRenderer::from_image_bytes(bytes)
            .map_err(|error| error.to_string())?
    } else {
        background::BackgroundRenderer::dynamic()
    };
    let mut cover = cover_bytes
        .map(|bytes| cover_renderer::CoverRenderer::from_bytes(bytes, Some(scene.title.clone())))
        .transpose()
        .map_err(|error| error.to_string())?;

    let mut surface = surfaces::raster_n32_premul(preview_size)
        .ok_or_else(|| "无法创建样式预览画布".to_owned())?;
    let canvas = surface.canvas();
    canvas.clear(skia_safe::Color::BLACK);
    let saved = canvas.save();
    canvas.scale((
        preview_dimensions.0 as f32 / output_width as f32,
        preview_dimensions.1 as f32 / output_height as f32,
    ));
    background
        .draw(canvas, &geometry, STYLE_PREVIEW_TIME_MS)
        .map_err(|error| error.to_string())?;
    if let Some(cover) = &mut cover {
        cover
            .draw(canvas, &geometry)
            .map_err(|error| error.to_string())?;
    }
    renderer
        .draw(
            canvas,
            &lines,
            &layout,
            STYLE_PREVIEW_TIME_MS,
            geometry.lyrics.height as u32,
        )
        .map_err(|error| error.to_string())?;
    canvas.restore_to_count(saved);

    let info = ImageInfo::new(preview_size, ColorType::RGBA8888, AlphaType::Premul, None);
    let row_bytes = preview_dimensions.0 as usize * 4;
    let mut pixels = vec![0; info.compute_byte_size(row_bytes)];
    if !surface.read_pixels(&info, &mut pixels, row_bytes, IPoint::new(0, 0)) {
        return Err("读取样式预览像素失败".to_owned());
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [preview_dimensions.0 as usize, preview_dimensions.1 as usize],
        &pixels,
    ))
}

struct StylePreviewRequest {
    key: StylePreviewKey,
    style: LyricsStyle,
    context: egui::Context,
}

struct StylePreviewOutput {
    key: StylePreviewKey,
    image: Result<egui::ColorImage, String>,
}

fn spawn_style_preview_worker() -> (
    mpsc::Sender<StylePreviewRequest>,
    mpsc::Receiver<StylePreviewOutput>,
) {
    let (request_sender, request_receiver) = mpsc::channel::<StylePreviewRequest>();
    let (output_sender, output_receiver) = mpsc::channel::<StylePreviewOutput>();
    thread::spawn(move || {
        let mut cached_cover: Option<(
            (Option<PathBuf>, Option<PathBuf>),
            Result<Option<Vec<u8>>, String>,
        )> = None;
        while let Ok(mut request) = request_receiver.recv() {
            while let Ok(newer) = request_receiver.try_recv() {
                request = newer;
            }
            let source = (
                request.key.scene.cover_path.clone(),
                request.key.scene.embedded_cover_audio.clone(),
            );
            if cached_cover.as_ref().map(|cached| &cached.0) != Some(&source) {
                cached_cover = Some((source, load_style_preview_cover(&request.key.scene)));
            }
            let image = match cached_cover
                .as_ref()
                .expect("preview cover cache")
                .1
                .as_ref()
            {
                Ok(cover) => {
                    render_style_preview(request.style, &request.key.scene, cover.as_deref())
                }
                Err(error) => Err(error.clone()),
            };
            let key = request.key;
            if output_sender
                .send(StylePreviewOutput { key, image })
                .is_err()
            {
                return;
            }
            request.context.request_repaint();
        }
    });
    (request_sender, output_receiver)
}

const BUNDLED_CJK_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.otf");

fn configure_chinese_font(context: &egui::Context) {
    let mut definitions = egui::FontDefinitions::default();
    definitions.font_data.insert(
        "goosy_cjk".to_owned(),
        Arc::new(egui::FontData::from_owned(BUNDLED_CJK_FONT.to_vec())),
    );
    definitions
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("egui proportional family")
        .insert(0, "goosy_cjk".to_owned());
    definitions
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .expect("egui monospace family")
        .insert(0, "goosy_cjk".to_owned());
    context.set_fonts(definitions);
}

const LYRIC_EXTENSIONS: &[&str] = &["lrc", "ttml", "xml", "yrc"];
const COVER_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff", "avif", "heic", "heif",
];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppPage {
    Render,
    Lyrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreviewEvent {
    frame: u64,
    width: usize,
    height: usize,
}

fn parse_preview_event(line: &str) -> Option<PreviewEvent> {
    let mut fields = line.split_whitespace();
    (fields.next()? == "GOOSY_PREVIEW").then_some(())?;
    let event = PreviewEvent {
        frame: fields.next()?.parse().ok()?,
        width: fields.next()?.parse().ok()?,
        height: fields.next()?.parse().ok()?,
    };
    (event.width > 0 && event.height > 0 && fields.next().is_none()).then_some(event)
}

fn new_preview_directory() -> PathBuf {
    let sequence = PREVIEW_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "goosy-render-preview-{}-{sequence}",
        std::process::id()
    ))
}
fn write_internal_preview_frame(
    directory: &Path,
    frame: u64,
    source: &[u8],
    width: u32,
    height: u32,
    preview: &mut Vec<u8>,
) -> std::io::Result<(u32, u32)> {
    let scale = (STYLE_PREVIEW_MAX_WIDTH as f32 / width.max(1) as f32)
        .min(STYLE_PREVIEW_MAX_HEIGHT as f32 / height.max(1) as f32)
        .min(1.0);
    let preview_width = (width as f32 * scale).round().max(1.0) as u32;
    let preview_height = (height as f32 * scale).round().max(1.0) as u32;
    preview.resize(preview_width as usize * preview_height as usize * 4, 0);
    for y in 0..preview_height as usize {
        let source_y = y * height as usize / preview_height as usize;
        for x in 0..preview_width as usize {
            let source_x = x * width as usize / preview_width as usize;
            let source_offset = (source_y * width as usize + source_x) * 4;
            let preview_offset = (y * preview_width as usize + x) * 4;
            preview[preview_offset..preview_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    std::fs::create_dir_all(directory)?;
    std::fs::write(directory.join(format!("{frame}.rgba")), preview)?;
    Ok((preview_width, preview_height))
}

fn terminate_render_child(child: &mut Option<Child>) -> std::io::Result<bool> {
    let Some(mut child) = child.take() else {
        return Ok(false);
    };
    if child.try_wait()?.is_some() {
        return Ok(false);
    }
    child.kill()?;
    child.wait()?;
    Ok(true)
}

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([700.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "GoosyRenderer",
        options,
        Box::new(|creation_context| {
            configure_chinese_font(&creation_context.egui_ctx);
            Ok(Box::new(GoosyApp::default()))
        }),
    )
}

struct GoosyApp {
    audio: Option<PathBuf>,
    lyrics_candidates: Vec<PathBuf>,
    cover_candidates: Vec<PathBuf>,
    selected_lyrics: Option<usize>,
    selected_cover: Option<usize>,
    title: String,
    output: Option<PathBuf>,
    width: u32,
    height: u32,
    fps: u32,
    font_scale_percent: u32,
    line_height_percent: u32,
    line_spacing_percent: u32,
    translation_gap_percent: u32,
    background_gap_percent: u32,
    horizontal_padding_percent: u32,
    debug_overlays: bool,
    use_embedded_cover: bool,
    no_audio: bool,
    render_child: Option<Child>,
    internal_render: Option<InternalRenderTask>,
    render_lifecycle: RenderLifecycle,
    render_sample: bool,
    progress_receiver: Option<mpsc::Receiver<String>>,
    log_receiver: Option<mpsc::Receiver<String>>,
    preview_interval: u64,
    preview_directory: Option<PathBuf>,
    preview_texture: Option<egui::TextureHandle>,
    preview_frame: u64,
    progress: f32,
    speed_history: VecDeque<(f32, f32)>,
    current_speed: f32,
    last_progress_sample: Option<(f32, f32)>,
    style_preview_key: Option<StylePreviewKey>,
    style_preview_texture: Option<egui::TextureHandle>,
    style_preview_error: Option<String>,
    style_preview_sender: mpsc::Sender<StylePreviewRequest>,
    style_preview_receiver: mpsc::Receiver<StylePreviewOutput>,
    render_stage: String,
    render_log: String,
    status: String,
    page: AppPage,
    render_translation: bool,
    render_background_vocal: bool,
    speed_printer: bool,
    auto_exclude_credits: bool,
    lyric_lines: Vec<lrc::LyricLine>,
    manual_excluded_lines: Vec<bool>,
    auto_excluded_lines: Vec<bool>,
}

impl Default for GoosyApp {
    fn default() -> Self {
        let (style_preview_sender, style_preview_receiver) = spawn_style_preview_worker();
        Self {
            audio: None,
            lyrics_candidates: Vec::new(),
            cover_candidates: Vec::new(),
            selected_lyrics: None,
            selected_cover: None,
            title: String::new(),
            output: None,
            width: 1920,
            height: 1080,
            fps: 30,
            font_scale_percent: 100,
            line_height_percent: 100,
            line_spacing_percent: 100,
            translation_gap_percent: 100,
            background_gap_percent: 100,
            horizontal_padding_percent: 100,
            debug_overlays: false,
            use_embedded_cover: true,
            no_audio: false,
            render_child: None,
            internal_render: None,
            render_lifecycle: RenderLifecycle::Idle,
            render_sample: false,
            progress_receiver: None,
            log_receiver: None,
            preview_interval: DEFAULT_PREVIEW_INTERVAL,
            preview_directory: None,
            preview_texture: None,
            preview_frame: 0,
            progress: 0.0,
            speed_history: VecDeque::new(),
            current_speed: 0.0,
            last_progress_sample: None,
            style_preview_key: None,
            style_preview_texture: None,
            style_preview_error: None,
            style_preview_sender,
            style_preview_receiver,
            render_stage: String::new(),
            render_log: String::new(),
            status: String::new(),
            page: AppPage::Render,
            render_translation: true,
            render_background_vocal: true,
            speed_printer: false,
            auto_exclude_credits: false,
            lyric_lines: Vec::new(),
            manual_excluded_lines: Vec::new(),
            auto_excluded_lines: Vec::new(),
        }
    }
}

impl GoosyApp {
    fn select_audio(&mut self, path: PathBuf) {
        let (lyrics, covers) = scan_sibling_assets(&path);
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("lyrics")
            .to_owned();
        let output = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}.mp4"));
        let metadata_title = video::audio_metadata(&path)
            .ok()
            .and_then(|metadata| metadata.title);
        self.audio = Some(path);
        self.lyrics_candidates = lyrics;
        self.cover_candidates = covers;
        self.selected_lyrics = (!self.lyrics_candidates.is_empty()).then_some(0);
        self.selected_cover = (!self.cover_candidates.is_empty()).then_some(0);
        self.title = metadata_title.unwrap_or(stem);
        self.output = Some(output);
        self.status = if self.selected_lyrics.is_some() {
            "已根据音频名称找到歌词和封面候选".to_owned()
        } else {
            "未找到同名歌词，将尝试读取音频内嵌歌词".to_owned()
        };
        self.reload_lyrics_preview();
    }

    fn reload_lyrics_preview(&mut self) {
        self.lyric_lines.clear();
        self.manual_excluded_lines.clear();
        self.auto_excluded_lines.clear();
        let Some(audio) = self.audio.as_deref() else {
            return;
        };
        let lyrics_path = self
            .selected_lyrics
            .and_then(|index| self.lyrics_candidates.get(index))
            .cloned();
        let text = match lyrics_path.as_deref() {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    self.status = format!("读取歌词失败：{error}");
                    return;
                }
            },
            None => match video::audio_metadata(audio)
                .ok()
                .and_then(|metadata| metadata.lyrics)
            {
                Some(text) => text,
                None => {
                    self.status = "当前音频没有可预览的内嵌歌词".to_owned();
                    return;
                }
            },
        };
        match libgoosy::parse_lyrics(&text, LyricFormat::Auto) {
            Ok(lines) => {
                self.lyric_lines = lines;
                self.manual_excluded_lines = vec![false; self.lyric_lines.len()];
                self.recompute_auto_exclusions();
            }
            Err(error) => {
                self.status = format!("解析歌词失败：{error}");
            }
        }
    }

    fn recompute_auto_exclusions(&mut self) {
        self.auto_excluded_lines = if self.auto_exclude_credits {
            detect_credit_lines(&self.lyric_lines)
        } else {
            vec![false; self.lyric_lines.len()]
        };
    }

    fn excluded_line_indices(&self) -> Vec<usize> {
        self.lyric_lines
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                (self
                    .manual_excluded_lines
                    .get(index)
                    .copied()
                    .unwrap_or(false)
                    || self
                        .auto_excluded_lines
                        .get(index)
                        .copied()
                        .unwrap_or(false))
                .then_some(index)
            })
            .collect()
    }

    fn format_line_time(time_ms: u64) -> String {
        format!(
            "{:02}:{:02}.{:03}",
            time_ms / 60_000,
            (time_ms / 1_000) % 60,
            time_ms % 1_000
        )
    }

    fn draw_lyrics_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("歌词行控制");
        ui.label("取消勾选的行不会进入渲染布局，后续歌词的时间轴保持不变。");
        ui.add_space(8.0);
        let auto_changed = ui
            .checkbox(
                &mut self.auto_exclude_credits,
                "自动排除头部/尾部的歌手、作曲、作词等署名信息",
            )
            .changed();
        if auto_changed {
            self.recompute_auto_exclusions();
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.render_translation, "渲染翻译");
            ui.checkbox(&mut self.render_background_vocal, "渲染伴唱");
        });
        ui.checkbox(&mut self.speed_printer, "速印机优化（黑白双色）");
        ui.add_space(6.0);
        if ui
            .add_enabled(
                !self.lyric_lines.is_empty() && self.render_child.is_none(),
                egui::Button::new("导出可打印 PDF…"),
            )
            .clicked()
        {
            self.export_pdf();
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        ui.separator();
        if self.lyric_lines.is_empty() {
            ui.label("选择音频或歌词文件后，这里会显示可单独排除的歌词行。");
            return;
        }
        let excluded_count = self.excluded_line_indices().len();
        ui.label(format!(
            "共 {} 行，当前排除 {} 行",
            self.lyric_lines.len(),
            excluded_count
        ));
        egui::ScrollArea::vertical()
            .id_salt("lyric_line_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in 0..self.lyric_lines.len() {
                    let line = &self.lyric_lines[index];
                    let auto_excluded = self.auto_excluded_lines[index];
                    let mut excluded = self.manual_excluded_lines[index] || auto_excluded;
                    let label = format!(
                        "{}  {}",
                        Self::format_line_time(line.start_ms),
                        if line.text.trim().is_empty() {
                            "（空行）"
                        } else {
                            line.text.trim()
                        }
                    );
                    let changed = ui
                        .add_enabled(!auto_excluded, egui::Checkbox::new(&mut excluded, label))
                        .changed();
                    if changed {
                        self.manual_excluded_lines[index] = excluded;
                    }
                    if auto_excluded {
                        ui.label("自动识别的署名行");
                    }
                }
            });
    }

    fn export_pdf(&mut self) {
        if self.lyric_lines.is_empty() {
            self.status = "请先选择有效的歌词文件".to_owned();
            return;
        }
        let default_name = self
            .audio
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .unwrap_or("lyrics");
        let Some(mut output) = FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(format!("{default_name}.pdf"))
            .save_file()
        else {
            return;
        };
        if output.extension().is_none() {
            output.set_extension("pdf");
        }
        let Some(audio) = self.audio.as_ref() else {
            self.status = "请先选择音频".to_owned();
            return;
        };
        let Ok(mut executable) = std::env::current_exe() else {
            self.status = "无法定位 goosy 可执行文件".to_owned();
            return;
        };
        executable.set_file_name(if cfg!(windows) {
            "goosy-render-worker.exe"
        } else {
            "goosy-render-worker"
        });
        if !executable.is_file() {
            self.status = format!("找不到独立渲染进程：{}", executable.display());
            return;
        }
        let lyrics = self
            .selected_lyrics
            .and_then(|index| self.lyrics_candidates.get(index));
        let mut command = Command::new(executable);
        command.arg("pdf").arg(audio);
        if let Some(lyrics) = lyrics {
            command.arg(lyrics);
        }
        command.arg("--output").arg(&output);
        if !self.title.trim().is_empty() {
            command.arg("--title").arg(self.title.trim());
        }
        if !self.render_translation {
            command.arg("--no-translation");
        }
        if !self.render_background_vocal {
            command.arg("--no-background-vocal");
        }
        if self.speed_printer {
            command.arg("--speed-printer");
        }
        for index in self.excluded_line_indices() {
            command.arg("--exclude-line").arg(index.to_string());
        }
        self.status = match command.output() {
            Ok(result) if result.status.success() => {
                format!("PDF 导出完成：{}", output.display())
            }
            Ok(result) => format!(
                "PDF 导出失败：{}",
                String::from_utf8_lossy(&result.stderr).trim()
            ),
            Err(error) => format!("PDF 导出失败：{error}"),
        };
    }

    fn choose_audio(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter(
                "Audio",
                &["mp3", "flac", "m4a", "wav", "aac", "ogg", "opus"],
            )
            .pick_file()
        {
            self.select_audio(path);
        }
    }

    fn choose_lyrics(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Lyrics", LYRIC_EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        if !self.lyrics_candidates.contains(&path) {
            self.lyrics_candidates.push(path.clone());
        }
        self.selected_lyrics = self
            .lyrics_candidates
            .iter()
            .position(|candidate| candidate == &path);
        self.reload_lyrics_preview();
    }

    fn choose_cover(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter(
                "Images",
                &[
                    "jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff", "avif", "heic",
                    "heif",
                ],
            )
            .pick_file()
        else {
            return;
        };
        if !self.cover_candidates.contains(&path) {
            self.cover_candidates.push(path.clone());
        }
        self.selected_cover = self
            .cover_candidates
            .iter()
            .position(|candidate| candidate == &path);
    }

    fn current_lyrics_style(&self) -> LyricsStyle {
        LyricsStyle {
            font_scale: self.font_scale_percent as f32 / 100.0,
            line_height_scale: self.line_height_percent as f32 / 100.0,
            group_gap_scale: self.line_spacing_percent as f32 / 100.0,
            translation_gap_scale: self.translation_gap_percent as f32 / 100.0,
            background_gap_scale: self.background_gap_percent as f32 / 100.0,
            horizontal_padding_scale: self.horizontal_padding_percent as f32 / 100.0,
            debug_overlays: self.debug_overlays,
        }
    }

    fn refresh_style_preview(&mut self, context: &egui::Context) {
        let style_key = [
            self.font_scale_percent,
            self.line_height_percent,
            self.line_spacing_percent,
            self.translation_gap_percent,
            self.background_gap_percent,
            self.horizontal_padding_percent,
            self.debug_overlays as u32,
        ];
        let cover_path = self
            .selected_cover
            .and_then(|index| self.cover_candidates.get(index).cloned());
        let embedded_cover_audio = if cover_path.is_none() && self.use_embedded_cover {
            self.audio.clone()
        } else {
            None
        };
        let key = StylePreviewKey {
            style: style_key,
            scene: StylePreviewScene {
                width: self.width,
                height: self.height,
                title: self.title.clone(),
                cover_path,
                embedded_cover_audio,
                render_translation: self.render_translation,
                render_background_vocal: self.render_background_vocal,
            },
        };
        if self.style_preview_key.as_ref() != Some(&key) {
            self.style_preview_key = Some(key.clone());
            self.style_preview_error = None;
            if self
                .style_preview_sender
                .send(StylePreviewRequest {
                    key,
                    style: self.current_lyrics_style(),
                    context: context.clone(),
                })
                .is_err()
            {
                self.style_preview_error = Some("样式预览工作线程已经停止".to_owned());
            }
        }

        let mut latest = None;
        for output in self.style_preview_receiver.try_iter() {
            if self.style_preview_key.as_ref() == Some(&output.key) {
                latest = Some(output.image);
            }
        }
        match latest {
            Some(Ok(image)) => {
                if let Some(texture) = &mut self.style_preview_texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.style_preview_texture = Some(context.load_texture(
                        "goosy-style-preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.style_preview_error = None;
            }
            Some(Err(error)) => {
                self.style_preview_error = Some(error);
            }
            None => {}
        }
    }

    fn cleanup_preview_directory(&mut self) {
        if let Some(directory) = self.preview_directory.take() {
            let _ = std::fs::remove_dir_all(directory);
        }
    }

    fn preview_path(&self, frame: u64) -> Option<PathBuf> {
        self.preview_directory
            .as_ref()
            .map(|directory| directory.join(format!("{frame}.rgba")))
    }

    fn discard_preview(&self, event: PreviewEvent) {
        if let Some(path) = self.preview_path(event.frame) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn load_preview(&mut self, context: &egui::Context, event: PreviewEvent) {
        let Some(path) = self.preview_path(event.frame) else {
            return;
        };
        let expected_size = event.width.saturating_mul(event.height).saturating_mul(4);
        let pixels = match std::fs::read(&path) {
            Ok(pixels) if pixels.len() == expected_size => pixels,
            Ok(pixels) => {
                self.status = format!(
                    "实时预览帧大小无效：应为 {expected_size} 字节，实际为 {} 字节",
                    pixels.len()
                );
                let _ = std::fs::remove_file(path);
                return;
            }
            Err(error) => {
                self.status = format!("读取实时预览失败：{error}");
                return;
            }
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([event.width, event.height], &pixels);
        if let Some(texture) = &mut self.preview_texture {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.preview_texture = Some(context.load_texture(
                "goosy-live-preview",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.preview_frame = event.frame;
        let _ = std::fs::remove_file(path);
        context.request_repaint();
    }

    fn send_render_command(&mut self, command: &str) {
        if let Some(task) = &self.internal_render {
            let _ = task.control.send(command.to_owned());
            return;
        }
        if let Some(child) = &mut self.render_child {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = writeln!(stdin, "{command}");
                let _ = stdin.flush();
            }
        }
    }

    fn pause_render(&mut self) {
        if self.render_lifecycle != RenderLifecycle::Running {
            return;
        }
        self.send_render_command("pause");
        self.render_lifecycle = RenderLifecycle::Paused;
        self.status = "渲染已暂停，可继续或重启".to_owned();
    }

    fn resume_render(&mut self) {
        if self.render_lifecycle != RenderLifecycle::Paused {
            return;
        }
        self.send_render_command("resume");
        self.render_lifecycle = RenderLifecycle::Running;
        self.status = "渲染继续进行".to_owned();
    }

    fn stop_render(&mut self) {
        if self.render_child.is_none() && self.internal_render.is_none() {
            return;
        }
        self.render_lifecycle = RenderLifecycle::EmergencyStopped;
        self.send_render_command("stop");
        if self.render_child.is_some() {
            let result = terminate_render_child(&mut self.render_child);
            self.progress_receiver = None;
            self.log_receiver = None;
            self.cleanup_preview_directory();
            self.render_stage = "渲染已急停".to_owned();
            self.status = match result {
                Ok(true) => "渲染已急停；本次任务不可恢复".to_owned(),
                Ok(false) => "渲染进程已经结束；本次任务不可恢复".to_owned(),
                Err(error) => format!("急停渲染失败：{error}"),
            };
        } else {
            self.status = "渲染急停请求已发送；本次任务不可恢复".to_owned();
        }
    }
    fn render_output_path(&self, sample: Option<(u64, u64)>) -> Option<PathBuf> {
        let output = self.output.clone()?;
        let Some((start_ms, _)) = sample else {
            return Some(output);
        };
        let stem = output.file_stem()?.to_str()?;
        let extension = output
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("mp4");
        Some(output.with_file_name(format!("{stem}.sample-{start_ms}ms.{extension}")))
    }

    fn random_sample_range(&self) -> Option<(u64, u64)> {
        let end_ms = self
            .lyric_lines
            .iter()
            .map(|line| line.end_ms)
            .max()?
            .saturating_add(1_000);
        let duration_ms = 15_000.min(end_ms);
        if duration_ms == 0 {
            return None;
        }
        let max_start = end_ms.saturating_sub(duration_ms);
        let eligible = self
            .lyric_lines
            .iter()
            .enumerate()
            .filter(|(index, line)| {
                !line.text.trim().is_empty() && !self.excluded_line_indices().contains(index)
            })
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            return None;
        }
        let (_, line) = eligible.get(
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos() as usize)
                % eligible.len(),
        )?;
        let start = line.start_ms.saturating_sub(3_000).min(max_start);
        Some((start, start.saturating_add(duration_ms)))
    }

    fn start_sample_render(&mut self) {
        let Some(sample) = self.random_sample_range() else {
            self.status = "没有可用于采样的歌词".to_owned();
            return;
        };
        self.start_render_with_range(Some(sample));
    }

    fn start_render(&mut self) {
        self.start_render_with_range(None);
    }

    fn start_render_with_range(&mut self, sample: Option<(u64, u64)>) {
        if self.render_child.is_some() || self.internal_render.is_some() {
            return;
        }
        self.render_sample = sample.is_some();
        #[cfg(target_os = "windows")]
        self.start_external_render(sample);
        #[cfg(not(target_os = "windows"))]
        self.start_internal_render(sample);
    }

    #[cfg(target_os = "windows")]
    fn start_external_render(&mut self, sample: Option<(u64, u64)>) {
        let Some(audio) = self.audio.clone() else {
            self.status = "请先选择音频和输出".to_owned();
            return;
        };
        let Some(output) = self.render_output_path(sample) else {
            self.status = "无法生成渲染输出路径".to_owned();
            return;
        };
        let lyrics = self
            .selected_lyrics
            .and_then(|index| self.lyrics_candidates.get(index).cloned());
        let Ok(mut executable) = std::env::current_exe() else {
            let message = "无法定位 goosy 可执行文件".to_owned();
            self.status = message;
            return;
        };
        executable.set_file_name(if cfg!(windows) {
            "goosy-render-worker.exe"
        } else {
            "goosy-render-worker"
        });
        if !executable.is_file() {
            let message = format!("找不到独立渲染进程：{}", executable.display());
            self.status = message;
            return;
        }
        self.cleanup_preview_directory();
        let preview_directory = new_preview_directory();
        if let Err(error) = std::fs::create_dir_all(&preview_directory) {
            self.status = format!("创建实时预览目录失败：{error}");
            return;
        }
        let mut command = Command::new(executable);
        command.arg("render").arg(audio);
        if let Some(lyrics) = lyrics {
            command.arg(lyrics);
        }
        command
            .arg("--output")
            .arg(output)
            .arg("--width")
            .arg(self.width.to_string())
            .arg("--height")
            .arg(self.height.to_string())
            .arg("--fps")
            .arg(self.fps.to_string())
            .arg("--font-scale")
            .arg(format!("{:.2}", self.font_scale_percent as f32 / 100.0))
            .arg("--line-height-scale")
            .arg(format!("{:.2}", self.line_height_percent as f32 / 100.0))
            .arg("--line-spacing-scale")
            .arg(format!("{:.2}", self.line_spacing_percent as f32 / 100.0))
            .arg("--translation-gap-scale")
            .arg(format!(
                "{:.2}",
                self.translation_gap_percent as f32 / 100.0
            ))
            .arg("--background-gap-scale")
            .arg(format!("{:.2}", self.background_gap_percent as f32 / 100.0))
            .arg("--horizontal-padding-scale")
            .arg(format!(
                "{:.2}",
                self.horizontal_padding_percent as f32 / 100.0
            ))
            .arg("--format")
            .arg("auto")
            .arg("--progress-events")
            .arg("--preview-dir")
            .arg(&preview_directory)
            .arg("--preview-interval")
            .arg(self.preview_interval.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((start_ms, end_ms)) = sample {
            command
                .arg("--sample-start-ms")
                .arg(start_ms.to_string())
                .arg("--sample-duration-ms")
                .arg(end_ms.saturating_sub(start_ms).to_string())
                .arg("--no-audio");
        }
        if self.debug_overlays {
            command.arg("--debug-overlays");
        }
        if self.no_audio {
            command.arg("--no-audio");
        }
        if !self.render_translation {
            command.arg("--no-translation");
        }
        if !self.render_background_vocal {
            command.arg("--no-background-vocal");
        }
        for index in self.excluded_line_indices() {
            command.arg("--exclude-line").arg(index.to_string());
        }
        if !self.use_embedded_cover {
            command.arg("--no-embedded-cover");
        }
        if let Some(index) = self.selected_cover {
            if let Some(cover) = self.cover_candidates.get(index) {
                command.arg("--cover").arg(cover);
            }
        }
        if !self.title.trim().is_empty() {
            command.arg("--title").arg(self.title.trim());
        }
        match command.spawn() {
            Ok(mut child) => {
                let (progress_sender, progress_receiver) = mpsc::channel();
                let (log_sender, log_receiver) = mpsc::channel();
                if let Some(stdout) = child.stdout.take() {
                    thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            let _ = progress_sender.send(line);
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    thread::spawn(move || {
                        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                            let _ = log_sender.send(line);
                        }
                    });
                }
                self.render_child = Some(child);
                self.render_lifecycle = RenderLifecycle::Running;
                self.internal_render = None;
                self.progress_receiver = Some(progress_receiver);
                self.log_receiver = Some(log_receiver);
                self.preview_directory = Some(preview_directory);
                self.preview_texture = None;
                self.preview_frame = 0;
                self.progress = 0.0;
                self.speed_history.clear();
                self.current_speed = 0.0;
                self.last_progress_sample = None;
                self.render_stage = "启动渲染进程".to_owned();
                self.render_log.clear();
                self.status = "渲染进程已启动，等待首帧…".to_owned();
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(preview_directory);
                let message = format!("启动渲染失败：{error}");
                self.status = message;
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    fn start_internal_render(&mut self, sample: Option<(u64, u64)>) {
        let (Some(audio), Some(output)) = (self.audio.clone(), self.render_output_path(sample))
        else {
            self.status = "请先选择音频和输出".to_owned();
            return;
        };
        let lyrics = self
            .selected_lyrics
            .and_then(|index| self.lyrics_candidates.get(index).cloned());
        let cover = self
            .selected_cover
            .and_then(|index| self.cover_candidates.get(index).cloned());
        self.cleanup_preview_directory();
        let preview_directory = new_preview_directory();
        if let Err(error) = std::fs::create_dir_all(&preview_directory) {
            self.status = format!("创建实时预览目录失败：{error}");
            return;
        }
        let thread_preview_directory = preview_directory.clone();
        let options = RenderOptions {
            song: audio,
            lyrics,
            output,
            width: self.width,
            height: self.height,
            fps: self.fps,
            background: None,
            cover,
            title: (!self.title.trim().is_empty()).then(|| self.title.trim().to_owned()),
            no_embedded_cover: !self.use_embedded_cover,
            no_audio: self.no_audio || sample.is_some(),
            render_translation: self.render_translation,
            lyrics_style: self.current_lyrics_style(),
            render_background_vocal: self.render_background_vocal,
            excluded_lines: self.excluded_line_indices(),
            format: LyricFormat::Auto,
            sample_start_ms: sample.map(|(start, _)| start),
            sample_duration_ms: sample.map(|(start, end)| end.saturating_sub(start)),
        };
        let preview_interval = self.preview_interval;
        let width = self.width;
        let height = self.height;
        let (progress_sender, progress_receiver) = mpsc::channel();
        let (log_sender, log_receiver) = mpsc::channel();
        let (control_sender, control_receiver) = mpsc::channel::<String>();
        let (done_sender, done_receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut preview_dir = Some(thread_preview_directory);
            let mut preview_pixels = Vec::new();
            let mut paused = false;
            let result = libgoosy::render_with_frame_progress_control(
                &options,
                |done, total, elapsed, pixels| {
                    let _ =
                        progress_sender.send(format!("GOOSY_PROGRESS {done} {total} {elapsed:.3}"));
                    let preview_due = done == 1 || done == total || done % preview_interval == 0;
                    if preview_due {
                        if let Some(directory) = preview_dir.as_deref() {
                            match write_internal_preview_frame(
                                directory,
                                done,
                                pixels,
                                width,
                                height,
                                &mut preview_pixels,
                            ) {
                                Ok((preview_width, preview_height)) => {
                                    let _ = progress_sender.send(format!(
                                        "GOOSY_PREVIEW {done} {preview_width} {preview_height}"
                                    ));
                                }
                                Err(error) => {
                                    let _ = log_sender
                                        .send(format!("写入实时预览失败，已停用预览：{error}"));
                                    preview_dir = None;
                                }
                            }
                        }
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
                            Err(mpsc::TryRecvError::Disconnected) => return RenderControl::Stop,
                        }
                    }
                },
            );
            let _ = done_sender.send(result);
        });
        self.internal_render = Some(InternalRenderTask {
            control: control_sender,
            done: done_receiver,
            thread: Some(thread),
        });
        self.render_child = None;
        self.progress_receiver = Some(progress_receiver);
        self.log_receiver = Some(log_receiver);
        self.preview_directory = Some(preview_directory);
        self.preview_texture = None;
        self.preview_frame = 0;
        self.progress = 0.0;
        self.speed_history.clear();
        self.current_speed = 0.0;
        self.last_progress_sample = None;
        self.render_lifecycle = RenderLifecycle::Running;
        self.render_stage = "单进程渲染中".to_owned();
        self.render_log.clear();
        self.status = if sample.is_some() {
            "随机 15 秒采样渲染已启动".to_owned()
        } else {
            "单进程渲染已启动，等待首帧…".to_owned()
        };
    }

    fn poll_render(&mut self, context: &egui::Context) {
        let mut latest_preview = None;
        if let Some(receiver) = &self.progress_receiver {
            let lines: Vec<_> = receiver.try_iter().collect();
            for line in lines {
                if let Some(event) = parse_preview_event(&line) {
                    if let Some(previous) = latest_preview.replace(event) {
                        self.discard_preview(previous);
                    }
                    continue;
                }
                let mut fields = line.split_whitespace();
                match fields.next() {
                    Some("GOOSY_STAGE") => {
                        let _key = fields.next();
                        let message = fields.collect::<Vec<_>>().join(" ");
                        if !message.is_empty() {
                            self.render_stage = message.clone();
                            self.status = message;
                        }
                    }
                    Some("GOOSY_PROGRESS") => {
                        let done = fields.next().and_then(|value| value.parse::<f32>().ok());
                        let total = fields.next().and_then(|value| value.parse::<f32>().ok());
                        let elapsed = fields.next().and_then(|value| value.parse::<f32>().ok());
                        if let (Some(done), Some(total), Some(elapsed)) = (done, total, elapsed) {
                            self.progress = if total > 0.0 {
                                (done / total).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            if elapsed > 0.0 {
                                self.current_speed =
                                    if let Some((previous_done, previous_elapsed)) =
                                        self.last_progress_sample
                                    {
                                        (done - previous_done)
                                            / (elapsed - previous_elapsed).max(0.001)
                                    } else {
                                        done / elapsed
                                    };
                                self.last_progress_sample = Some((done, elapsed));
                                self.speed_history.push_back((elapsed, self.current_speed));
                                while self.speed_history.len() > 120 {
                                    self.speed_history.pop_front();
                                }
                            }
                            self.status = format!(
                                "正在渲染：{done:.0}/{total:.0} · {:.1} 帧/秒",
                                self.current_speed
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(event) = latest_preview {
            self.load_preview(context, event);
        }
        if let Some(receiver) = &self.log_receiver {
            let lines: Vec<_> = receiver.try_iter().collect();
            for line in lines {
                if !line.trim().is_empty() {
                    // The stderr reader thread already persisted this line synchronously.
                    if !self.render_log.is_empty() {
                        self.render_log.push('\n');
                    }
                    self.render_log.push_str(&line);
                    if self.render_log.len() > 8_192 {
                        let keep_from = self.render_log.len() - 8_192;
                        let boundary = self.render_log.ceil_char_boundary(keep_from);
                        self.render_log.drain(..boundary);
                    }
                }
            }
        }
        if !self.render_log.is_empty() {
            self.status = format!("{}：\n{}", self.render_stage, self.render_log);
        }
        let result = self.render_child.as_mut().map(|child| child.try_wait());
        match result {
            Some(Ok(Some(status))) => {
                self.render_child = None;
                self.progress_receiver = None;
                self.log_receiver = None;
                self.cleanup_preview_directory();
                self.progress = if status.success() { 1.0 } else { self.progress };
                if status.success() {
                    self.render_lifecycle = RenderLifecycle::Idle;
                    self.status = "渲染完成".to_owned();
                } else if self.render_lifecycle != RenderLifecycle::EmergencyStopped {
                    self.render_lifecycle = RenderLifecycle::Idle;
                    if self.render_log.is_empty() {
                        self.status = format!("渲染失败：{status}");
                    }
                }
            }
            Some(Ok(None)) => context.request_repaint_after(Duration::from_millis(100)),
            Some(Err(error)) => {
                self.render_child = None;
                self.progress_receiver = None;
                self.log_receiver = None;
                self.cleanup_preview_directory();
                let message = format!("读取渲染状态失败：{error}");
                self.status = message;
            }
            None => {}
        }
        let internal_result = self
            .internal_render
            .as_ref()
            .and_then(|task| task.done.try_recv().ok());
        if let Some(result) = internal_result {
            if let Some(mut task) = self.internal_render.take() {
                if let Some(thread) = task.thread.take() {
                    let _ = thread.join();
                }
            }
            self.progress_receiver = None;
            self.log_receiver = None;
            self.cleanup_preview_directory();
            match result {
                Ok(()) if self.render_lifecycle == RenderLifecycle::EmergencyStopped => {}
                Ok(()) => {
                    self.progress = 1.0;
                    self.render_lifecycle = RenderLifecycle::Idle;
                    self.status = if self.render_sample {
                        "随机 15 秒采样渲染完成".to_owned()
                    } else {
                        "渲染完成".to_owned()
                    };
                }
                Err(error) if self.render_lifecycle == RenderLifecycle::EmergencyStopped => {}
                Err(error) => {
                    self.render_lifecycle = RenderLifecycle::Idle;
                    self.status = format!("渲染失败：{error}");
                }
            }
        }
        if self.internal_render.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
    fn draw_speed_graph(&self, ui: &mut egui::Ui) {
        if self.speed_history.is_empty() {
            return;
        }
        ui.label(format!("实时渲染速度：{:.1} 帧/秒", self.current_speed));
        let width = ui.available_width().max(240.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 132.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, egui::Color32::from_rgb(21, 27, 38));
        let max_speed = self
            .speed_history
            .iter()
            .map(|(_, speed)| *speed)
            .fold(1.0_f32, f32::max)
            * 1.15;
        for fraction in [0.25_f32, 0.5, 0.75] {
            let y = rect.bottom() - rect.height() * fraction;
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, egui::Color32::from_gray(48)),
            );
        }
        let count = self.speed_history.len().max(2) as f32 - 1.0;
        let points: Vec<_> = self
            .speed_history
            .iter()
            .enumerate()
            .map(|(index, (_, speed))| {
                let x = rect.left() + rect.width() * index as f32 / count;
                let y = rect.bottom() - rect.height() * (*speed / max_speed).clamp(0.0, 1.0);
                egui::pos2(x, y)
            })
            .collect();
        for pair in points.windows(2) {
            painter.line_segment(
                [pair[0], pair[1]],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(73, 214, 157)),
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(8.0, 7.0),
            egui::Align2::LEFT_TOP,
            format!("0–{max_speed:.0} fps"),
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(180),
        );
    }

    fn draw_render_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("素材与输出");
        ui.label("选择音频后自动匹配同名歌词与封面。");
        ui.add_space(4.0);
        egui::Grid::new("source_grid")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label("音频");
                ui.horizontal(|ui| {
                    let name = self
                        .audio
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .and_then(|name| name.to_str())
                        .unwrap_or("尚未选择");
                    ui.label(name);
                    if ui.button("选择…").clicked() {
                        self.choose_audio();
                    }
                });
                ui.end_row();

                ui.label("歌词");
                let previous_lyrics = self.selected_lyrics;
                ui.horizontal(|ui| {
                    if self.lyrics_candidates.is_empty() {
                        ui.label("自动读取内嵌歌词");
                    } else {
                        let selected = Self::selected_name(
                            &self.lyrics_candidates,
                            self.selected_lyrics,
                            "选择歌词",
                        );
                        egui::ComboBox::from_id_salt("lyrics_combo")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for (index, path) in self.lyrics_candidates.iter().enumerate() {
                                    let name = path
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("lyrics");
                                    ui.selectable_value(
                                        &mut self.selected_lyrics,
                                        Some(index),
                                        name,
                                    );
                                }
                            });
                    }
                    if ui.button("手动…").clicked() {
                        self.choose_lyrics();
                    }
                });
                ui.end_row();
                if previous_lyrics != self.selected_lyrics {
                    self.reload_lyrics_preview();
                }

                ui.label("封面");
                ui.horizontal(|ui| {
                    let selected = Self::selected_name(
                        &self.cover_candidates,
                        self.selected_cover,
                        "不显示封面",
                    );
                    if self.cover_candidates.is_empty() {
                        ui.label(if self.use_embedded_cover {
                            "使用内嵌封面"
                        } else {
                            "未找到封面"
                        });
                    } else {
                        egui::ComboBox::from_id_salt("cover_combo")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.selected_cover, None, "不显示封面");
                                for (index, path) in self.cover_candidates.iter().enumerate() {
                                    let name = path
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("cover");
                                    ui.selectable_value(
                                        &mut self.selected_cover,
                                        Some(index),
                                        name,
                                    );
                                }
                            });
                    }
                    if ui.button("手动…").clicked() {
                        self.choose_cover();
                    }
                });
                ui.end_row();

                ui.label("歌曲名称");
                ui.text_edit_singleline(&mut self.title);
                ui.end_row();

                ui.label("输出文件");
                ui.horizontal(|ui| {
                    let display = self
                        .output
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .and_then(|name| name.to_str())
                        .unwrap_or("选择音频后自动生成");
                    let response = ui.label(display);
                    if let Some(output) = &self.output {
                        response.on_hover_text(output.display().to_string());
                    }
                    if ui.button("选择…").clicked() {
                        if let Some(path) =
                            FileDialog::new().set_file_name("lyrics.mp4").save_file()
                        {
                            self.output = Some(path);
                        }
                    }
                });
                ui.end_row();
            });

        ui.add_space(6.0);
        ui.group(|ui| {
            ui.strong("输出参数");
            ui.horizontal_wrapped(|ui| {
                ui.label("尺寸");
                ui.add(
                    egui::DragValue::new(&mut self.width)
                        .range(320..=7680)
                        .suffix(" px"),
                );
                ui.label("×");
                ui.add(
                    egui::DragValue::new(&mut self.height)
                        .range(180..=4320)
                        .suffix(" px"),
                );
                ui.label("帧率");
                ui.add(
                    egui::DragValue::new(&mut self.fps)
                        .range(1..=120)
                        .suffix(" fps"),
                );
            });
            egui::CollapsingHeader::new("歌词样式")
                .default_open(true)
                .show(ui, |ui| {
                    style_percent_slider(ui, "字号", &mut self.font_scale_percent, 50..=200);
                    style_percent_slider(ui, "段内行高", &mut self.line_height_percent, 80..=180);
                    style_percent_slider(ui, "歌词行间距", &mut self.line_spacing_percent, 0..=200);
                    style_percent_slider(
                        ui,
                        "翻译间距",
                        &mut self.translation_gap_percent,
                        0..=200,
                    );
                    style_percent_slider(ui, "伴唱间距", &mut self.background_gap_percent, 0..=200);
                    style_percent_slider(
                        ui,
                        "左右留白",
                        &mut self.horizontal_padding_percent,
                        0..=200,
                    );
                    ui.checkbox(&mut self.debug_overlays, "调试框（容器/字形/行距）");
                });
            ui.horizontal(|ui| {
                ui.label("预览间隔");
                ui.add(
                    egui::DragValue::new(&mut self.preview_interval)
                        .range(1..=300)
                        .suffix(" 帧/次"),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.use_embedded_cover, "使用内嵌封面");
                ui.checkbox(&mut self.no_audio, "不输出音频");
            });
        });
    }

    fn draw_render_monitor(&mut self, ui: &mut egui::Ui) {
        ui.heading("渲染状态");
        let active = self.render_child.is_some() || self.internal_render.is_some();
        let paused = self.render_lifecycle == RenderLifecycle::Paused;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!active, egui::Button::new("开始渲染"))
                .clicked()
            {
                self.start_render();
            }
            if ui
                .add_enabled(
                    !active && !self.lyric_lines.is_empty(),
                    egui::Button::new("随机采样 15 秒"),
                )
                .clicked()
            {
                self.start_sample_render();
            }
            if active
                && ui
                    .add_enabled(
                        true,
                        egui::Button::new(if paused {
                            "继续渲染"
                        } else {
                            "暂停渲染"
                        }),
                    )
                    .clicked()
            {
                if paused {
                    self.resume_render();
                } else {
                    self.pause_render();
                }
            }
            if active
                && ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("急停")
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(176, 32, 48)),
                    )
                    .clicked()
            {
                self.stop_render();
            }
            if active {
                ui.spinner();
            }
        });
        if active || self.progress > 0.0 {
            ui.add(egui::ProgressBar::new(self.progress).show_percentage());
        }
        if let Some(texture) = &self.preview_texture {
            ui.add_space(4.0);
            ui.label(format!("实时预览 · 第 {} 帧", self.preview_frame));
            ui.add(
                egui::Image::new(texture)
                    .max_width(ui.available_width())
                    .max_height(260.0)
                    .maintain_aspect_ratio(true),
            );
        } else if let Some(texture) = &self.style_preview_texture {
            ui.add_space(4.0);
            ui.label(format!(
                "参数即时预览 · {}×{} 等比",
                self.width, self.height
            ));
            ui.add(
                egui::Image::new(texture)
                    .max_width(ui.available_width())
                    .max_height(260.0)
                    .maintain_aspect_ratio(true),
            );
            ui.weak("拖动左侧歌词样式参数，预览会立即更新。");
        } else if let Some(error) = &self.style_preview_error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("样式预览失败：{error}"));
        } else if !active {
            ui.weak("正在生成样式预览…");
        }
        if !self.speed_history.is_empty() {
            self.draw_speed_graph(ui);
        }
        if !self.status.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("render_status")
                .max_height(90.0)
                .show(ui, |ui| {
                    ui.label(&self.status);
                });
        }
    }

    fn draw_render_page(&mut self, ui: &mut egui::Ui) {
        self.refresh_style_preview(ui.ctx());
        if render_page_uses_columns(ui.available_width()) {
            ui.columns(2, |columns| {
                let (left, right) = columns.split_at_mut(1);
                egui::ScrollArea::vertical()
                    .id_salt("render_settings_scroll")
                    .show(&mut left[0], |ui| self.draw_render_settings(ui));
                egui::ScrollArea::vertical()
                    .id_salt("render_monitor_scroll")
                    .show(&mut right[0], |ui| self.draw_render_monitor(ui));
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("render_compact_scroll")
                .show(ui, |ui| {
                    self.draw_render_settings(ui);
                    ui.separator();
                    self.draw_render_monitor(ui);
                });
        }
    }

    fn selected_name(paths: &[PathBuf], index: Option<usize>, empty: &str) -> String {
        index
            .and_then(|index| paths.get(index))
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| empty.to_owned())
    }
}

impl Drop for GoosyApp {
    fn drop(&mut self) {
        let _ = terminate_render_child(&mut self.render_child);
        if let Some(mut task) = self.internal_render.take() {
            let _ = task.control.send("stop".to_owned());
            if let Some(thread) = task.thread.take() {
                let _ = thread.join();
            }
        }
        self.cleanup_preview_directory();
    }
}

impl eframe::App for GoosyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_render(ui.ctx());
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("GoosyRenderer");
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.page == AppPage::Render, "渲染设置")
                    .clicked()
                {
                    self.page = AppPage::Render;
                }
                if ui
                    .selectable_label(self.page == AppPage::Lyrics, "歌词行控制")
                    .clicked()
                {
                    self.page = AppPage::Lyrics;
                }
            });
            ui.separator();
            if self.page == AppPage::Render {
                self.draw_render_page(ui);
            } else {
                self.draw_lyrics_page(ui);
            }
        });
    }
}

fn scan_sibling_assets(audio: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(parent) = audio.parent() else {
        return (Vec::new(), Vec::new());
    };
    let Some(stem) = audio.file_stem().and_then(|stem| stem.to_str()) else {
        return (Vec::new(), Vec::new());
    };
    let target_stem = stem.to_ascii_lowercase();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return (Vec::new(), Vec::new());
    };
    let mut lyrics = Vec::new();
    let mut covers = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path == audio {
            continue;
        }
        let Some(candidate_stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if candidate_stem.to_ascii_lowercase() != target_stem {
            continue;
        }
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if has_extension(extension, LYRIC_EXTENSIONS) {
            lyrics.push(path);
        } else if has_extension(extension, COVER_EXTENSIONS) {
            covers.push(path);
        }
    }
    lyrics.sort();
    covers.sort();
    (lyrics, covers)
}

fn has_extension(extension: &str, accepted: &[&str]) -> bool {
    accepted
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

const CREDIT_MARKERS: &[&str] = &[
    "歌手", "作曲", "作词", "作詞", "演唱", "编曲", "制作", "原唱", "词：", "曲：", "词:", "曲:",
    "artist", "singer", "composer", "lyricist", "arranger", "producer",
];

fn detect_credit_lines(lines: &[lrc::LyricLine]) -> Vec<bool> {
    let mut excluded = vec![false; lines.len()];
    if lines.is_empty() {
        return excluded;
    }
    let edge_count = ((lines.len() + 9) / 10).clamp(2, 8).min(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let in_edge = index < edge_count || index >= lines.len() - edge_count;
        let main = line.text.to_lowercase();
        let translation = line
            .translation
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        excluded[index] = in_edge
            && CREDIT_MARKERS
                .iter()
                .any(|marker| main.contains(marker) || translation.contains(marker));
    }
    excluded
}

#[cfg(test)]
mod tests {
    use super::{
        COVER_EXTENSIONS, GoosyApp, LYRIC_EXTENSIONS, PreviewEvent, StylePreviewKey,
        StylePreviewRequest, StylePreviewScene, detect_credit_lines, has_extension,
        new_preview_directory, parse_preview_event, render_page_uses_columns, render_style_preview,
        spawn_style_preview_worker, style_preview_dimensions, terminate_render_child,
    };
    use libgoosy::LyricsStyle;
    use libgoosy::lrc::LyricLine;

    fn line(text: &str) -> LyricLine {
        LyricLine {
            start_ms: 0,
            end_ms: 1_000,
            text: text.to_owned(),
            translation: None,
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: None,
            words: Vec::new(),
        }
    }

    fn preview_scene(width: u32, height: u32) -> StylePreviewScene {
        StylePreviewScene {
            width,
            height,
            title: "Preview title".to_owned(),
            cover_path: None,
            embedded_cover_audio: None,
            render_translation: true,
            render_background_vocal: true,
        }
    }

    fn preview_key(style: [u32; 7], scene: StylePreviewScene) -> StylePreviewKey {
        StylePreviewKey { style, scene }
    }

    fn test_cover_png() -> Vec<u8> {
        use skia_safe::{EncodedImageFormat, ImageInfo, Pixmap};

        let info = ImageInfo::new_n32_premul((2, 2), None);
        let mut pixels = [
            40, 80, 220, 255, 40, 80, 220, 255, 40, 80, 220, 255, 40, 80, 220, 255,
        ];
        let pixmap = Pixmap::new(&info, &mut pixels, 8).unwrap();
        pixmap.encode(EncodedImageFormat::PNG, 100).unwrap()
    }

    #[test]
    fn accepts_case_insensitive_sibling_extensions() {
        assert!(has_extension("TTML", LYRIC_EXTENSIONS));
        assert!(has_extension("JPEG", COVER_EXTENSIONS));
        assert!(has_extension("webp", COVER_EXTENSIONS));
        assert!(!has_extension("mp3", COVER_EXTENSIONS));
    }

    #[test]
    fn detects_credit_markers_only_at_edges() {
        let lines = vec![
            line("歌手：Someone"),
            line("普通歌词"),
            line("作词：Someone"),
        ];
        assert_eq!(detect_credit_lines(&lines), vec![true, false, true]);
    }

    #[test]
    fn does_not_exclude_credit_word_in_middle_of_song() {
        let mut lines = vec![line("普通歌词"); 10];
        lines[5] = line("作曲的旋律");
        assert!(!detect_credit_lines(&lines)[5]);
    }

    #[test]
    fn parses_complete_preview_events_only() {
        assert_eq!(
            parse_preview_event("GOOSY_PREVIEW 45 640 360"),
            Some(PreviewEvent {
                frame: 45,
                width: 640,
                height: 360,
            })
        );
        assert_eq!(parse_preview_event("GOOSY_PREVIEW 45 0 360"), None);
        assert_eq!(parse_preview_event("GOOSY_PREVIEW 45 640"), None);
        assert_eq!(parse_preview_event("GOOSY_PROGRESS 45 300 1.0"), None);
    }

    #[test]
    fn random_sample_selects_a_bounded_lyrics_window() {
        let mut app = GoosyApp::default();
        app.lyric_lines = vec![line("first"), line("second")];
        app.lyric_lines[0].start_ms = 20_000;
        app.lyric_lines[0].end_ms = 21_000;
        app.lyric_lines[1].start_ms = 40_000;
        app.lyric_lines[1].end_ms = 41_000;

        let (start, end) = app.random_sample_range().unwrap();
        assert_eq!(end - start, 15_000);
        assert!(start <= 27_000);
        assert!(end <= 42_000);
    }

    #[test]
    fn random_sample_returns_none_without_nonempty_lyrics() {
        let mut app = GoosyApp::default();
        app.lyric_lines = vec![line("   ")];
        assert_eq!(app.random_sample_range(), None);
    }

    #[test]
    fn terminates_a_running_render_child() {
        if std::env::var_os("GOOSY_TERMINATION_FIXTURE").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        let executable = std::env::current_exe().unwrap();
        let child = std::process::Command::new(executable)
            .args([
                "--exact",
                "gui::tests::terminates_a_running_render_child",
                "--nocapture",
            ])
            .env("GOOSY_TERMINATION_FIXTURE", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = Some(child);

        assert!(terminate_render_child(&mut child).unwrap());
        assert!(child.is_none());
    }

    #[test]
    fn loads_preview_into_texture_and_removes_staging_frame() {
        let directory = new_preview_directory();
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("7.rgba");
        std::fs::write(&path, [255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let mut app = GoosyApp::default();
        app.preview_directory = Some(directory);
        let context = eframe::egui::Context::default();

        app.load_preview(
            &context,
            PreviewEvent {
                frame: 7,
                width: 2,
                height: 1,
            },
        );

        assert_eq!(app.preview_frame, 7);
        assert_eq!(app.preview_texture.as_ref().unwrap().size(), [2, 1]);
        assert!(!path.exists());
    }

    #[test]
    fn render_page_switches_to_a_scrollable_single_column_on_narrow_windows() {
        assert!(!render_page_uses_columns(700.0));
        assert!(render_page_uses_columns(760.0));
        assert!(render_page_uses_columns(960.0));
    }

    #[test]
    fn render_page_stays_within_default_and_minimum_viewports() {
        for size in [
            eframe::egui::vec2(960.0, 620.0),
            eframe::egui::vec2(700.0, 480.0),
        ] {
            let context = eframe::egui::Context::default();
            let input = eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO,
                    size,
                )),
                ..Default::default()
            };
            let mut app = GoosyApp::default();

            let mut output = context.run_ui(input, |ui| app.draw_render_page(ui));
            output.textures_delta.clear();

            let used = context.globally_used_rect();
            assert!(
                used.width() <= size.x + 1.0,
                "used={used:?}, viewport={size:?}"
            );
            assert!(
                used.height() <= size.y + 1.0,
                "used={used:?}, viewport={size:?}"
            );
        }
    }

    #[test]
    fn style_preview_pixels_change_with_layout_parameters() {
        let scene = preview_scene(1_920, 1_080);
        let compact = render_style_preview(
            LyricsStyle {
                line_height_scale: 0.8,
                group_gap_scale: 0.0,
                translation_gap_scale: 0.0,
                background_gap_scale: 0.0,
                horizontal_padding_scale: 0.0,
                ..LyricsStyle::default()
            },
            &scene,
            None,
        )
        .unwrap();
        let expanded = render_style_preview(
            LyricsStyle {
                line_height_scale: 1.8,
                group_gap_scale: 2.0,
                translation_gap_scale: 2.0,
                background_gap_scale: 2.0,
                horizontal_padding_scale: 2.0,
                ..LyricsStyle::default()
            },
            &scene,
            None,
        )
        .unwrap();

        assert_eq!(compact.size, expanded.size);
        assert!(
            compact
                .pixels
                .iter()
                .zip(&expanded.pixels)
                .any(|(left, right)| left != right)
        );
    }
    #[test]
    fn style_preview_worker_converges_to_the_latest_request() {
        let (sender, receiver) = spawn_style_preview_worker();
        let context = eframe::egui::Context::default();
        let first_key = preview_key([100; 7], preview_scene(1_920, 1_080));
        let final_key = preview_key([120, 140, 180, 50, 160, 130, 0], preview_scene(1_280, 720));
        sender
            .send(StylePreviewRequest {
                key: first_key,
                style: LyricsStyle::default(),
                context: context.clone(),
            })
            .unwrap();
        sender
            .send(StylePreviewRequest {
                key: final_key.clone(),
                style: LyricsStyle {
                    font_scale: 1.2,
                    line_height_scale: 1.4,
                    group_gap_scale: 1.8,
                    translation_gap_scale: 0.5,
                    background_gap_scale: 1.6,
                    horizontal_padding_scale: 1.3,
                    debug_overlays: false,
                },
                context,
            })
            .unwrap();

        loop {
            let output = receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            if output.key == final_key {
                assert!(output.image.is_ok());
                break;
            }
        }
    }

    #[test]
    fn style_preview_uses_output_aspect_ratio_and_selected_cover() {
        assert_eq!(style_preview_dimensions(1_080, 1_920), (203, 360));
        let landscape = preview_scene(1_920, 1_080);
        let without_cover = render_style_preview(LyricsStyle::default(), &landscape, None).unwrap();
        let cover_png = test_cover_png();
        let with_cover =
            render_style_preview(LyricsStyle::default(), &landscape, Some(&cover_png)).unwrap();
        let split_x = (with_cover.size[0] as f32 * 0.381_966_011_25) as usize;
        let cover_center = split_x / 2 + with_cover.size[1] / 2 * with_cover.size[0];

        assert_eq!(with_cover.size, [640, 360]);
        assert_ne!(
            with_cover.pixels[cover_center],
            without_cover.pixels[cover_center]
        );
    }

    #[test]
    fn style_preview_font_size_is_derived_from_selected_output_resolution() {
        let low_resolution =
            render_style_preview(LyricsStyle::default(), &preview_scene(640, 360), None).unwrap();
        let full_hd =
            render_style_preview(LyricsStyle::default(), &preview_scene(1_920, 1_080), None)
                .unwrap();
        let bright_neutral_pixels = |image: &eframe::egui::ColorImage| {
            image
                .pixels
                .iter()
                .filter(|pixel| {
                    let [red, green, blue, _] = pixel.to_array();
                    red > 100
                        && red.abs_diff(green) <= 20
                        && red.abs_diff(blue) <= 20
                        && green.abs_diff(blue) <= 20
                })
                .count()
        };

        assert_eq!(low_resolution.size, full_hd.size);
        assert!(bright_neutral_pixels(&low_resolution) > bright_neutral_pixels(&full_hd));
    }
}
