use anyhow::{Context, Result, bail};
use skia_safe::{AlphaType, ColorSpace, ColorType, ImageInfo, IPoint, ISize, Surface};

use crate::background;
use crate::layout::Layout;
use crate::lrc::LyricLine;
use crate::lyrics_renderer::LyricsRenderer;

pub struct Renderer {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub lines: Vec<LyricLine>,
    layout: Layout,
    lyrics: LyricsRenderer,
}

impl Renderer {
    pub fn new(width: u32, height: u32, fps: u32, lines: Vec<LyricLine>) -> Result<Self> {
        if width == 0 || height == 0 || fps == 0 { bail!("width, height, and fps must be positive"); }
        let lyrics = LyricsRenderer::new(&lines, width, height)?;
        let layout = Layout::new(&lines, height as f32, lyrics.line_step());
        Ok(Self { width, height, fps, lines, layout, lyrics })
    }

    pub fn render_frame(&mut self, t_ms: u64) -> Result<Vec<u8>> {
        self.layout.update(&self.lines, t_ms, self.fps);
        let info = ImageInfo::new(
            ISize::new(self.width as i32, self.height as i32),
            ColorType::RGBA8888,
            AlphaType::Premul,
            ColorSpace::new_srgb(),
        );
        let mut surface = Surface::new_raster(&info, self.width as usize * 4, None)
            .context("create raster surface")?;
        let canvas = surface.canvas();
        background::draw(canvas, self.width, self.height)?;
        self.lyrics.draw(canvas, &self.lines, &self.layout, t_ms, self.height)?;
        let mut pixels = vec![0_u8; self.width as usize * self.height as usize * 4];
        if !surface.read_pixels(&info, &mut pixels, self.width as usize * 4, IPoint::new(0, 0)) {
            bail!("read RGBA pixels from surface");
        }
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_lyrics_render_identical_frames() -> Result<()> {
        let lines = vec![
            LyricLine { start_ms: 0, end_ms: 2_000, text: "Stable line".to_owned(), words: Vec::new() },
            LyricLine { start_ms: 2_000, end_ms: 4_000, text: "Next line".to_owned(), words: Vec::new() },
        ];
        let mut renderer = Renderer::new(320, 180, 30, lines)?;
        for _ in 0..180 { renderer.render_frame(1_000)?; }
        let first = renderer.render_frame(1_000)?;
        let second = renderer.render_frame(1_000)?;
        assert_eq!(first, second);
        Ok(())
    }
}
