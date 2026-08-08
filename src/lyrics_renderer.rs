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
use crate::lrc::LyricLine;

pub struct LyricsRenderer {
    base_paragraphs: Vec<Paragraph>,
    solid_paragraphs: Vec<Paragraph>,
    word_boxes: Vec<Vec<Vec<TextBox>>>,
    translation_paragraphs: Vec<Option<Paragraph>>,
    margin_x: f32,
    text_width: f32,
    main_font_size: f32,
    line_step: f64,
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
        let mut lyric_blurs = vec![None];
        for sigma in 1..=5 {
            lyric_blurs.push(Some(
                image_filters::blur((sigma as f32, sigma as f32), None, None, None)
                    .context("create lyric blur filter")?,
            ));
        }
        let line_step =
            lines
                .iter()
                .enumerate()
                .fold(main_font_size * 1.3, |step, (index, _line)| {
                    let translation_height = translation_paragraphs[index]
                        .as_ref()
                        .map(|paragraph| paragraph.height())
                        .unwrap_or(0.0);
                    let translation_gap = if translation_height > 0.0 {
                        main_font_size * 0.15
                    } else {
                        0.0
                    };
                    step.max(base_paragraphs[index].height() + translation_height + translation_gap)
                });
        Ok(Self {
            base_paragraphs,
            solid_paragraphs,
            word_boxes,
            translation_paragraphs,
            margin_x,
            text_width,
            main_font_size,
            line_step: line_step as f64,
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
        let active_idx = layout.active_idx();
        for (index, paragraph) in self.base_paragraphs.iter().enumerate() {
            if lines[index].text.is_empty() && self.translation_paragraphs[index].is_none() {
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
                        t_ms,
                        self.margin_x,
                        self.text_width,
                        top_y,
                        self.main_font_size,
                    );
                    canvas.restore_to_count(layer);
                }
            }
            canvas.restore_to_count(transform);
        }
        Ok(())
    }
    pub fn line_step(&self) -> f64 {
        self.line_step
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
            top_y + paragraph.height() + main_font_size * 0.15,
        );
        return;
    }
    paragraph.paint(canvas, position);
    if word_boxes.len() == line.words.len() && !line.words.is_empty() {
        let mut mask = PathBuilder::new();
        let mut has_mask = false;
        for (word, boxes) in line.words.iter().zip(word_boxes) {
            let progress = bez_in(word_progress(word.start_ms, word.end_ms, t_ms)) as f32;
            let mut remaining = boxes
                .iter()
                .map(|text_box| text_box.rect.width())
                .sum::<f32>()
                * progress;
            for text_box in boxes {
                if remaining <= 0.0 {
                    break;
                }
                let rect = text_box.rect;
                let fill_width = remaining.min(rect.width());
                if fill_width > 0.0 {
                    mask.add_rect(
                        Rect::from_xywh(
                            rect.left() + margin_x,
                            top_y + rect.top(),
                            fill_width,
                            rect.height(),
                        ),
                        None,
                        None,
                    );
                    has_mask = true;
                }
                remaining -= rect.width();
            }
        }
        if has_mask {
            let path = mask.detach();
            canvas.save();
            canvas.clip_path(&path, None, true);
            active_paragraph.paint(canvas, position);
            canvas.restore();
        }
    } else {
        let duration = line.end_ms.saturating_sub(line.start_ms).max(1) as f64;
        let progress = ((t_ms.saturating_sub(line.start_ms) as f64) / duration).clamp(0.0, 1.0);
        canvas.save();
        canvas.clip_rect(
            Rect::from_xywh(
                margin_x,
                top_y,
                text_width * bez_in(progress) as f32,
                paragraph.height(),
            ),
            None,
            true,
        );
        active_paragraph.paint(canvas, position);
        canvas.restore();
    }
    draw_translation(
        canvas,
        translation,
        margin_x,
        top_y + paragraph.height() + main_font_size * 0.15,
    );
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
            text_style.set_font_families(&["PingFang SC", "sans-serif"]);
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
    text_style.set_font_families(&["PingFang SC", "sans-serif"]);
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
    use super::find_word_utf16_range;

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
}
