fn sample_curve(a1: f64, a2: f64, t: f64) -> f64 {
    3.0 * (1.0 - t).powi(2) * t * a1 + 3.0 * (1.0 - t) * t.powi(2) * a2 + t.powi(3)
}

fn sample_derivative(a1: f64, a2: f64, t: f64) -> f64 {
    3.0 * (1.0 - t).powi(2) * a1 + 6.0 * (1.0 - t) * t * (a2 - a1) + 3.0 * t.powi(2) * (1.0 - a2)
}

pub fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if x == 0.0 { return 0.0; }
    if x == 1.0 { return 1.0; }
    let mut t = x;
    for _ in 0..8 {
        let error = sample_curve(x1, x2, t) - x;
        let derivative = sample_derivative(x1, x2, t);
        if derivative.abs() < 1e-7 { break; }
        t = (t - error / derivative).clamp(0.0, 1.0);
    }
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..24 {
        let current = sample_curve(x1, x2, t);
        if (current - x).abs() < 1e-8 { break; }
        if current < x { low = t; } else { high = t; }
        t = (low + high) * 0.5;
    }
    sample_curve(y1, y2, t).clamp(0.0, 1.0)
}

pub fn bez_in(x: f64) -> f64 { bezier(0.2, 0.4, 0.58, 1.0, x) }
pub fn bez_out(x: f64) -> f64 { bezier(0.3, 0.0, 0.58, 1.0, x) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_has_exact_endpoints() {
        assert_eq!(bez_in(0.0), 0.0);
        assert_eq!(bez_in(1.0), 1.0);
        assert_eq!(bez_out(0.0), 0.0);
        assert_eq!(bez_out(1.0), 1.0);
    }

    #[test]
    fn easing_stays_monotonic_and_bounded() {
        let mut previous = 0.0;
        for index in 0..=100 {
            let value = bez_in(index as f64 / 100.0);
            assert!((0.0..=1.0).contains(&value));
            assert!(value >= previous);
            previous = value;
        }
    }
}
