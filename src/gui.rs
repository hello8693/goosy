use eframe::egui;
use rfd::FileDialog;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

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

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 620.0])
            .with_min_inner_size([680.0, 500.0]),
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
    use_embedded_cover: bool,
    no_audio: bool,
    render_child: Option<Child>,
    progress_receiver: Option<mpsc::Receiver<String>>,
    progress: f32,
    speed_history: VecDeque<(f32, f32)>,
    current_speed: f32,
    last_progress_sample: Option<(f32, f32)>,
    status: String,
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
            use_embedded_cover: true,
            no_audio: false,
            render_child: None,
            progress_receiver: None,
            progress: 0.0,
            speed_history: VecDeque::new(),
            current_speed: 0.0,
            last_progress_sample: None,
            status: String::new(),
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
        let metadata_title = crate::video::audio_metadata(&path)
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
            .add_filter("Lyrics", &["lrc", "ttml", "xml", "yrc"])
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

    fn start_render(&mut self) {
        let (Some(audio), Some(output)) = (self.audio.clone(), self.output.clone()) else {
            self.status = "请先选择音频和输出".to_owned();
            return;
        };
        let lyrics = self
            .selected_lyrics
            .and_then(|index| self.lyrics_candidates.get(index).cloned());
        let Ok(executable) = std::env::current_exe() else {
            self.status = "无法定位 goosy 可执行文件".to_owned();
            return;
        };
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
            .arg("--format")
            .arg("auto")
            .arg("--progress-events")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if self.no_audio {
            command.arg("--no-audio");
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
                let receiver = child.stdout.take().map(|stdout| {
                    let (sender, receiver) = mpsc::channel();
                    thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().filter_map(Result::ok) {
                            let _ = sender.send(line);
                        }
                    });
                    receiver
                });
                self.render_child = Some(child);
                self.progress_receiver = receiver;
                self.progress = 0.0;
                self.speed_history.clear();
                self.current_speed = 0.0;
                self.last_progress_sample = None;
            }
            Err(error) => {
                self.status = format!("启动渲染失败：{error}");
            }
        }
    }
    fn poll_render(&mut self, context: &egui::Context) {
        if let Some(receiver) = &self.progress_receiver {
            for line in receiver.try_iter() {
                let mut fields = line.split_whitespace();
                if fields.next() == Some("GOOSY_PROGRESS") {
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
                            self.current_speed = if let Some((previous_done, previous_elapsed)) =
                                self.last_progress_sample
                            {
                                let frame_delta = done - previous_done;
                                let time_delta = (elapsed - previous_elapsed).max(0.001);
                                frame_delta / time_delta
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
            }
        }
        let result = self.render_child.as_mut().map(|child| child.try_wait());
        match result {
            Some(Ok(Some(status))) => {
                self.render_child = None;
                self.progress_receiver = None;
                self.progress = if status.success() { 1.0 } else { self.progress };
                self.status = if status.success() {
                    "渲染完成".to_owned()
                } else {
                    format!("渲染失败：{status}")
                };
            }
            Some(Ok(None)) => context.request_repaint_after(Duration::from_millis(100)),
            Some(Err(error)) => {
                self.render_child = None;
                self.progress_receiver = None;
                self.status = format!("读取渲染状态失败：{error}");
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

    fn selected_name(paths: &[PathBuf], index: Option<usize>, empty: &str) -> String {
        index
            .and_then(|index| paths.get(index))
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| empty.to_owned())
    }
}

impl eframe::App for GoosyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_render(ui.ctx());
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("GoosyRenderer");
            ui.label("选择音频后，自动匹配同名歌词与封面。");
            ui.add_space(8.0);

            egui::Grid::new("source_grid")
                .num_columns(2)
                .spacing([12.0, 12.0])
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
                        if ui.button("选择音频…").clicked() {
                            self.choose_audio();
                        }
                    });
                    ui.end_row();

                    ui.label("歌词");
                    ui.horizontal(|ui| {
                        if self.lyrics_candidates.is_empty() {
                            ui.label("未找到同名歌词，将读取音频内嵌歌词");
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
                        if ui.button("手动选择…").clicked() {
                            self.choose_lyrics();
                        }
                    });
                    ui.end_row();

                    ui.label("封面");
                    ui.horizontal(|ui| {
                        let selected = Self::selected_name(
                            &self.cover_candidates,
                            self.selected_cover,
                            "不显示封面",
                        );
                        if !self.cover_candidates.is_empty() {
                            egui::ComboBox::from_id_salt("cover_combo")
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.selected_cover,
                                        None,
                                        "不显示封面",
                                    );
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
                        } else {
                            ui.label(if self.use_embedded_cover {
                                "将使用音频内嵌封面"
                            } else {
                                "未找到同名封面"
                            });
                        }
                        if ui.button("手动选择…").clicked() {
                            self.choose_cover();
                        }
                    });
                    ui.end_row();

                    ui.label("歌曲名称");
                    ui.text_edit_singleline(&mut self.title);
                    ui.end_row();

                    ui.label("输出文件");
                    ui.horizontal(|ui| {
                        ui.label(
                            self.output
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "选择音频后自动生成".to_owned()),
                        );
                        if ui.button("选择输出…").clicked() {
                            if let Some(path) =
                                FileDialog::new().set_file_name("lyrics.mp4").save_file()
                            {
                                self.output = Some(path);
                            }
                        }
                    });
                    ui.end_row();
                });

            ui.separator();
            ui.collapsing("输出设置", |ui| {
                ui.horizontal(|ui| {
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
                ui.checkbox(&mut self.use_embedded_cover, "默认使用音频内嵌封面");
                ui.checkbox(&mut self.no_audio, "不输出音频");
            });

            ui.add_space(12.0);
            let running = self.render_child.is_some();
            if ui
                .add_enabled(!running, egui::Button::new("开始渲染"))
                .clicked()
            {
                self.start_render();
            }
            if running {
                ui.spinner();
                ui.add(egui::ProgressBar::new(self.progress).show_percentage());
            }
            if !self.speed_history.is_empty() {
                self.draw_speed_graph(ui);
            }
            if !self.status.is_empty() {
                ui.add_space(8.0);
                ui.label(&self.status);
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

#[cfg(test)]
mod tests {
    use super::{COVER_EXTENSIONS, LYRIC_EXTENSIONS, has_extension};

    #[test]
    fn accepts_case_insensitive_sibling_extensions() {
        assert!(has_extension("TTML", LYRIC_EXTENSIONS));
        assert!(has_extension("JPEG", COVER_EXTENSIONS));
        assert!(has_extension("webp", COVER_EXTENSIONS));
        assert!(!has_extension("mp3", COVER_EXTENSIONS));
    }
}
