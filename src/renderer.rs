use anyhow::{Result, bail};
use skia_safe::{AlphaType, Color, ColorSpace, ColorType, ISize, ImageInfo};

use crate::background::BackgroundRenderer;
use crate::cover_renderer::CoverRenderer;
use crate::geometry::FrameGeometry;
use crate::layout::Layout;
use crate::lrc::LyricLine;
use crate::lyrics_renderer::LyricsRenderer;

pub struct Renderer {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub lines: Vec<LyricLine>,
    geometry: FrameGeometry,
    layout: Layout,
    background: BackgroundRenderer,
    cover: Option<CoverRenderer>,
    lyrics: LyricsRenderer,
    surface: crate::surface::SurfaceRenderer,
    info: ImageInfo,
    row_bytes: usize,
}

impl Renderer {
    pub fn new(width: u32, height: u32, fps: u32, lines: Vec<LyricLine>) -> Result<Self> {
        Self::with_background(width, height, fps, lines, BackgroundRenderer::dynamic())
    }

    pub fn with_background(
        width: u32,
        height: u32,
        fps: u32,
        lines: Vec<LyricLine>,
        background: BackgroundRenderer,
    ) -> Result<Self> {
        Self::with_scene(width, height, fps, lines, background, None)
    }
    pub fn with_scene_options(
        width: u32,
        height: u32,
        fps: u32,
        mut lines: Vec<LyricLine>,
        background: BackgroundRenderer,
        cover: Option<CoverRenderer>,
        render_translation: bool,
        render_background_vocal: bool,
        excluded_lines: &[usize],
    ) -> Result<Self> {
        if width == 0 || height == 0 || fps == 0 {
            bail!("width, height, and fps must be positive");
        }
        if !excluded_lines.is_empty() {
            let excluded = excluded_lines
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            lines = lines
                .into_iter()
                .enumerate()
                .filter_map(|(index, line)| (!excluded.contains(&index)).then_some(line))
                .collect();
        }
        let geometry = FrameGeometry::for_frame(width, height);
        let lyrics = LyricsRenderer::new_with_options(
            &lines,
            geometry.lyrics,
            render_translation,
            render_background_vocal,
        )?;
        let layout = Layout::new(
            &lines,
            geometry.lyrics.height,
            lyrics.group_heights(),
            lyrics.group_gap(),
            lyrics.interlude_slot_height(),
        );
        let surface = crate::surface::SurfaceRenderer::try_new()?;
        Ok(Self {
            width,
            height,
            fps,
            lines,
            geometry,
            layout,
            background,
            cover,
            lyrics,
            surface,
            info: ImageInfo::new(
                ISize::new(width as i32, height as i32),
                ColorType::RGBA8888,
                AlphaType::Premul,
                ColorSpace::new_srgb(),
            ),
            row_bytes: width as usize * 4,
        })
    }

    pub fn with_scene(
        width: u32,
        height: u32,
        fps: u32,
        lines: Vec<LyricLine>,
        background: BackgroundRenderer,
        cover: Option<CoverRenderer>,
    ) -> Result<Self> {
        Self::with_scene_options(
            width,
            height,
            fps,
            lines,
            background,
            cover,
            true,
            true,
            &[],
        )
    }

    pub fn backend_details(&self) -> String {
        self.surface.backend_details()
    }
    pub fn backend_name(&self) -> &'static str {
        self.surface.backend_name()
    }

    pub fn render_frame(&mut self, t_ms: u64) -> Result<Vec<u8>> {
        let mut pixels = Vec::new();
        self.render_frame_into(t_ms, &mut pixels)?;
        Ok(pixels)
    }

    pub fn render_frame_into(&mut self, t_ms: u64, pixels: &mut Vec<u8>) -> Result<()> {
        self.layout.update(&self.lines, t_ms, self.fps);
        let geometry = self.geometry;
        let lines = &self.lines;
        let layout = &self.layout;
        let background = &mut self.background;
        let cover = &mut self.cover;
        let lyrics = &self.lyrics;
        self.surface
            .render_into(&self.info, self.row_bytes, pixels, |canvas| {
                canvas.clear(Color::BLACK);
                background.draw(canvas, &geometry, t_ms)?;
                if let Some(cover) = cover {
                    cover.draw(canvas, &geometry)?;
                }
                lyrics.draw(canvas, lines, layout, t_ms, geometry.lyrics.height as u32)
            })
    }
}

#[cfg(target_os = "windows")]
pub fn probe_renderer_initialization(width: u32, height: u32, fps: u32) -> Result<()> {
    use crate::lrc::LyricLine;

    skia_safe::icu::init();
    let _font_manager = skia_safe::FontMgr::new();
    let lines = vec![LyricLine {
        start_ms: 0,
        end_ms: 1_000,
        text: "Goosy 渲染探针".to_owned(),
        translation: Some("Renderer probe".to_owned()),
        agent_id: None,
        is_duet: false,
        is_background: false,
        background_vocal: None,
        words: Vec::new(),
    }];
    let mut renderer = Renderer::new(width, height, fps, lines)?;
    let mut pixels = Vec::new();
    renderer.render_frame_into(0, &mut pixels)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_lyrics_render_identical_frames() -> Result<()> {
        let lines = vec![
            LyricLine {
                start_ms: 0,
                end_ms: 2_000,
                text: "Stable line".to_owned(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: Vec::new(),
            },
            LyricLine {
                start_ms: 2_000,
                end_ms: 4_000,
                text: "Next line".to_owned(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: Vec::new(),
            },
        ];
        let mut renderer = Renderer::new(320, 180, 30, lines)?;
        for _ in 0..180 {
            renderer.render_frame(1_000)?;
        }
        let first = renderer.render_frame(1_000)?;
        let second = renderer.render_frame(1_000)?;
        assert_eq!(first, second);
        Ok(())
    }
    #[test]
    fn excluded_lines_are_removed_without_retiming_remaining_lines() -> Result<()> {
        let lines = vec![
            LyricLine {
                start_ms: 0,
                end_ms: 1_000,
                text: "Excluded".to_owned(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: Vec::new(),
            },
            LyricLine {
                start_ms: 1_000,
                end_ms: 2_000,
                text: "Visible".to_owned(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: Vec::new(),
            },
        ];
        let renderer = Renderer::with_scene_options(
            320,
            180,
            30,
            lines,
            BackgroundRenderer::dynamic(),
            None,
            true,
            true,
            &[0],
        )?;
        assert_eq!(renderer.lines.len(), 1);
        assert_eq!(renderer.lines[0].text, "Visible");
        assert_eq!(renderer.lines[0].start_ms, 1_000);
        Ok(())
    }
}
