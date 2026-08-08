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
            let split_x = frame.width / 2.0;
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
    fn landscape_reserves_left_half_for_cover() {
        let geometry = FrameGeometry::for_frame(1920, 1080);
        assert_eq!(geometry.cover.unwrap().width, 960.0);
        assert_eq!(geometry.lyrics.x, 960.0);
        assert_eq!(geometry.lyrics.width, 960.0);
    }

    #[test]
    fn portrait_keeps_full_width_for_lyrics() {
        let geometry = FrameGeometry::for_frame(1080, 1920);
        assert!(geometry.cover.is_none());
        assert_eq!(geometry.lyrics, geometry.frame);
    }
}
