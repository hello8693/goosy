#[derive(Debug, Clone)]
pub struct Spring {
    current_position: f64,
    target_position: f64,
    stiffness: f64,
    damping: f64,
    mass: f64,
    soft: bool,
    time: f64,
    from: f64,
    velocity: f64,
    initial_velocity: f64,
}

fn solve_spring(from: f64, velocity: f64, to: f64, stiffness: f64, damping: f64, mass: f64, soft: bool, t: f64) -> f64 {
    let delta = to - from;
    if soft || damping >= 2.0 * (stiffness * mass).sqrt() {
        let af = -(stiffness / mass).sqrt();
        let leftover = -af * delta - velocity;
        to - (delta + t * leftover) * (t * af).exp()
    } else {
        let df = (4.0 * mass * stiffness - damping * damping).sqrt();
        let leftover = (damping * delta - 2.0 * mass * velocity) / df;
        let dfm = df / (2.0 * mass);
        let dm = -damping / (2.0 * mass);
        to - ((t * dfm).cos() * delta + (t * dfm).sin() * leftover) * (t * dm).exp()
    }
}

impl Spring {
    pub fn new(position: f64) -> Self {
        Self::with_parameters(position, 100.0, 10.0, 1.0, false)
    }

    pub fn with_parameters(position: f64, stiffness: f64, damping: f64, mass: f64, soft: bool) -> Self {
        Self {
            current_position: position,
            target_position: position,
            stiffness,
            damping,
            velocity: 0.0,
            initial_velocity: 0.0,
            mass,
            soft,
            time: 0.0,
            from: position,
        }
    }
    pub fn set_parameters(&mut self, stiffness: f64, damping: f64, mass: f64, soft: bool) {
        self.stiffness = stiffness;
        self.damping = damping;
        self.mass = mass;
        self.soft = soft;
        self.from = self.current_position;
        self.initial_velocity = self.velocity;
        self.time = 0.0;
    }


    pub fn set_position(&mut self, position: f64) {
        self.current_position = position;
        self.target_position = position;
        self.from = position;
        self.velocity = 0.0;
        self.initial_velocity = 0.0;
        self.time = 0.0;
    }

    pub fn set_target_position(&mut self, position: f64) {
        if (self.target_position - position).abs() < 0.001 { return; }
        self.target_position = position;
        self.from = self.current_position;
        self.initial_velocity = self.velocity;
        self.time = 0.0;
    }

    pub fn update(&mut self, dt: f64) {
        if (self.current_position - self.target_position).abs() < 0.01 && self.velocity.abs() < 0.01 {
            self.set_position(self.target_position);
            return;
        }
        self.time += dt.max(0.0);
        self.current_position = self.sample(self.time);
        let h = 1e-4;
        self.velocity = (self.sample(self.time + h) - self.sample(self.time - h)) / (2.0 * h);
        if (self.current_position - self.target_position).abs() < 0.01 && self.velocity.abs() < 0.01 {
            self.set_position(self.target_position);
        }
    }

    pub fn current_position(&self) -> f64 { self.current_position }

    pub fn current_velocity(&self) -> f64 { self.velocity }

    fn sample(&self, time: f64) -> f64 {
        solve_spring(self.from, self.initial_velocity, self.target_position, self.stiffness, self.damping, self.mass, self.soft, time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_converges_with_default_policy() {
        let mut spring = Spring::with_parameters(0.0, 220.0, 33.0, 1.0, false);
        spring.set_target_position(1.0);
        for _ in 0..120 { spring.update(1.0 / 60.0); }
        assert!((spring.current_position() - 1.0).abs() < 0.01);
    }

    #[test]
    fn matches_closed_form_samples() {
        let mut spring = Spring::with_parameters(0.0, 4.0, 1.0, 1.0, false);
        spring.set_target_position(1.0);
        let expected = [0.0, 0.11286328, 0.39294515];
        for (index, expected) in expected.into_iter().enumerate() {
            if index > 0 { spring.update(0.25); }
            assert!((spring.current_position() - expected).abs() < 0.00001, "sample {index}: {}", spring.current_position());
        }
    }

    #[test]
    fn overdamped_motion_does_not_overshoot() {
        let mut spring = Spring::with_parameters(0.0, 100.0, 30.0, 1.0, false);
        spring.set_target_position(1.0);
        let mut previous = 0.0;
        for _ in 0..300 {
            spring.update(1.0 / 60.0);
            assert!(spring.current_position() >= previous - 1e-9);
            assert!(spring.current_position() <= 1.0 + 1e-9);
            previous = spring.current_position();
        }
    }
}
