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

    fn is_state_finite(&self) -> bool {
        self.x_prev.is_finite() && self.dx_prev.is_finite() && self.raw_x_prev.is_finite()
    }

    fn seed(&mut self, x: f32) -> f32 {
        self.initialized = true;
        self.raw_x_prev = x;
        self.x_prev = x;
        self.dx_prev = 0.0;
        x
    }

    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        // Reject any non-finite sample before it can reach the filter state. An
        // infinite sample would produce Inf - Inf on the next call, and the
        // resulting NaN would persist for the lifetime of the filter.
        if !x.is_finite() {
            return if self.initialized { self.x_prev } else { 0.0 };
        }

        // Derive sample rate from frame delta time
        let hz = if dt > 0.0 { 1.0 / dt } else { self.hz };
        self.hz = hz;

        if !self.initialized || !self.is_state_finite() {
            return self.seed(x);
        }

        let dx = (x - self.raw_x_prev) * hz;
        self.raw_x_prev = x;

        let edx = Self::low_pass(&mut self.dx_prev, dx, Self::alpha(hz, self.d_cutoff));
        let cutoff = self.min_cutoff + self.beta * edx.abs();

        let filtered = Self::low_pass(&mut self.x_prev, x, Self::alpha(hz, cutoff));

        // A sufficiently small dt drives hz high enough to overflow the
        // derivative term even for finite input, so re-seed rather than latch.
        if !filtered.is_finite() {
            return self.seed(x);
        }

        filtered
    }
}
