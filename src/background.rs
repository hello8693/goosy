use crate::geometry::FrameGeometry;
use anyhow::{Context, Result};
use skia_safe::canvas::SrcRectConstraint;
use skia_safe::gradient::shaders::linear_gradient;
use skia_safe::gradient::{Colors, Gradient};
use skia_safe::image::CachingHint;
use skia_safe::image_filters;
use skia_safe::{
    AlphaType, Canvas, Color, ColorSpace, ColorType, Data, IPoint, ISize, Image, ImageFilter,
    ImageInfo, Paint, Point, Rect, TileMode,
};

const BACKGROUND_ZOOM: f32 = 1.45;
const BACKGROUND_BLUR_SIGMA: f32 = 96.0;
use std::path::Path;

pub trait BackgroundLayer {
    fn draw(&mut self, canvas: &Canvas, geometry: &FrameGeometry, t_ms: u64) -> Result<()>;
}

pub struct BackgroundRenderer {
    layers: Vec<Box<dyn BackgroundLayer>>,
}

impl BackgroundRenderer {
    pub fn dynamic() -> Self {
        let mut renderer = Self { layers: Vec::new() };
        renderer.add_layer(GradientLayer::new());
        renderer.add_layer(FallbackMaskLayer::new());
        renderer
    }

    pub fn gradient() -> Self {
        Self::dynamic()
    }

    pub fn from_image_path(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read background image {}", path.display()))?;
        Self::from_image_bytes(&bytes).context("decode background image")
    }

    pub fn from_image_bytes(bytes: &[u8]) -> Result<Self> {
        let image =
            Image::from_encoded(Data::new_copy(bytes)).context("decode background image")?;
        Self::from_image(image)
    }

    pub(crate) fn from_image(image: Image) -> Result<Self> {
        let image_width = image.width() as f32;
        let image_height = image.height() as f32;
        let sampled_color = sample_image_color(&image).unwrap_or((32, 32, 40));
        let blur_filter = image_filters::blur(
            (BACKGROUND_BLUR_SIGMA, BACKGROUND_BLUR_SIGMA),
            TileMode::Clamp,
            None,
            None,
        )
        .context("create background blur filter")?;
        let mut renderer = Self { layers: Vec::new() };
        renderer.add_layer(ImageLayer {
            image,
            image_width,
            image_height,
            blur_paints: ImageLayer::make_blur_paints(&blur_filter),
            tint_paint: ImageLayer::make_tint_paint(sampled_color),
        });
        renderer.add_layer(FallbackMaskLayer::new());
        Ok(renderer)
    }

    pub fn add_layer<L>(&mut self, layer: L)
    where
        L: BackgroundLayer + 'static,
    {
        self.layers.push(Box::new(layer));
    }

    pub fn draw(&mut self, canvas: &Canvas, geometry: &FrameGeometry, t_ms: u64) -> Result<()> {
        for layer in &mut self.layers {
            layer.draw(canvas, geometry, t_ms)?;
        }
        Ok(())
    }
}

struct GradientLayer {
    last_lift: Option<i32>,
    last_frame: Option<(f32, f32, f32, f32)>,
    paint: Paint,
}

impl GradientLayer {
    fn new() -> Self {
        Self {
            last_lift: None,
            last_frame: None,
            paint: Paint::default(),
        }
    }
}

impl BackgroundLayer for GradientLayer {
    fn draw(&mut self, canvas: &Canvas, geometry: &FrameGeometry, t_ms: u64) -> Result<()> {
        let phase = (t_ms as f32 / 12_000.0).sin();
        let lift = (phase * 5.0) as i32;
        let frame = geometry.frame;
        let frame_key = (frame.x, frame.y, frame.width, frame.height);
        if self.last_lift != Some(lift) || self.last_frame != Some(frame_key) {
            let colors = [
                Color::from_argb(255, 16, 16, 20).into(),
                Color::from_argb(
                    255,
                    (28 + lift).clamp(0, 255) as u8,
                    28,
                    (38 + lift).clamp(0, 255) as u8,
                )
                .into(),
            ];
            let positions = [0.0, 1.0];
            let gradient = Gradient::new(
                Colors::new(&colors, Some(&positions), TileMode::Clamp, None),
                skia_safe::gradient::Interpolation::default(),
            );
            self.paint.set_shader(Some(
                linear_gradient(
                    (Point::new(0.0, frame.y), Point::new(0.0, frame.bottom())),
                    &gradient,
                    None,
                )
                .context("create background shader")?,
            ));
            self.last_lift = Some(lift);
            self.last_frame = Some(frame_key);
        }
        canvas.draw_rect(
            Rect::from_xywh(frame.x, frame.y, frame.width, frame.height),
            &self.paint,
        );
        Ok(())
    }
}
fn sample_image_color(image: &Image) -> Option<(u8, u8, u8)> {
    let info = ImageInfo::new(
        ISize::new(3, 3),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        ColorSpace::new_srgb(),
    );
    let mut pixels = [0u8; 36];
    if !image.read_pixels(
        &info,
        &mut pixels,
        12,
        IPoint::new(0, 0),
        CachingHint::Disallow,
    ) {
        return None;
    }
    let mut sums = [0u32; 3];
    for pixel in pixels.chunks_exact(4) {
        for channel in 0..3 {
            sums[channel] += pixel[channel] as u32;
        }
    }
    Some((
        (sums[0] / 9) as u8,
        (sums[1] / 9) as u8,
        (sums[2] / 9) as u8,
    ))
}

struct ImageLayer {
    image: Image,
    image_width: f32,
    image_height: f32,
    blur_paints: [Paint; 2],
    tint_paint: Paint,
}

impl ImageLayer {
    fn make_blur_paints(blur_filter: &ImageFilter) -> [Paint; 2] {
        [0.80_f32, 0.20_f32].map(|alpha| {
            let mut paint = Paint::default();
            paint
                .set_alpha_f(alpha)
                .set_image_filter(blur_filter.clone());
            paint
        })
    }

    fn make_tint_paint(sampled_color: (u8, u8, u8)) -> Paint {
        let mut paint = Paint::default();
        paint.set_color(Color::from_argb(
            36,
            sampled_color.0,
            sampled_color.1,
            sampled_color.2,
        ));
        paint
    }
}

impl BackgroundLayer for ImageLayer {
    fn draw(&mut self, canvas: &Canvas, geometry: &FrameGeometry, t_ms: u64) -> Result<()> {
        let iw = self.image_width;
        let ih = self.image_height;
        if iw <= 0.0 || ih <= 0.0 {
            return Ok(());
        }
        let dst = Rect::from_xywh(
            geometry.frame.x,
            geometry.frame.y,
            geometry.frame.width,
            geometry.frame.height,
        );
        let scale = (dst.width() / iw).max(dst.height() / ih) * BACKGROUND_ZOOM;
        let visible_w = dst.width() / scale;
        let visible_h = dst.height() / scale;
        let drift = (t_ms as f32 / 8_000.0).sin() * (iw - visible_w).max(0.0) * 0.25;
        let src = Rect::from_xywh(
            ((iw - visible_w) * 0.5 + drift).clamp(0.0, (iw - visible_w).max(0.0)),
            ((ih - visible_h) * 0.5).max(0.0),
            visible_w.min(iw),
            visible_h.min(ih),
        );
        let center = Point::new(dst.center_x(), dst.center_y());
        let phase = t_ms as f32 / 9_000.0;
        let passes = [
            (0.0, 1.0, 0.80),
            ((phase * 1.7).sin() * 4.0 - 2.0, 1.08, 0.20),
        ];
        for (index, (angle, pass_scale, _alpha)) in passes.into_iter().enumerate() {
            let saved = canvas.save();
            canvas.rotate(angle, Some(center));
            canvas.scale((pass_scale, pass_scale));
            canvas.draw_image_rect(
                &self.image,
                Some((&src, SrcRectConstraint::Strict)),
                &dst,
                &self.blur_paints[index],
            );
            canvas.restore_to_count(saved);
        }
        canvas.draw_rect(dst, &self.tint_paint);
        Ok(())
    }
}

struct FallbackMaskLayer {
    paint: Paint,
}

impl FallbackMaskLayer {
    fn new() -> Self {
        let mut paint = Paint::default();
        paint.set_color(Color::from_argb(72, 0, 0, 0));
        Self { paint }
    }
}

impl BackgroundLayer for FallbackMaskLayer {
    fn draw(&mut self, canvas: &Canvas, geometry: &FrameGeometry, _t_ms: u64) -> Result<()> {
        canvas.draw_rect(
            Rect::from_xywh(
                geometry.frame.x,
                geometry.frame.y,
                geometry.frame.width,
                geometry.frame.height,
            ),
            &self.paint,
        );
        Ok(())
    }
}
