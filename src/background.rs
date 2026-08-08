use anyhow::{Context, Result};
use skia_safe::{Canvas, Color, Paint, Point, Rect};
use skia_safe::gradient::{Colors, Gradient};
use skia_safe::gradient::shaders::linear_gradient;

pub fn draw(canvas: &Canvas, width: u32, height: u32) -> Result<()> {
    let colors = [Color::from_argb(255, 16, 16, 20).into(), Color::from_argb(255, 28, 28, 38).into()];
    let positions = [0.0, 1.0];
    let gradient = Gradient::new(
        Colors::new(&colors, Some(&positions), skia_safe::TileMode::Clamp, None),
        skia_safe::gradient::Interpolation::default(),
    );
    let mut paint = Paint::default();
    paint.set_shader(linear_gradient(
        (Point::new(0.0, 0.0), Point::new(0.0, height as f32)),
        &gradient,
        None,
    ).context("create background shader")?);
    canvas.draw_rect(Rect::from_wh(width as f32, height as f32), &paint);
    Ok(())
}
