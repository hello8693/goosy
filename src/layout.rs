use crate::lrc::{LyricLine, scan_end_ms};
use crate::spring::Spring;

const POS_STIFFNESS: f64 = 40.0;
const POS_DAMPING: f64 = 10.0;
const MIN_ANTICIPATION_MS: u64 = 180;
const MAX_ANTICIPATION_MS: u64 = 700;
const MIN_INTERLUDE_MS: u64 = 4_000;
const INTERLUDE_END_LEAD_MS: u64 = 250;
const INTERLUDE_FLOAT_DELAY_MS: u64 = 70;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interlude {
    pub next_idx: usize,
    pub start_ms: u64,
    pub end_ms: u64,
}

fn interlude_between(lines: &[LyricLine], next_idx: usize) -> Option<Interlude> {
    let next = lines.get(next_idx)?;
    let start_ms = next_idx
        .checked_sub(1)
        .and_then(|index| lines.get(index).map(|line| line.end_ms))
        .unwrap_or(0);
    let end_ms = next.start_ms.saturating_sub(INTERLUDE_END_LEAD_MS);
    (end_ms.saturating_sub(start_ms) >= MIN_INTERLUDE_MS).then_some(Interlude {
        next_idx,
        start_ms,
        end_ms,
    })
}

pub fn interlude_for(lines: &[LyricLine], t_ms: u64) -> Option<Interlude> {
    (0..lines.len()).find_map(|next_idx| {
        let interlude = interlude_between(lines, next_idx)?;
        (interlude.start_ms <= t_ms && t_ms < interlude.end_ms).then_some(interlude)
    })
}

#[derive(Debug)]
pub struct Layout {
    pub pos_y: Vec<Spring>,
    pub scale: Vec<Spring>,
    active_idx: usize,
    focus_idx: usize,
    height: f64,
    initialized: bool,
    // Total height occupied by each lyric group, including its text.
    group_heights: Vec<f64>,
    // Additional empty space between adjacent groups; excludes group_heights.
    group_gap: f64,
    interlude_slot_height: f64,
    interlude: Option<Interlude>,
}

impl Layout {
    pub fn new(
        lines: &[LyricLine],
        height: f32,
        group_heights: &[f64],
        group_gap: f64,
        interlude_slot_height: f64,
    ) -> Self {
        let active_idx = 0;
        let center = height as f64 / 2.0;
        let group_heights = lines
            .iter()
            .enumerate()
            .map(|(index, _)| group_heights.get(index).copied().unwrap_or(0.0).max(0.0))
            .collect::<Vec<_>>();
        let pos_y = lines
            .iter()
            .enumerate()
            .map(|(index, _)| {
                Spring::with_parameters(
                    center + (index as f64 - active_idx as f64) * group_gap,
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
            focus_idx: active_idx,
            height: height as f64,
            initialized: false,
            group_heights,
            group_gap: group_gap.max(0.0),
            interlude_slot_height: interlude_slot_height.max(0.0),
            interlude: None,
        }
    }

    pub fn update(&mut self, lines: &[LyricLine], t_ms: u64, fps: u32) {
        if lines.is_empty() || fps == 0 {
            return;
        }
        let next_active = active_index(lines, t_ms);
        let next_focus = anticipatory_index(lines, t_ms, next_active);
        let next_interlude = interlude_for(lines, t_ms);
        if !self.initialized {
            self.active_idx = next_active;
            self.focus_idx = next_focus;
            self.interlude = next_interlude;
            self.set_targets(lines);
            self.initialized = true;
        } else {
            let focus_changed = next_focus != self.focus_idx;
            let interlude_changed = next_interlude != self.interlude;
            self.active_idx = next_active;
            self.focus_idx = next_focus;
            self.interlude = next_interlude;
            if focus_changed || interlude_changed {
                for index in 0..lines.len() {
                    let target_y = self.target_y(index);
                    self.pos_y[index].set_target_position(target_y);
                    self.scale[index].set_target_position(if index == self.focus_idx {
                        1.0
                    } else {
                        0.97
                    });
                }
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

    pub fn focus_idx(&self) -> usize {
        self.focus_idx
    }

    fn group_height(&self, index: usize) -> f64 {
        self.group_heights.get(index).copied().unwrap_or(0.0)
    }

    fn interlude_slot_after(&self, index: usize) -> f64 {
        (self
            .interlude
            .is_some_and(|interlude| interlude.next_idx == index + 1))
        .then_some(self.interlude_slot_height)
        .unwrap_or(0.0)
    }

    pub fn interlude(&self) -> Option<Interlude> {
        self.interlude
    }

    pub fn interlude_top_y(&self, next_idx: usize) -> Option<f32> {
        let interlude = self.interlude?;
        (interlude.next_idx == next_idx).then_some(
            self.pos_y.get(next_idx)?.current_position() as f32 - self.interlude_slot_height as f32,
        )
    }

    fn target_y(&self, index: usize) -> f64 {
        let focus_height = self.group_height(self.focus_idx);
        let focus_top = self.height / 2.0 - focus_height / 2.0;
        if index < self.focus_idx {
            let preceding = (index..self.focus_idx)
                .map(|item| {
                    self.group_height(item) + self.group_gap + self.interlude_slot_after(item)
                })
                .sum::<f64>();
            focus_top - preceding
        } else if index == self.focus_idx {
            focus_top
        } else {
            let following = (self.focus_idx..index)
                .map(|item| {
                    self.group_height(item) + self.group_gap + self.interlude_slot_after(item)
                })
                .sum::<f64>();
            focus_top + following
        }
    }

    fn set_targets(&mut self, lines: &[LyricLine]) {
        for index in 0..lines.len() {
            let target_y = self.target_y(index);
            self.pos_y[index].set_position(target_y);
            self.scale[index].set_position(if index == self.focus_idx { 1.0 } else { 0.97 });
        }
    }
}
fn anticipatory_index(lines: &[LyricLine], t_ms: u64, current: usize) -> usize {
    let Some(next) = lines.get(current + 1) else {
        return current;
    };
    if let Some(interlude) = interlude_between(lines, current + 1) {
        let float_at = interlude
            .end_ms
            .saturating_add(INTERLUDE_FLOAT_DELAY_MS)
            .min(next.start_ms.saturating_sub(1));
        return (t_ms >= float_at).then_some(current + 1).unwrap_or(current);
    }
    let current_start = lines[current].start_ms;
    let interval = next.start_ms.saturating_sub(current_start);
    let anticipation = (interval / 3).clamp(MIN_ANTICIPATION_MS, MAX_ANTICIPATION_MS);
    let scan_end = scan_end_ms(&lines[current]).min(next.start_ms);
    let transition_at = next
        .start_ms
        .saturating_sub(anticipation)
        .max(scan_end)
        .max(current_start);
    if t_ms >= transition_at {
        current + 1
    } else {
        current
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
                background_vocal: None,
                words: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn scroll_begins_before_next_line_start_without_early_highlight() {
        let lines = lines();
        let mut layout = Layout::new(&lines, 1_080.0, &[70.0, 70.0, 70.0], 0.0, 0.0);
        layout.update(&lines, 0, 30);
        let initial = layout.pos_y[0].current_position();
        let initial_scale = layout.scale[0].current_position();
        layout.update(&lines, 1_500, 30);
        assert!((layout.pos_y[0].current_position() - initial).abs() < 1e-9);
        assert!((layout.scale[0].current_position() - initial_scale).abs() < 1e-9);
        layout.update(&lines, 1_800, 30);
        assert!(layout.pos_y[0].current_position() < initial);
        assert!(layout.scale[0].current_position() < initial_scale);
        assert!(layout.scale[1].current_position() > 0.97);
        assert_eq!(layout.active_idx(), 0);
        layout.update(&lines, 2_000, 30);
        assert_eq!(layout.active_idx(), 1);
    }
    #[test]
    fn transition_moves_over_multiple_frames() {
        let lines = lines();
        let mut layout = Layout::new(&lines, 1_080.0, &[70.0, 70.0, 70.0], 0.0, 0.0);
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
        let mut layout = Layout::new(&lines, 1_080.0, &[70.0, 70.0, 70.0], 0.0, 0.0);
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

    #[test]
    fn variable_group_heights_keep_the_gap_constant() {
        let lines = lines();
        let group_heights = [100.0, 200.0, 100.0];
        let mut layout = Layout::new(&lines, 1_080.0, &group_heights, 20.0, 0.0);
        layout.update(&lines, 0, 30);
        let first = layout.pos_y[0].current_position();
        let second = layout.pos_y[1].current_position();
        let third = layout.pos_y[2].current_position();
        assert!((second - (first + group_heights[0] + 20.0)).abs() < 1e-9);
        assert!((third - (second + group_heights[1] + 20.0)).abs() < 1e-9);
    }

    #[test]
    fn interlude_dots_end_before_next_line_float() {
        let lines = vec![
            LyricLine {
                start_ms: 0,
                end_ms: 1_000,
                text: "First".to_owned(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: Vec::new(),
            },
            LyricLine {
                start_ms: 6_000,
                end_ms: 7_000,
                text: "Second".to_owned(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: Vec::new(),
            },
        ];
        let interlude = interlude_for(&lines, 2_000).unwrap();
        assert_eq!(interlude.start_ms, 1_000);
        assert_eq!(interlude.end_ms, 5_750);
        let mut layout = Layout::new(&lines, 1_080.0, &[70.0, 70.0], 0.0, 40.0);
        layout.update(&lines, 2_000, 30);
        assert!(
            (layout.pos_y[1].current_position() - layout.pos_y[0].current_position() - 110.0).abs()
                < 1e-9
        );
        layout.update(&lines, 5_800, 30);
        assert_eq!(layout.focus_idx, 0);
        layout.update(&lines, 5_820, 30);
        assert_eq!(layout.focus_idx, 1);
        assert_eq!(layout.active_idx(), 0);
    }
}
