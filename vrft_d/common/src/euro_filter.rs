#[derive(Debug, Clone, Copy)]
pub struct EuroFilter {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    hz: f32,
    x_prev: f32,
    dx_prev: f32,
    raw_x_prev: f32,
    initialized: bool,
}

impl Default for EuroFilter {
    fn default() -> Self {
        Self {
            min_cutoff: 1.0,
            beta: 0.5,
            d_cutoff: 1.0,
            hz: 10.0,
            x_prev: 0.0,
            dx_prev: 0.0,
            raw_x_prev: 0.0,
            initialized: false,
        }
    }
}

impl EuroFilter {
    pub fn new() -> Self {
        Self {
            d_cutoff: 0.1,
            ..Default::default()
        }
    }

    pub fn new_with_config(min_cutoff: f32, beta: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            d_cutoff: 0.1,
            ..Default::default()
        }
    }

    fn alpha(hz: f32, cutoff: f32) -> f32 {
        let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        let te = 1.0 / hz;
        1.0 / (1.0 + tau / te)
    }

    fn low_pass(hat_x_prev: &mut f32, x: f32, alpha: f32) -> f32 {
        let hat_x = alpha * x + (1.0 - alpha) * *hat_x_prev;
        *hat_x_prev = hat_x;
        hat_x
    }

    pub fn filter(&mut self, x: f32) -> f32 {
        if x.is_nan() {
            return 0.0;
        }

        if !self.initialized {
            self.initialized = true;
            self.raw_x_prev = x;
            self.x_prev = x;
            self.dx_prev = 0.0;
            return x;
        }

        let dx = (x - self.raw_x_prev) * self.hz;
        self.raw_x_prev = x;

        let edx = Self::low_pass(&mut self.dx_prev, dx, Self::alpha(self.hz, self.d_cutoff));
        let cutoff = self.min_cutoff + self.beta * edx.abs();

        Self::low_pass(&mut self.x_prev, x, Self::alpha(self.hz, cutoff))
    }
}
