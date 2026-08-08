use crate::lrc::LyricLine;
use crate::spring::Spring;

const POS_STIFFNESS: f64 = 40.0;
const POS_DAMPING: f64 = 10.0;

#[derive(Debug)]
pub struct Layout {
    pub pos_y: Vec<Spring>,
    pub scale: Vec<Spring>,
    active_idx: usize,
    height: f64,
    initialized: bool,
    step_y: f64,
}

impl Layout {
    pub fn new(lines: &[LyricLine], height: f32, step_y: f64) -> Self {
        let active_idx = 0;
        let center = height as f64 / 2.0;
        let pos_y = lines
            .iter()
            .enumerate()
            .map(|(index, _)| {
                Spring::with_parameters(
                    center + (index as f64 - active_idx as f64) * step_y,
                    POS_STIFFNESS,
                    POS_DAMPING,
                    1.0,
                    true,
                )
            })
            .collect();
        let scale = lines
            .iter()
            .map(|_| Spring::with_parameters(1.0, 100.0, 10.0, 1.0, true))
            .collect();
        Self {
            pos_y,
            scale,
            active_idx,
            height: height as f64,
            initialized: false,
            step_y,
        }
    }

    pub fn update(&mut self, lines: &[LyricLine], t_ms: u64, fps: u32) {
        if lines.is_empty() || fps == 0 {
            return;
        }
        let next_active = active_index(lines, t_ms);
        if !self.initialized {
            self.active_idx = next_active;
            self.set_targets(lines);
            self.initialized = true;
        } else if next_active != self.active_idx {
            self.active_idx = next_active;
            for index in 0..lines.len() {
                self.pos_y[index].set_parameters(POS_STIFFNESS, POS_DAMPING, 1.0, true);
                let target_y = self.target_y(index);
                self.pos_y[index].set_target_position(target_y);
                self.scale[index].set_target_position(if index == self.active_idx {
                    1.0
                } else {
                    0.97
                });
            }
        }
        let dt = 1.0 / fps as f64;
        for spring in &mut self.pos_y {
            spring.update(dt);
        }
        for spring in &mut self.scale {
            spring.update(dt);
        }
    }

    pub fn active_idx(&self) -> usize {
        self.active_idx
    }

    fn target_y(&self, index: usize) -> f64 {
        self.height / 2.0 + (index as f64 - self.active_idx as f64) * self.step_y
    }

    fn set_targets(&mut self, lines: &[LyricLine]) {
        for index in 0..lines.len() {
            let target_y = self.target_y(index);
            self.pos_y[index].set_position(target_y);
            self.scale[index].set_position(if index == self.active_idx { 1.0 } else { 0.97 });
        }
    }
}

fn active_index(lines: &[LyricLine], t_ms: u64) -> usize {
    lines
        .iter()
        .position(|line| line.start_ms <= t_ms && t_ms < line.end_ms)
        .unwrap_or_else(|| {
            lines
                .iter()
                .rposition(|line| line.start_ms <= t_ms)
                .unwrap_or(0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<LyricLine> {
        (0..3)
            .map(|index| LyricLine {
                start_ms: index * 2_000,
                end_ms: (index + 1) * 2_000,
                text: format!("Line {index}"),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                words: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn transition_moves_over_multiple_frames() {
        let lines = lines();
        let mut layout = Layout::new(&lines, 1_080.0, 70.0);
        layout.update(&lines, 0, 30);
        let before = layout.pos_y[0].current_position();
        layout.update(&lines, 2_000, 30);
        let at_switch = layout.pos_y[0].current_position();
        layout.update(&lines, 2_000 + 1_000 / 30, 30);
        let next = layout.pos_y[0].current_position();
        assert!(at_switch < before);
        assert!(next < at_switch);
        assert!((before - at_switch).abs() < 20.0);
    }

    #[test]
    fn scale_settles_monotonically_after_line_change() {
        let lines = lines();
        let mut layout = Layout::new(&lines, 1_080.0, 70.0);
        layout.update(&lines, 0, 30);
        layout.update(&lines, 2_000, 30);
        let mut previous = layout.scale[0].current_position();
        for frame in 1..60 {
            layout.update(&lines, 2_000 + frame * 1_000 / 30, 30);
            let current = layout.scale[0].current_position();
            assert!(current <= previous + 1e-9);
            assert!(current >= 0.97 - 1e-9);
            previous = current;
        }
        assert!((previous - 0.97).abs() < 0.01);
    }
}
