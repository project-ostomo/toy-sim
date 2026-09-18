//! Rate-limited pointing using the discovered body-frame torque authority.
use crate::Bindings;
use glam::{DMat3, DQuat, DVec3};

pub const MAX_RATE: f64 = 2.5;

pub fn valid_rotation(raw: [f64; 4]) -> Option<DQuat> {
    let q = DQuat::from_array(raw);
    (q.is_finite() && q.length_squared() > 1e-12).then(|| q.normalize())
}

pub fn point(q: DQuat, engine_axis: DVec3, direction: DVec3) -> DQuat {
    DQuat::from_rotation_arc(q * engine_axis, direction) * q
}

pub fn error_angle(q: DQuat, axis: DVec3, direction: DVec3) -> f64 {
    (q * axis).dot(direction).clamp(-1., 1.).acos()
}

fn acceleration_limit(inertia: DMat3, axis: DVec3, b: &Bindings) -> f64 {
    let torque_per_acceleration = b.torquer_rotation.inverse() * (inertia * axis);
    (0..3)
        .filter(|&i| torque_per_acceleration[i].abs() > 1e-6)
        .map(|i| b.torque_capacity[i] / torque_per_acceleration[i].abs())
        .fold(f64::INFINITY, f64::min)
        .clamp(1e-6, 1e6)
}

/// Requested body torque, including gyroscopic compensation. Allocation handles lever arms.
pub fn torque(q: DQuat, omega: DVec3, inertia: DMat3, target: DQuat, b: &Bindings) -> DVec3 {
    let mut dq = q.inverse() * target;
    if dq.w < 0. {
        dq = -dq;
    }
    let error = dq.to_scaled_axis();
    let axis = error.try_normalize().unwrap_or(DVec3::X);
    let alpha = acceleration_limit(inertia, axis, b) * 0.7;
    // The linear cap near zero avoids a discontinuous square-root servo at 10 Hz.
    let rate = (2. * alpha * error.length())
        .sqrt()
        .min(MAX_RATE)
        .min(error.length() * 3.0);
    let body_w = q.inverse() * omega;
    let angular_a = (axis * rate - body_w) * 6.;
    let body_torque = inertia * angular_a + body_w.cross(inertia * body_w);
    (b.torquer_rotation.inverse() * body_torque).clamp(-b.torque_capacity, b.torque_capacity)
}

/// Conservative rest-to-rest half-turn, using a bound on the inertia tensor.
/// Does not depend on pointing error: keep the stopping envelope stable in a flip.
pub fn turn_allowance(inertia: DMat3, b: &Bindings) -> f64 {
    let moment = inertia
        .x_axis
        .abs()
        .element_sum()
        .max(inertia.y_axis.abs().element_sum())
        .max(inertia.z_axis.abs().element_sum());
    let alpha = (0.7 * b.torque_limit / moment.max(1e-9)).max(1e-6);
    let angle = core::f64::consts::PI;
    if angle < MAX_RATE * MAX_RATE / alpha {
        2. * (angle / alpha).sqrt() + 1.
    } else {
        angle / MAX_RATE + MAX_RATE / alpha + 1.
    }
}
