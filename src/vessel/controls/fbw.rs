use bevy::math::{DQuat, DVec3};

/// Simple PID directional controller.
#[derive(Debug, Clone)]
pub struct PidDirectionalFbw {
    /// Proportional gain
    pub p: f64,
    /// Integral gain
    pub i: f64,
    /// Derivative gain
    pub d: f64,
    /// Integral wind-up guard (absolute)
    pub i_limit: f64,

    integral: DVec3,
    last_err: DVec3,
}

impl PidDirectionalFbw {
    pub fn new(p: f64, i: f64, d: f64, i_limit: f64) -> Self {
        Self {
            p,
            i,
            d,
            i_limit,
            integral: DVec3::ZERO,
            last_err: DVec3::ZERO,
        }
    }

    /// Quaternion error expressed in **body** frame as a scaled-axis vector.
    #[inline]
    fn body_error_vec(current: DQuat, target: DQuat) -> DVec3 {
        // q_err rotates from current orientation to target, expressed in body frame
        let mut q_err = current.conjugate() * target;

        // Enforce shortest-arc representation
        if q_err.w < 0.0 {
            q_err = DQuat::from_xyzw(-q_err.x, -q_err.y, -q_err.z, -q_err.w);
        }

        q_err.to_scaled_axis()
    }
}

impl PidDirectionalFbw {
    pub fn step(&mut self, current: DQuat, target: DQuat, dt: f64) -> DVec3 {
        let error = Self::body_error_vec(current, target);

        self.integral += error * dt;
        self.integral = self
            .integral
            .clamp(DVec3::splat(-self.i_limit), DVec3::splat(self.i_limit));

        let derivative = if dt > 0.0 {
            (error - self.last_err) / dt
        } else {
            DVec3::ZERO
        };

        let output = self.p * error + self.i * self.integral + self.d * derivative;

        self.last_err = error;

        output
    }
}

/// A PID-based rotational fly-by-wire.
pub struct PidRotationalFbw {
    p: f64,
    i: f64,
    d: f64,
    i_limit: f64,

    integral: DVec3,
    last_err: DVec3,
}

impl PidRotationalFbw {
    pub fn new(p: f64, i: f64, d: f64, i_limit: f64) -> Self {
        Self {
            p,
            i,
            d,
            i_limit,
            integral: Default::default(),
            last_err: Default::default(),
        }
    }
}

impl PidRotationalFbw {
    pub fn step(&mut self, current: DVec3, target: DVec3, dt: f64) -> DVec3 {
        let error = target - current;

        self.integral += error * dt;
        self.integral = self
            .integral
            .clamp(DVec3::splat(-self.i_limit), DVec3::splat(self.i_limit));

        let derivative = if dt > 0.0 {
            (error - self.last_err) / dt
        } else {
            DVec3::ZERO
        };

        let output = self.p * error + self.i * self.integral + self.d * derivative;

        self.last_err = error;

        output
    }
}
