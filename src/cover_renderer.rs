use anyhow::{Context, Result};
use skia_safe::canvas::SrcRectConstraint;
use skia_safe::image::images;
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle,
};
use skia_safe::{
    BlurStyle, Canvas, Color, Data, FontMgr, FontStyle, Image, ImageInfo, MaskFilter, Paint,
    PaintStyle, Pixmap, Point, RRect, Rect, SamplingOptions,
};
use std::path::Path;

use crate::geometry::FrameGeometry;

pub struct CoverRenderer {
    image: Image,
    source: Rect,
    title: Option<String>,
    title_paragraph: Option<Paragraph>,
    title_layout_width: f32,
    shadow_filter: Option<MaskFilter>,
    shadow_sigma: f32,
    shadow_paint: Paint,
    image_paint: Paint,
    border_paint: Paint,
    scaled_image: Option<Image>,
    scaled_side: u32,
}

impl CoverRenderer {
    pub fn from_path(path: &Path, title: Option<String>) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read cover image {}", path.display()))?;
        Self::from_bytes(&bytes, title).context("decode cover image")
    }

    pub fn from_bytes(bytes: &[u8], title: Option<String>) -> Result<Self> {
        let image = Image::from_encoded(Data::new_copy(bytes)).context("decode cover image")?;
        let image_width = image.width() as f32;
        let image_height = image.height() as f32;
        let crop_side = image_width.min(image_height);
        let source = Rect::from_xywh(
            (image_width - crop_side) * 0.5,
            (image_height - crop_side) * 0.5,
            crop_side,
            crop_side,
        );
        let mut shadow_paint = Paint::default();
        shadow_paint
            .set_anti_alias(true)
            .set_color(Color::from_argb(120, 0, 0, 0));
        let mut image_paint = Paint::default();
        image_paint.set_anti_alias(true);
        let mut border_paint = Paint::default();
        border_paint
            .set_anti_alias(true)
            .set_style(PaintStyle::Stroke)
            .set_color(Color::from_argb(38, 255, 255, 255));
        Ok(Self {
            image,
            source,
            title: title.filter(|title| !title.trim().is_empty()),
            title_paragraph: None,
            title_layout_width: 0.0,
            shadow_filter: None,
            shadow_sigma: 0.0,
            shadow_paint,
            image_paint,
            border_paint,
            scaled_image: None,
            scaled_side: 0,
        })
    }
    fn ensure_scaled_image(&mut self, side: f32) -> Result<()> {
        if self.source.width() + 0.01 < self.image.width() as f32
            || self.source.height() + 0.01 < self.image.height() as f32
        {
            return Ok(());
        }
        let side_px = side.ceil().max(1.0) as u32;
        if self.scaled_image.is_some() && self.scaled_side == side_px {
            return Ok(());
        }
        let info = ImageInfo::new_n32_premul((side_px as i32, side_px as i32), None);
        let row_bytes = info.min_row_bytes();
        let mut pixels = vec![0_u8; row_bytes * side_px as usize];
        let pixmap =
            Pixmap::new(&info, &mut pixels, row_bytes).context("create scaled cover pixmap")?;
        if !self
            .image
            .scale_pixels(&pixmap, SamplingOptions::default(), None)
        {
            anyhow::bail!("scale cover image to {side_px}x{side_px}");
        }
        drop(pixmap);
        self.scaled_image = Some(
            images::raster_from_data(&info, Data::new_copy(&pixels), row_bytes)
                .context("create scaled cover image")?,
        );
        self.scaled_side = side_px;
        Ok(())
    }

    pub fn draw(&mut self, canvas: &Canvas, geometry: &FrameGeometry) -> Result<()> {
        let Some(viewport) = geometry.cover else {
            return Ok(());
        };
        let side = (viewport.width * 0.78).min(viewport.height * 0.62);
        if side <= 0.0 {
            return Ok(());
        }
        let font_size = (viewport.width * 0.048)
            .min(viewport.height * 0.034)
            .max(16.0);
        self.ensure_title(side, font_size)?;
        self.ensure_shadow(side)?;
        self.ensure_scaled_image(side)?;

        let title_height = self
            .title_paragraph
            .as_ref()
            .map(|paragraph| paragraph.height())
            .unwrap_or(0.0);
        let title_gap = if title_height > 0.0 {
            font_size * 0.65
        } else {
            0.0
        };
        let total_height = side + title_gap + title_height;
        let left = viewport.x + (viewport.width - side) * 0.5;
        let top = viewport.y + (viewport.height - total_height) * 0.5;
        let destination = Rect::from_xywh(left, top, side, side);
        let radius = side * 0.055;

        if self.shadow_filter.is_some() {
            let shadow_rect = Rect::from_xywh(left, top + side * 0.025, side, side);
            canvas.draw_rrect(
                RRect::new_rect_xy(shadow_rect, radius, radius),
                &self.shadow_paint,
            );
        }

        let rounded = RRect::new_rect_xy(destination, radius, radius);
        let saved = canvas.save();
        canvas.clip_rrect(rounded, None, true);
        let image = self.scaled_image.as_ref().unwrap_or(&self.image);
        let source = self
            .scaled_image
            .as_ref()
            .map(|_| Rect::from_xywh(0.0, 0.0, side.ceil(), side.ceil()))
            .unwrap_or(self.source);
        canvas.draw_image_rect(
            image,
            Some((&source, SrcRectConstraint::Strict)),
            &destination,
            &self.image_paint,
        );
        canvas.restore_to_count(saved);

        self.border_paint.set_stroke_width((side * 0.002).max(1.0));
        canvas.draw_rrect(rounded, &self.border_paint);

        if let Some(paragraph) = &self.title_paragraph {
            paragraph.paint(canvas, Point::new(left, top + side + title_gap));
        }
        Ok(())
    }

    fn ensure_title(&mut self, width: f32, font_size: f32) -> Result<()> {
        if self.title.is_none()
            || (self.title_layout_width - width).abs() < 0.01 && self.title_paragraph.is_some()
        {
            return Ok(());
        }
        let mut collection = FontCollection::new();
        collection.set_default_font_manager(FontMgr::new(), None);
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style
            .set_text_align(TextAlign::Center)
            .set_max_lines(2)
            .set_ellipsis("…");
        let mut text_style = TextStyle::new();
        text_style
            .set_font_families(&[
                "SF Pro Display",
                "PingFang SC",
                "Microsoft YaHei",
                "Noto Sans CJK SC",
                "sans-serif",
            ])
            .set_font_size(font_size)
            .set_font_style(FontStyle::bold())
            .set_color(Color::WHITE)
            .set_height(1.18)
            .set_height_override(true);
        paragraph_style.set_text_style(&text_style);
        let mut builder = ParagraphBuilder::new(&paragraph_style, collection);
        builder
            .push_style(&text_style)
            .add_text(self.title.as_deref().unwrap_or_default());
        let mut paragraph = builder.build();
        paragraph.layout(width);
        self.title_paragraph = Some(paragraph);
        self.title_layout_width = width;
        Ok(())
    }

    fn ensure_shadow(&mut self, side: f32) -> Result<()> {
        let sigma = (side * 0.028).max(4.0);
        if self.shadow_filter.is_some() && (self.shadow_sigma - sigma).abs() < 0.01 {
            return Ok(());
        }
        let filter = MaskFilter::blur(BlurStyle::Normal, sigma, true)
            .context("create cover shadow filter")?;
        self.shadow_paint.set_mask_filter(Some(filter.clone()));
        self.shadow_filter = Some(filter);
        self.shadow_sigma = sigma;
        Ok(())
    }
}
