use eframe::egui;
use libgoosy::{LyricFormat, lrc, video};
use rfd::FileDialog;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

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
    use_embedded_cover: bool,
    no_audio: bool,
    render_child: Option<Child>,
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
            use_embedded_cover: true,
            no_audio: false,
            render_child: None,
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

    fn stop_render(&mut self) {
        let result = terminate_render_child(&mut self.render_child);
        self.progress_receiver = None;
        self.log_receiver = None;
        self.cleanup_preview_directory();
        self.render_stage = "渲染已急停".to_owned();
        self.status = match result {
            Ok(true) => "渲染已急停；输出文件可能不完整".to_owned(),
            Ok(false) => "渲染进程已经结束".to_owned(),
            Err(error) => format!("急停渲染失败：{error}"),
        };
    }

    fn start_render(&mut self) {
        let (Some(audio), Some(output)) = (self.audio.clone(), self.output.clone()) else {
            self.status = "请先选择音频和输出".to_owned();
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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
                    self.status = "渲染完成".to_owned();
                } else if self.render_log.is_empty() {
                    self.status = format!("渲染失败：{status}");
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
        let running = self.render_child.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("开始渲染"))
                .clicked()
            {
                self.start_render();
            }
            if running
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
            if self.render_child.is_some() {
                ui.spinner();
            }
        });
        if self.render_child.is_some() {
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
        } else if !running {
            ui.weak("开始渲染后将在这里显示实时画面。");
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
        COVER_EXTENSIONS, GoosyApp, LYRIC_EXTENSIONS, PreviewEvent, detect_credit_lines,
        has_extension, new_preview_directory, parse_preview_event, render_page_uses_columns,
        terminate_render_child,
    };
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
}
