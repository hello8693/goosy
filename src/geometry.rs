#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameGeometry {
    pub frame: Viewport,
    pub lyrics: Viewport,
    pub cover: Option<Viewport>,
}

impl FrameGeometry {
    pub fn for_frame(width: u32, height: u32) -> Self {
        let frame = Viewport {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        };
        if width > height {
            let split_x = frame.width * 0.381_966_011_25;
            Self {
                frame,
                cover: Some(Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: split_x,
                    height: frame.height,
                }),
                lyrics: Viewport {
                    x: split_x,
                    y: 0.0,
                    width: frame.width - split_x,
                    height: frame.height,
                },
            }
        } else {
            Self {
                frame,
                lyrics: frame,
                cover: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landscape_uses_golden_ratio_with_larger_lyric_viewport() {
        let geometry = FrameGeometry::for_frame(1920, 1080);
        let cover_width = 1920.0 * 0.381_966_011_25;
        assert!((geometry.cover.unwrap().width - cover_width).abs() < 0.01);
        assert!((geometry.lyrics.x - cover_width).abs() < 0.01);
        assert!((geometry.lyrics.width - 1920.0 * 0.618_033_988_75).abs() < 0.01);
    }

    #[test]
    fn portrait_keeps_full_width_for_lyrics() {
        let geometry = FrameGeometry::for_frame(1080, 1920);
        assert!(geometry.cover.is_none());
        assert_eq!(geometry.lyrics, geometry.frame);
    }
}
