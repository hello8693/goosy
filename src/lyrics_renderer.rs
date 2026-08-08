use anyhow::{Context, Result};
use skia_safe::{Canvas, Color, FontMgr, Paint, Point, Rect};
use skia_safe::canvas::SaveLayerRec;
use skia_safe::image_filters;
use skia_safe::textlayout::{FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle};

use crate::easing::bez_in;
use crate::layout::Layout;
use crate::lrc::LyricLine;

pub struct LyricsRenderer {
    base_paragraphs: Vec<Paragraph>,
    solid_paragraphs: Vec<Paragraph>,
    margin_x: f32,
    text_width: f32,
    line_step: f64,
}
impl LyricsRenderer {
    pub fn new(lines: &[LyricLine], width: u32, height: u32) -> Result<Self> {
        let margin_x = (width as f32 * 0.08).min(160.0);
        let text_width = (width as f32 - 2.0 * margin_x).max(1.0);
        let font_size = if width <= 768 { (width as f32 * 0.08).max(12.0) } else { (height as f32 * 0.05).max(width as f32 * 0.025).max(12.0) };
        let base_paragraphs = build_paragraphs(lines, Color::from_argb(102, 255, 255, 255), text_width, font_size)?;
        let solid_paragraphs = build_paragraphs(lines, Color::WHITE, text_width, font_size)?;
        Ok(Self { base_paragraphs, solid_paragraphs, margin_x, text_width, line_step: (font_size * 1.3) as f64 })
    }

    pub fn draw(&self, canvas: &Canvas, lines: &[LyricLine], layout: &Layout, t_ms: u64, height: u32) -> Result<()> {
        let active_idx = layout.active_idx();
        for (index, paragraph) in self.base_paragraphs.iter().enumerate() {
            if lines[index].text.is_empty() { continue; }
            let top_y = layout.pos_y[index].current_position() as f32;
            let scale = layout.scale[index].current_position() as f32;
            let opacity = edge_opacity(top_y, height as f32, index == active_idx);
            let transform = canvas.save();
            canvas.translate((0.0, top_y));
            canvas.scale((scale, scale));
            canvas.translate((0.0, -top_y));
            let blur_sigma = ((index as isize - active_idx as isize).abs() as f32).min(5.0);
            if blur_sigma > 0.01 {
                let blur_filter = image_filters::blur((blur_sigma, blur_sigma), None, None, None)
                    .context("create lyric blur filter")?;
                let mut layer_paint = Paint::default();
                layer_paint.set_alpha_f(opacity).set_image_filter(blur_filter);
                let layer_rec = SaveLayerRec::default().paint(&layer_paint);
                let layer = canvas.save_layer(&layer_rec);
                draw_line(canvas, paragraph, &self.solid_paragraphs[index], &lines[index], index == active_idx, t_ms, self.margin_x, self.text_width, top_y);
                canvas.restore_to_count(layer);
            } else {
                let layer = canvas.save_layer_alpha_f(None, opacity);
                draw_line(canvas, paragraph, &self.solid_paragraphs[index], &lines[index], true, t_ms, self.margin_x, self.text_width, top_y);
                canvas.restore_to_count(layer);
            }
            canvas.restore_to_count(transform);
        }
        Ok(())
    }
    pub fn line_step(&self) -> f64 { self.line_step }

}

fn draw_line(canvas: &Canvas, paragraph: &Paragraph, active_paragraph: &Paragraph, line: &LyricLine, active: bool, t_ms: u64, margin_x: f32, text_width: f32, top_y: f32) {
    let position = Point::new(margin_x, top_y);
    if !active {
        paragraph.paint(canvas, position);
        return;
    }
    paragraph.paint(canvas, position);
    let duration = line.end_ms.saturating_sub(line.start_ms).max(1) as f64;
    let progress = ((t_ms.saturating_sub(line.start_ms) as f64) / duration).clamp(0.0, 1.0);
    let fill_width = text_width * bez_in(progress) as f32;
    canvas.save();
    canvas.clip_rect(Rect::from_xywh(margin_x, top_y, fill_width, 100.0), None, true);
    active_paragraph.paint(canvas, position);
    canvas.restore();
}

fn build_paragraphs(lines: &[LyricLine], color: Color, text_width: f32, font_size: f32) -> Result<Vec<Paragraph>> {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    lines.iter().map(|line| {
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_align(TextAlign::Left);
        let mut text_style = TextStyle::new();
        text_style.set_font_families(&["PingFang SC", "sans-serif"]);
        text_style.set_font_size(font_size);
        text_style.set_color(color);
        paragraph_style.set_text_style(&text_style);
        let mut builder = ParagraphBuilder::new(&paragraph_style, collection.clone());
        builder.push_style(&text_style).add_text(&line.text);
        let mut paragraph = builder.build();
        paragraph.layout(text_width);
        Ok(paragraph)
    }).collect()
}

fn edge_opacity(top_y: f32, height: f32, active: bool) -> f32 {
    if active { return 1.0; }
    let fade_top = ((top_y - 0.05 * height) / (0.10 * height)).clamp(0.0, 1.0);
    let fade_bottom = ((height - 0.05 * height - top_y) / (0.10 * height)).clamp(0.0, 1.0);
    fade_top.min(fade_bottom)
}
