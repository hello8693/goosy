use anyhow::{Context, Result};
use skia_safe::canvas::SaveLayerRec;
use skia_safe::image_filters;
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, RectHeightStyle, RectWidthStyle,
    TextAlign, TextBox, TextStyle,
};
use skia_safe::{Canvas, Color, FontMgr, FontStyle, ImageFilter, Paint, PathBuilder, Point, Rect};

use crate::easing::bez_in;
use crate::geometry::Viewport;
use crate::layout::Layout;
use crate::lrc::{LyricLine, scan_end_ms};
const TRANSLATION_GAP_EM: f32 = 0.15;
const BACKGROUND_GAP_EM: f32 = 0.12;
const BACKGROUND_FONT_SCALE: f32 = 0.7;
const GROUP_GAP_EM: f32 = 0.45;
const HIGHLIGHT_FADE_MS: f32 = 140.0;
pub struct LyricsRenderer {
    base_paragraphs: Vec<Paragraph>,
    solid_paragraphs: Vec<Paragraph>,
    word_boxes: Vec<Vec<Vec<TextBox>>>,
    translation_paragraphs: Vec<Option<Paragraph>>,
    background_lines: Vec<Option<LyricLine>>,
    background_paragraphs: Vec<Option<Paragraph>>,
    background_solid_paragraphs: Vec<Option<Paragraph>>,
    background_word_boxes: Vec<Vec<Vec<TextBox>>>,
    background_translation_paragraphs: Vec<Option<Paragraph>>,
    margin_x: f32,
    text_width: f32,
    main_font_size: f32,
    background_font_size: f32,
    group_heights: Vec<f64>,
    group_gap: f64,
    dot_size: f32,
    dot_gap: f32,
    dot_margin: f32,
    lyric_blurs: Vec<Option<ImageFilter>>,
}

impl LyricsRenderer {
    pub fn new(lines: &[LyricLine], viewport: Viewport) -> Result<Self> {
        let margin_x = viewport.x;
        let text_width = viewport.width.max(1.0);
        let main_font_size = if viewport.width <= 768.0 {
            (viewport.width * 0.08).max(12.0)
        } else {
            (viewport.height * 0.05)
                .max(viewport.width * 0.025)
                .max(12.0)
        };
        let translation_font_size = (main_font_size * 0.5).max(10.0);
        let background_font_size = (main_font_size * BACKGROUND_FONT_SCALE).max(10.0);
        let background_translation_font_size = (background_font_size * 0.5).max(10.0);
        let base_paragraphs = build_paragraphs(
            lines,
            Color::from_argb(102, 255, 255, 255),
            text_width,
            main_font_size,
        )?;
        let solid_paragraphs = build_paragraphs(lines, Color::WHITE, text_width, main_font_size)?;
        let word_boxes = build_word_geometry(lines, &base_paragraphs);
        let translation_paragraphs = build_translation_paragraphs(
            lines,
            text_width,
            translation_font_size,
            main_font_size * 1.3,
        )?;
        let background_lines = lines
            .iter()
            .map(|line| {
                line.background_vocal.as_ref().map(|background| LyricLine {
                    start_ms: background.start_ms,
                    end_ms: background.end_ms,
                    text: background.text.clone(),
                    translation: background.translation.clone(),
                    agent_id: line.agent_id.clone(),
                    is_duet: line.is_duet,
                    is_background: true,
                    background_vocal: None,
                    words: background.words.clone(),
                })
            })
            .collect::<Vec<_>>();
        let background_paragraphs = build_optional_paragraphs(
            &background_lines,
            Color::from_argb(102, 255, 255, 255),
            text_width,
            background_font_size,
        )?;
        let background_solid_paragraphs = build_optional_paragraphs(
            &background_lines,
            Color::WHITE,
            text_width,
            background_font_size,
        )?;
        let background_word_boxes =
            build_optional_word_geometry(&background_lines, &background_paragraphs);
        let background_translation_paragraphs = build_optional_translation_paragraphs(
            &background_lines,
            text_width,
            background_translation_font_size,
            background_font_size * 1.3,
        )?;
        let group_gap = main_font_size * GROUP_GAP_EM;
        let dot_size = (main_font_size * 0.5).max(6.0);
        let dot_gap = (main_font_size * 0.25).max(2.0);
        let dot_margin = main_font_size * 0.4;
        let group_heights = lines
            .iter()
            .enumerate()
            .map(|(index, _line)| {
                let translation_height = translation_paragraphs[index]
                    .as_ref()
                    .map(|paragraph| paragraph.height())
                    .unwrap_or(0.0);
                let translation_gap = if translation_height > 0.0 {
                    main_font_size * TRANSLATION_GAP_EM
                } else {
                    0.0
                };
                let background_height = background_paragraphs[index]
                    .as_ref()
                    .map(|paragraph| paragraph.height())
                    .unwrap_or(0.0);
                let background_translation_height = background_translation_paragraphs[index]
                    .as_ref()
                    .map(|paragraph| paragraph.height())
                    .unwrap_or(0.0);
                let background_gap = if background_height > 0.0 {
                    main_font_size * BACKGROUND_GAP_EM
                } else {
                    0.0
                };
                let background_translation_gap = if background_translation_height > 0.0 {
                    background_font_size * TRANSLATION_GAP_EM
                } else {
                    0.0
                };
                (base_paragraphs[index].height()
                    + translation_height
                    + translation_gap
                    + background_gap
                    + background_height
                    + background_translation_gap
                    + background_translation_height) as f64
            })
            .collect();
        let mut lyric_blurs = vec![None];
        for sigma in 1..=5 {
            lyric_blurs.push(Some(
                image_filters::blur((sigma as f32, sigma as f32), None, None, None)
                    .context("create lyric blur filter")?,
            ));
        }
        Ok(Self {
            base_paragraphs,
            solid_paragraphs,
            word_boxes,
            translation_paragraphs,
            background_lines,
            background_paragraphs,
            background_solid_paragraphs,
            background_word_boxes,
            background_translation_paragraphs,
            margin_x,
            text_width,
            main_font_size,
            background_font_size,
            group_heights,
            group_gap: group_gap as f64,
            dot_size,
            dot_gap,
            dot_margin,
            lyric_blurs,
        })
    }

    pub fn draw(
        &self,
        canvas: &Canvas,
        lines: &[LyricLine],
        layout: &Layout,
        t_ms: u64,
        height: u32,
    ) -> Result<()> {
        self.draw_interlude_dots(canvas, lines, layout, t_ms);

        let active_idx = layout.active_idx();
        for (index, paragraph) in self.base_paragraphs.iter().enumerate() {
            if lines[index].text.is_empty()
                && self.translation_paragraphs[index].is_none()
                && self.background_paragraphs[index].is_none()
            {
                continue;
            }
            let top_y = layout.pos_y[index].current_position() as f32;
            let translation_height = self.translation_paragraphs[index]
                .as_ref()
                .map(|paragraph| paragraph.height())
                .unwrap_or(0.0);
            let extent = paragraph.height() + translation_height + self.main_font_size * 0.5;
            if top_y + extent < -self.main_font_size || top_y > height as f32 + self.main_font_size
            {
                continue;
            }
            let scale = layout.scale[index].current_position() as f32;
            let opacity = edge_opacity(top_y, height as f32, index == active_idx);
            let activation = ((t_ms.saturating_sub(lines[index].start_ms) as f32)
                / HIGHLIGHT_FADE_MS)
                .clamp(0.0, 1.0);
            let highlight_strength = if index == active_idx {
                (1.0_f32 - ((1.0_f32 - scale) / 0.03_f32)).clamp(0.0, 1.0) * activation
            } else {
                0.0
            };
            let anchor_x = if lines[index].is_duet {
                self.margin_x + self.text_width
            } else {
                self.margin_x
            };
            let transform = canvas.save();
            canvas.translate((anchor_x, top_y));
            canvas.scale((scale, scale));
            canvas.translate((-anchor_x, -top_y));
            let blur_sigma = ((index as isize - active_idx as isize).abs() as f32).min(5.0);
            if blur_sigma > 0.01 {
                let blur_filter = self.lyric_blurs[blur_sigma as usize]
                    .as_ref()
                    .expect("cached lyric blur filter");
                let mut layer_paint = Paint::default();
                layer_paint
                    .set_alpha_f(opacity)
                    .set_image_filter(blur_filter.clone());
                let layer_rec = SaveLayerRec::default().paint(&layer_paint);
                let layer = canvas.save_layer(&layer_rec);
                draw_line(
                    canvas,
                    paragraph,
                    &self.solid_paragraphs[index],
                    &self.word_boxes[index],
                    self.translation_paragraphs[index].as_ref(),
                    &lines[index],
                    index == active_idx,
                    highlight_strength,
                    t_ms,
                    self.margin_x,
                    self.text_width,
                    top_y,
                    self.main_font_size,
                );
                canvas.restore_to_count(layer);
            } else {
                if opacity >= 0.999 {
                    draw_line(
                        canvas,
                        paragraph,
                        &self.solid_paragraphs[index],
                        &self.word_boxes[index],
                        self.translation_paragraphs[index].as_ref(),
                        &lines[index],
                        true,
                        highlight_strength,
                        t_ms,
                        self.margin_x,
                        self.text_width,
                        top_y,
                        self.main_font_size,
                    );
                } else {
                    let layer = canvas.save_layer_alpha_f(None, opacity);
                    draw_line(
                        canvas,
                        paragraph,
                        &self.solid_paragraphs[index],
                        &self.word_boxes[index],
                        self.translation_paragraphs[index].as_ref(),
                        &lines[index],
                        true,
                        highlight_strength,
                        t_ms,
                        self.margin_x,
                        self.text_width,
                        top_y,
                        self.main_font_size,
                    );
                    canvas.restore_to_count(layer);
                }
            }
            if let (Some(background), Some(background_solid)) = (
                self.background_paragraphs[index].as_ref(),
                self.background_solid_paragraphs[index].as_ref(),
            ) {
                let translation_gap = if translation_height > 0.0 {
                    self.main_font_size * TRANSLATION_GAP_EM
                } else {
                    0.0
                };
                let background_top = top_y
                    + paragraph.height()
                    + translation_height
                    + translation_gap
                    + self.main_font_size * BACKGROUND_GAP_EM;
                let background_line = self.background_lines[index]
                    .as_ref()
                    .expect("background line initialized");
                draw_line(
                    canvas,
                    background,
                    background_solid,
                    &self.background_word_boxes[index],
                    self.background_translation_paragraphs[index].as_ref(),
                    background_line,
                    index == active_idx,
                    highlight_strength,
                    t_ms,
                    self.margin_x,
                    self.text_width,
                    background_top,
                    self.background_font_size,
                );
            }
            canvas.restore_to_count(transform);
        }
        Ok(())
    }
    fn draw_interlude_dots(
        &self,
        canvas: &Canvas,
        lines: &[LyricLine],
        layout: &Layout,
        t_ms: u64,
    ) {
        let Some(interlude) = layout.interlude() else {
            return;
        };
        let Some(line) = lines.get(interlude.next_idx) else {
            return;
        };
        let duration = interlude.end_ms.saturating_sub(interlude.start_ms).max(1) as f64;
        let current = t_ms.saturating_sub(interlude.start_ms) as f64;
        let remaining = duration - current;
        let breathe_duration = duration / (duration / 1_500.0).ceil().max(1.0);
        let mut scale =
            (1.5 * std::f64::consts::PI - (current / breathe_duration) * 2.0).sin() / 20.0 + 1.0;
        if current < 2_000.0 {
            scale *= ease_out_expo((current / 2_000.0).clamp(0.0, 1.0));
        }
        if remaining < 750.0 {
            scale *= 1.0 - ease_in_out_back(((750.0 - remaining) / 750.0 / 2.0).clamp(0.0, 1.0));
        }
        let mut global_opacity = if current < 500.0 {
            0.0
        } else if current < 1_000.0 {
            (current - 500.0) / 500.0
        } else {
            1.0
        };
        if remaining < 375.0 {
            global_opacity *= (remaining / 375.0).clamp(0.0, 1.0);
        }
        let dots_duration = (duration - 750.0).max(1.0);
        let dot_opacity = [0.0, 1.0, 2.0].map(|delay| {
            let opacity = (((current - dots_duration * delay / 3.0) * 3.0 / dots_duration) * 0.75)
                .clamp(0.25, 1.0);
            (global_opacity * opacity).clamp(0.0, 1.0)
        });
        if dot_opacity.iter().all(|opacity| *opacity <= 0.0) {
            return;
        }
        let Some(slot_top) = layout.interlude_top_y(interlude.next_idx) else {
            return;
        };
        let width = self.dot_size * 3.0 + self.dot_gap * 2.0;
        let left = if line.is_duet {
            self.margin_x + self.text_width - width
        } else {
            self.margin_x
        };
        let center = Point::new(
            left + width / 2.0,
            slot_top + self.dot_margin + self.dot_size / 2.0,
        );
        let transform = canvas.save();
        canvas.translate((center.x, center.y));
        canvas.scale((scale as f32 * 0.7, scale as f32 * 0.7));
        canvas.translate((-center.x, -center.y));
        let mut paint = Paint::default();
        paint.set_color(Color::WHITE);
        for (index, opacity) in dot_opacity.into_iter().enumerate() {
            paint.set_alpha_f(opacity as f32);
            canvas.draw_circle(
                Point::new(
                    left + self.dot_size / 2.0 + index as f32 * (self.dot_size + self.dot_gap),
                    center.y,
                ),
                self.dot_size / 2.0,
                &paint,
            );
        }
        canvas.restore_to_count(transform);
    }

    pub fn interlude_slot_height(&self) -> f64 {
        (self.dot_size + self.dot_margin * 2.0) as f64
    }

    pub fn group_heights(&self) -> &[f64] {
        &self.group_heights
    }

    pub fn group_gap(&self) -> f64 {
        self.group_gap
    }
}
fn ease_in_out_back(x: f64) -> f64 {
    let c1 = 1.70158;
    let c2 = c1 * 1.525;
    if x < 0.5 {
        ((2.0 * x).powi(2) * ((c2 + 1.0) * 2.0 * x - c2)) / 2.0
    } else {
        ((2.0 * x - 2.0).powi(2) * ((c2 + 1.0) * (x * 2.0 - 2.0) + c2) + 2.0) / 2.0
    }
}

fn ease_out_expo(x: f64) -> f64 {
    if (x - 1.0).abs() < f64::EPSILON {
        1.0
    } else {
        1.0 - 2.0_f64.powf(-10.0 * x)
    }
}
fn draw_line(
    canvas: &Canvas,
    paragraph: &Paragraph,
    active_paragraph: &Paragraph,
    word_boxes: &[Vec<TextBox>],
    translation: Option<&Paragraph>,
    line: &LyricLine,
    active: bool,
    highlight_strength: f32,
    t_ms: u64,
    margin_x: f32,
    text_width: f32,
    top_y: f32,
    main_font_size: f32,
) {
    let position = Point::new(margin_x, top_y);
    if !active {
        paragraph.paint(canvas, position);
        draw_translation(
            canvas,
            translation,
            margin_x,
            top_y + paragraph.height() + main_font_size * TRANSLATION_GAP_EM,
        );
        return;
    }
    paragraph.paint(canvas, position);
    if highlight_strength <= 0.0 {
        draw_translation(
            canvas,
            translation,
            margin_x,
            top_y + paragraph.height() + main_font_size * TRANSLATION_GAP_EM,
        );
        return;
    }
    let feather_width = (main_font_size * 0.08).clamp(2.0, 8.0);
    let mut core_mask = PathBuilder::new();
    let mut feather_masks = [PathBuilder::new(), PathBuilder::new(), PathBuilder::new()];
    let mut has_core = false;
    let mut has_feather = [false; 3];
    if word_boxes.len() == line.words.len() && !line.words.is_empty() {
        for (word, boxes) in line.words.iter().zip(word_boxes) {
            let total_width = boxes
                .iter()
                .map(|text_box| text_box.rect.width())
                .sum::<f32>();
            let progress = bez_in(word_progress(word.start_ms, word.end_ms, t_ms)) as f32;
            let fill_width = total_width * progress;
            let feather = if progress < 0.999 {
                feather_width.min(fill_width)
            } else {
                0.0
            };
            let core_end = fill_width - feather;
            let mut cursor = 0.0;
            for text_box in boxes {
                let rect = text_box.rect;
                let box_start = cursor;
                let box_end = cursor + rect.width();
                let core_start = box_start.max(0.0);
                let core_stop = core_end.min(box_end);
                if core_stop > core_start {
                    core_mask.add_rect(
                        Rect::from_xywh(
                            rect.left() + margin_x + core_start - box_start,
                            top_y + rect.top(),
                            core_stop - core_start,
                            rect.height(),
                        ),
                        None,
                        None,
                    );
                    has_core = true;
                }
                for level in 0..3 {
                    let edge_start = core_end + feather * level as f32 / 3.0;
                    let edge_stop = core_end + feather * (level + 1) as f32 / 3.0;
                    let start = edge_start.max(box_start);
                    let stop = edge_stop.min(box_end).min(fill_width);
                    if stop > start {
                        feather_masks[level].add_rect(
                            Rect::from_xywh(
                                rect.left() + margin_x + start - box_start,
                                top_y + rect.top(),
                                stop - start,
                                rect.height(),
                            ),
                            None,
                            None,
                        );
                        has_feather[level] = true;
                    }
                }
                cursor = box_end;
            }
        }
    } else {
        // Plain LRC has no word timing: scan the entire laid-out line as one unit.
        let scan_end = scan_end_ms(line);
        let duration = scan_end.saturating_sub(line.start_ms).max(1) as f64;
        let progress = ((t_ms.saturating_sub(line.start_ms) as f64) / duration).clamp(0.0, 1.0);
        let fill_width = text_width * bez_in(progress) as f32;
        let feather = if progress < 0.999 {
            feather_width.min(fill_width)
        } else {
            0.0
        };
        let core_end = (fill_width - feather).max(0.0);
        if core_end > 0.0 {
            core_mask.add_rect(
                Rect::from_xywh(margin_x, top_y, core_end, paragraph.height()),
                None,
                None,
            );
            has_core = true;
        }
        for level in 0..3 {
            let start = core_end + feather * level as f32 / 3.0;
            let stop = (core_end + feather * (level + 1) as f32 / 3.0).min(fill_width);
            if stop > start {
                feather_masks[level].add_rect(
                    Rect::from_xywh(margin_x + start, top_y, stop - start, paragraph.height()),
                    None,
                    None,
                );
                has_feather[level] = true;
            }
        }
    }
    if has_core {
        paint_highlight_mask(
            canvas,
            &core_mask.detach(),
            active_paragraph,
            position,
            highlight_strength,
        );
    }
    for level in 0..3 {
        if has_feather[level] {
            let edge_alpha = highlight_strength * (1.0 - (level as f32 + 0.5) / 3.0);
            paint_highlight_mask(
                canvas,
                &feather_masks[level].detach(),
                active_paragraph,
                position,
                edge_alpha,
            );
        }
    }
    draw_translation(
        canvas,
        translation,
        margin_x,
        top_y + paragraph.height() + main_font_size * TRANSLATION_GAP_EM,
    );
}

fn paint_highlight_mask(
    canvas: &Canvas,
    path: &skia_safe::Path,
    paragraph: &Paragraph,
    position: Point,
    alpha: f32,
) {
    if alpha <= 0.0 {
        return;
    }
    canvas.save();
    canvas.clip_path(path, None, true);
    if alpha >= 0.999 {
        paragraph.paint(canvas, position);
    } else {
        let layer = canvas.save_layer_alpha_f(None, alpha);
        paragraph.paint(canvas, position);
        canvas.restore_to_count(layer);
    }
    canvas.restore();
}

fn draw_translation(canvas: &Canvas, translation: Option<&Paragraph>, margin_x: f32, top_y: f32) {
    if let Some(paragraph) = translation {
        paragraph.paint(canvas, Point::new(margin_x, top_y));
    }
}

fn word_progress(start_ms: u64, end_ms: u64, t_ms: u64) -> f64 {
    if t_ms <= start_ms {
        return 0.0;
    }
    if t_ms >= end_ms {
        return 1.0;
    }
    let duration = end_ms.saturating_sub(start_ms).max(1) as f64;
    (t_ms.saturating_sub(start_ms) as f64 / duration).clamp(0.0, 1.0)
}
fn find_word_utf16_range(
    text: &str,
    word: &str,
    search_from: usize,
) -> Option<(usize, usize, usize)> {
    if word.is_empty() {
        return None;
    }
    let relative_start = text.get(search_from..)?.find(word)?;
    let start_byte = search_from + relative_start;
    let end_byte = start_byte + word.len();
    let start_utf16 = text[..start_byte].encode_utf16().count();
    let end_utf16 = start_utf16 + word.encode_utf16().count();
    Some((start_utf16, end_utf16, end_byte))
}

fn build_word_geometry(lines: &[LyricLine], paragraphs: &[Paragraph]) -> Vec<Vec<Vec<TextBox>>> {
    lines
        .iter()
        .zip(paragraphs)
        .map(|(line, paragraph)| {
            let mut search_from = 0usize;
            line.words
                .iter()
                .map(|word| {
                    let Some((start, end, next_search)) =
                        find_word_utf16_range(&line.text, &word.text, search_from)
                    else {
                        return Vec::new();
                    };
                    search_from = next_search;
                    paragraph.get_rects_for_range(
                        start..end,
                        RectHeightStyle::Tight,
                        RectWidthStyle::Tight,
                    )
                })
                .collect()
        })
        .collect()
}

fn build_optional_word_geometry(
    lines: &[Option<LyricLine>],
    paragraphs: &[Option<Paragraph>],
) -> Vec<Vec<Vec<TextBox>>> {
    lines
        .iter()
        .zip(paragraphs)
        .map(|(line, paragraph)| {
            let (Some(line), Some(paragraph)) = (line.as_ref(), paragraph.as_ref()) else {
                return Vec::new();
            };
            let mut search_from = 0usize;
            line.words
                .iter()
                .map(|word| {
                    let Some((start, end, next_search)) =
                        find_word_utf16_range(&line.text, &word.text, search_from)
                    else {
                        return Vec::new();
                    };
                    search_from = next_search;
                    paragraph.get_rects_for_range(
                        start..end,
                        RectHeightStyle::Tight,
                        RectWidthStyle::Tight,
                    )
                })
                .collect()
        })
        .collect()
}

fn build_optional_paragraphs(
    lines: &[Option<LyricLine>],
    color: Color,
    text_width: f32,
    font_size: f32,
) -> Result<Vec<Option<Paragraph>>> {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    lines
        .iter()
        .map(|line| {
            let Some(line) = line.as_ref() else {
                return Ok(None);
            };
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(if line.is_duet {
                TextAlign::Right
            } else {
                TextAlign::Left
            });
            let mut text_style = TextStyle::new();
            text_style.set_font_families(&[
                "PingFang SC",
                "Microsoft YaHei",
                "Noto Sans CJK SC",
                "sans-serif",
            ]);
            text_style.set_font_size(font_size);
            text_style.set_color(color);
            text_style.set_font_style(FontStyle::bold());
            text_style.set_height(1.3);
            text_style.set_height_override(true);
            paragraph_style.set_text_style(&text_style);
            let mut builder = ParagraphBuilder::new(&paragraph_style, collection.clone());
            builder.push_style(&text_style).add_text(&line.text);
            let mut paragraph = builder.build();
            paragraph.layout(text_width);
            Ok(Some(paragraph))
        })
        .collect()
}

fn build_optional_translation_paragraphs(
    lines: &[Option<LyricLine>],
    text_width: f32,
    font_size: f32,
    line_height: f32,
) -> Result<Vec<Option<Paragraph>>> {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    let mut text_style = TextStyle::new();
    text_style.set_font_families(&[
        "PingFang SC",
        "Microsoft YaHei",
        "Noto Sans CJK SC",
        "sans-serif",
    ]);
    text_style.set_font_size(font_size);
    text_style.set_color(Color::from_argb(77, 255, 255, 255));
    text_style.set_font_style(FontStyle::normal());
    text_style.set_height(line_height / font_size);
    text_style.set_height_override(true);
    lines
        .iter()
        .map(|line| {
            let Some(line) = line.as_ref() else {
                return Ok(None);
            };
            let Some(translation) = line.translation.as_deref() else {
                return Ok(None);
            };
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(if line.is_duet {
                TextAlign::Right
            } else {
                TextAlign::Left
            });
            paragraph_style.set_text_style(&text_style);
            let mut builder = ParagraphBuilder::new(&paragraph_style, collection.clone());
            builder.push_style(&text_style).add_text(translation);
            let mut paragraph = builder.build();
            paragraph.layout(text_width);
            Ok(Some(paragraph))
        })
        .collect()
}

fn build_paragraphs(
    lines: &[LyricLine],
    color: Color,
    text_width: f32,
    font_size: f32,
) -> Result<Vec<Paragraph>> {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    lines
        .iter()
        .map(|line| {
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(if line.is_duet {
                TextAlign::Right
            } else {
                TextAlign::Left
            });
            let mut text_style = TextStyle::new();
            text_style.set_font_families(&[
                "PingFang SC",
                "Microsoft YaHei",
                "Noto Sans CJK SC",
                "sans-serif",
            ]);
            text_style.set_font_size(font_size);
            text_style.set_color(color);
            text_style.set_font_style(FontStyle::bold());
            text_style.set_height(1.3);
            text_style.set_height_override(true);
            paragraph_style.set_text_style(&text_style);
            let mut builder = ParagraphBuilder::new(&paragraph_style, collection.clone());
            builder.push_style(&text_style).add_text(&line.text);
            let mut paragraph = builder.build();
            paragraph.layout(text_width);
            Ok(paragraph)
        })
        .collect()
}

fn build_translation_paragraphs(
    lines: &[LyricLine],
    text_width: f32,
    font_size: f32,
    line_height: f32,
) -> Result<Vec<Option<Paragraph>>> {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    let mut text_style = TextStyle::new();
    text_style.set_font_families(&[
        "PingFang SC",
        "Microsoft YaHei",
        "Noto Sans CJK SC",
        "sans-serif",
    ]);
    text_style.set_font_size(font_size);
    text_style.set_color(Color::from_argb(77, 255, 255, 255));
    text_style.set_font_style(FontStyle::normal());
    text_style.set_height(line_height / font_size);
    text_style.set_height_override(true);
    lines
        .iter()
        .map(|line| {
            let Some(translation) = line.translation.as_deref() else {
                return Ok(None);
            };
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(if line.is_duet {
                TextAlign::Right
            } else {
                TextAlign::Left
            });
            paragraph_style.set_text_style(&text_style);
            let mut builder = ParagraphBuilder::new(&paragraph_style, collection.clone());
            builder.push_style(&text_style).add_text(translation);
            let mut paragraph = builder.build();
            paragraph.layout(text_width);
            Ok(Some(paragraph))
        })
        .collect()
}

fn edge_opacity(top_y: f32, height: f32, active: bool) -> f32 {
    if active {
        return 1.0;
    }
    let fade_top = ((top_y - 0.05 * height) / (0.10 * height)).clamp(0.0, 1.0);
    let fade_bottom = ((height - 0.05 * height - top_y) / (0.10 * height)).clamp(0.0, 1.0);
    fade_top.min(fade_bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_cjk_and_emoji_to_utf16_offsets() {
        let text = "你 😀 hi";
        assert_eq!(find_word_utf16_range(text, "😀", 0), Some((2, 4, 8)));
        assert_eq!(
            find_word_utf16_range(text, "hi", 8),
            Some((5, 7, text.len()))
        );
    }

    #[test]
    fn skips_untimed_separators_between_words() {
        let text = "foo bar";
        assert_eq!(find_word_utf16_range(text, "foo", 0), Some((0, 3, 3)));
        assert_eq!(find_word_utf16_range(text, "bar", 3), Some((4, 7, 7)));
    }

    #[test]
    fn wrapped_group_reserves_space_after_translation() {
        let lines = vec![LyricLine {
            start_ms: 0,
            end_ms: 2_000,
            text: "This main lyric wraps onto several visual lines".to_owned(),
            translation: Some("这是一条同样会换行的长翻译歌词".to_owned()),
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: None,
            words: Vec::new(),
        }];
        let renderer = LyricsRenderer::new(
            &lines,
            Viewport {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 240.0,
            },
        )
        .unwrap();
        assert!(renderer.base_paragraphs[0].line_number() > 1);
        let translation_height = renderer.translation_paragraphs[0]
            .as_ref()
            .unwrap()
            .height();
        let required = renderer.base_paragraphs[0].height()
            + translation_height
            + renderer.main_font_size * (TRANSLATION_GAP_EM + GROUP_GAP_EM);
        assert!((renderer.group_heights()[0] + renderer.group_gap()) as f32 >= required - 0.01);
    }
}
