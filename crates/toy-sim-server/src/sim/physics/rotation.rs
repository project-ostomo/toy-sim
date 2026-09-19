//! Angular-momentum kick followed by a symmetric free-rigid-body drift.
//!
//! Split H(L) = 1/2 Lᵀ I⁻¹ L into three rank-one quadratic Hamiltonians using
//! I⁻¹ = C Cᵀ. Each H_i = 1/2 (c_i · L)² has an exact flow: rotate the body
//! about c_i while keeping world angular momentum fixed. Composing these flows
//! as A/2, B/2, C, B/2, A/2 gives a second-order free drift. Unlike explicit
//! Euler's gyroscopic update, this cannot invent unbounded angular momentum.
//! Cholesky handles non-diagonal inertia without storing principal axes.
use bevy::math::{DMat3, DQuat, DVec3};

const MAX_SEGMENT_ANGLE: f64 = 0.1;
const MAX_SEGMENTS: usize = 4096;

#[derive(Clone)]
pub struct RotationTrajectory {
    rotations: Vec<DQuat>,
    momentum: DVec3,
    inertia_inv: DMat3,
    duration: f64,
    step: f64,
    angular_bound: f64,
    requires_rotational_envelope: bool,
}

impl RotationTrajectory {
    pub fn new(rotation: DQuat, momentum: DVec3, inertia_inv: DMat3, duration: f64) -> Self {
        assert!(duration.is_finite() && duration >= 0.0);
        assert!(rotation.is_finite() && momentum.is_finite() && inertia_inv.is_finite());
        let axes = cholesky_columns(inertia_inv);
        let momentum_length = momentum.length();
        let coefficients = [
            0.5 * axes[0].length_squared() * momentum_length,
            0.5 * axes[1].length_squared() * momentum_length,
            axes[2].length_squared() * momentum_length,
            0.5 * axes[1].length_squared() * momentum_length,
            0.5 * axes[0].length_squared() * momentum_length,
        ];
        let total_rate: f64 = coefficients.iter().sum();
        assert!(total_rate.is_finite() && total_rate >= 0.0);
        let requested = (duration * total_rate / MAX_SEGMENT_ANGLE).ceil().max(1.0);
        let requires_rotational_envelope = requested > MAX_SEGMENTS as f64;
        let segments = requested.min(MAX_SEGMENTS as f64) as usize;
        let step = duration / segments as f64;
        let mut rotations = Vec::with_capacity(segments + 1);
        rotations.push(rotation);

        let mut max_step: f64 = 0.0;
        for index in 0..segments {
            let start = index as f64 * step;
            let end = if index + 1 == segments {
                duration
            } else {
                (index + 1) as f64 * step
            };
            let elapsed = end - start;
            max_step = max_step.max(elapsed);
            rotations.push(drift(rotations[index], momentum, inertia_inv, elapsed).0);
        }

        // Across a segment, the drift derivative obeys b' = (1 + a*h)*b + a.
        // Its closed form is (product(1 + a*h) - 1) / h, bounded by
        // expm1(sum(a)*h) / h. Short segments keep this near sum(a), even
        // when the body completes many rotations during one physics tick.
        let angular_bound = coefficients
            .iter()
            .fold(0.0, |bound, rate| (1.0 + rate * max_step) * bound + rate);

        Self {
            rotations,
            momentum,
            inertia_inv,
            duration,
            step,
            angular_bound,
            requires_rotational_envelope,
        }
    }

    pub fn sample(&self, elapsed: f64) -> (DQuat, DVec3) {
        let elapsed = elapsed.clamp(0.0, self.duration);
        if elapsed == self.duration || self.step == 0.0 {
            let rotation = *self.rotations.last().expect("rotation trajectory endpoint");
            let velocity = rotation * (self.inertia_inv * (rotation.inverse() * self.momentum));
            return (rotation, velocity);
        }

        let index = ((elapsed / self.step).floor() as usize).min(self.rotations.len() - 2);
        drift(
            self.rotations[index],
            self.momentum,
            self.inertia_inv,
            elapsed - index as f64 * self.step,
        )
    }

    pub fn angular_bound(&self) -> f64 {
        self.angular_bound
    }

    pub fn requires_rotational_envelope(&self) -> bool {
        self.requires_rotational_envelope
    }
}

pub fn integrate(
    rotation: DQuat,
    angular_velocity: DVec3,
    torque: DVec3,
    inertia: DMat3,
    inertia_inv: DMat3,
    dt: f64,
) -> (DQuat, DVec3) {
    if dt == 0. {
        return (rotation, angular_velocity);
    }
    // Transient world momentum, reconstructed from the existing ECS state.
    let momentum = rotation * (inertia * (rotation.inverse() * angular_velocity)) + torque * dt;
    drift(rotation, momentum, inertia_inv, dt)
}

pub fn drift(mut rotation: DQuat, momentum: DVec3, inertia_inv: DMat3, dt: f64) -> (DQuat, DVec3) {
    if momentum == DVec3::ZERO {
        return (rotation, DVec3::ZERO);
    }
    if dt == 0.0 {
        return (
            rotation,
            rotation * (inertia_inv * (rotation.inverse() * momentum)),
        );
    }
    let axes = cholesky_columns(inertia_inv);
    // One symmetric drift per physics tick; no angular-speed-dependent work.
    for (axis, duration) in [
        (axes[0], dt * 0.5),
        (axes[1], dt * 0.5),
        (axes[2], dt),
        (axes[1], dt * 0.5),
        (axes[0], dt * 0.5),
    ] {
        let body_momentum = rotation.inverse() * momentum;
        let increment = axis * (axis.dot(body_momentum) * duration);
        rotation = (rotation * DQuat::from_scaled_axis(increment)).normalize();
    }
    let angular_velocity = rotation * (inertia_inv * (rotation.inverse() * momentum));
    (rotation, angular_velocity)
}

/// Columns of C such that C Cᵀ = inverse inertia (symmetric positive definite).
pub(super) fn cholesky_columns(a: DMat3) -> [DVec3; 3] {
    let c00 = a.x_axis.x.sqrt();
    let c10 = a.x_axis.y / c00;
    let c20 = a.x_axis.z / c00;
    let c11 = (a.y_axis.y - c10 * c10).sqrt();
    let c21 = (a.y_axis.z - c20 * c10) / c11;
    let c22 = (a.z_axis.z - c20 * c20 - c21 * c21).sqrt();
    [
        DVec3::new(c00, c10, c20),
        DVec3::new(0., c11, c21),
        DVec3::new(0., 0., c22),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter_inertia() -> DMat3 {
        let cat = toy_sim_ships::Catalogue::builtin();
        let d = toy_sim_ships::starter(toy_sim_ships::EXAMPLE_CONTROLLER.to_vec())
            .compile(&cat)
            .unwrap();
        let mut hardware = toy_sim_ships::ShipState::new(&d, &cat);
        hardware.test_loadout(&d, &cat);
        hardware.mass_properties(&d, &cat).1
    }
    fn momentum(q: DQuat, w: DVec3, inertia: DMat3) -> DVec3 {
        q * (inertia * (q.inverse() * w))
    }
    fn energy(q: DQuat, w: DVec3, inertia: DMat3) -> f64 {
        let body_w = q.inverse() * w;
        0.5 * body_w.dot(inertia * body_w)
    }

    #[test]
    fn torque_free_starter_roll_does_not_amplify_tiny_off_axis_errors() {
        let inertia = starter_inertia();
        let inverse = inertia.inverse();
        let mut q = DQuat::from_euler(bevy::math::EulerRot::XYZ, 0.3, 0.5, 0.7);
        let mut w = q * DVec3::new(1e-12, 0., 10.);
        let initial_l = momentum(q, w, inertia);
        let initial_e = energy(q, w, inertia);
        let mut max_energy_error = 0f64;
        for _ in 0..6000 {
            (q, w) = integrate(q, w, DVec3::ZERO, inertia, inverse, 0.1);
            assert!(q.is_finite() && w.is_finite());
            max_energy_error = max_energy_error.max((energy(q, w, inertia) / initial_e - 1.).abs());
            assert!((momentum(q, w, inertia) - initial_l).length() / initial_l.length() < 1e-8);
        }
        assert!(max_energy_error < 1e-8, "energy error: {max_energy_error}");
    }

    #[test]
    fn sustained_starter_roll_stays_finite_with_one_step() {
        let inertia = starter_inertia();
        let inverse = inertia.inverse();
        let mut q = DQuat::from_euler(bevy::math::EulerRot::XYZ, 0.3, 0.5, 0.7);
        let mut w = DVec3::ZERO;
        let mut expected_l = DVec3::ZERO;
        for _ in 0..6000 {
            let torque = q * DVec3::Z * 20000.;
            expected_l += torque * 0.1;
            (q, w) = integrate(q, w, torque, inertia, inverse, 0.1);
            assert!(q.is_finite() && w.is_finite());
            assert!((q.length_squared() - 1.).abs() < 1e-12);
            assert!(
                (momentum(q, w, inertia) - expected_l).length() / expected_l.length().max(1.)
                    < 1e-8
            );
        }
    }

    fn asymmetric_inertia() -> DMat3 {
        let axes = DMat3::from_quat(DQuat::from_euler(bevy::math::EulerRot::XYZ, 0.4, -0.3, 0.8));
        axes * DMat3::from_diagonal(DVec3::new(2., 3., 5.)) * axes.transpose()
    }

    #[test]
    fn fast_asymmetric_tumbling_preserves_momentum_and_bounds_energy() {
        let inertia = asymmetric_inertia();
        let inverse = inertia.inverse();
        let mut q = DQuat::from_rotation_y(0.7);
        let mut w = DVec3::new(2., -3., 4.);
        let initial_l = momentum(q, w, inertia);
        // Principal moments are 2, 3, 5: fixed momentum bounds the energy
        // even when a single step does not resolve fast tumbling accurately.
        let min_energy = initial_l.length_squared() / (2. * 5.);
        let max_energy = initial_l.length_squared() / (2. * 2.);
        for _ in 0..6000 {
            (q, w) = integrate(q, w, DVec3::ZERO, inertia, inverse, 0.1);
            let e = energy(q, w, inertia);
            assert!(q.is_finite() && w.is_finite());
            assert!(e >= min_energy * (1. - 1e-8) && e <= max_energy * (1. + 1e-8));
            assert!((momentum(q, w, inertia) - initial_l).length() < 1e-8);
        }
    }

    #[test]
    fn moderate_rotation_agrees_with_fine_reference() {
        let inertia = asymmetric_inertia();
        let inverse = inertia.inverse();
        let run = |dt: f64| {
            let mut q = DQuat::from_rotation_y(0.7);
            let mut w = DVec3::new(0.2, -0.3, 0.4);
            for _ in 0..(10. / dt).round() as usize {
                (q, w) = integrate(q, w, DVec3::ZERO, inertia, inverse, dt);
            }
            (q, w)
        };
        let (q, w) = run(0.1);
        let (fine_q, fine_w) = run(0.001);
        let angle = 2. * q.dot(fine_q).abs().clamp(0., 1.).acos();
        assert!(angle < 0.005, "orientation error: {angle} radians");
        assert!(
            (w - fine_w).length() < 0.005,
            "angular velocity error: {}",
            (w - fine_w).length()
        );
    }

    #[test]
    fn cancelling_torque_stops_spin_and_spherical_principal_spin_is_exact() {
        let inertia = DMat3::from_diagonal(DVec3::splat(5.));
        let q = DQuat::from_rotation_x(0.3);
        let w = DVec3::Y * 2.;
        let (stopped_q, stopped_w) = integrate(q, w, -w * 50., inertia, inertia.inverse(), 0.1);
        assert!(stopped_q.abs_diff_eq(q, 1e-14));
        assert!(stopped_w.length() < 1e-14);

        let (new_q, new_w) = integrate(
            DQuat::IDENTITY,
            DVec3::Z * 2.,
            DVec3::ZERO,
            inertia,
            inertia.inverse(),
            0.1,
        );
        assert!(new_q.abs_diff_eq(DQuat::from_rotation_z(0.2), 1e-12));
        assert!(new_w.abs_diff_eq(DVec3::Z * 2., 1e-12));
    }

    #[test]
    fn cached_missile_tumble_has_a_tight_conservative_bound_across_knots() {
        let inverse = DMat3::from_diagonal(DVec3::new(
            0.007909387700624201,
            0.00824696314249866,
            0.05339145736682132,
        ));
        let inertia = inverse.inverse();
        let q = DQuat::from_xyzw(
            0.5964242606382991,
            0.04332083849778918,
            -0.7742972168942927,
            -0.20703918997054577,
        );
        let world_momentum = DVec3::new(374.7698, -4152.6555, -4109.7523);
        let trajectory = RotationTrajectory::new(q, world_momentum, inverse, 0.1);
        assert!(!trajectory.requires_rotational_envelope());

        let rate =
            world_momentum.length() * (inverse.x_axis.x + inverse.y_axis.y + inverse.z_axis.z);
        assert!(trajectory.angular_bound() < rate * 1.052);
        assert!(trajectory.sample(0.0).0.abs_diff_eq(q, 1e-14));

        let delta = 1e-7;
        for index in 0..trajectory.rotations.len() - 1 {
            let knot = index as f64 * trajectory.step;
            for phase in [-0.5 * delta, 0.0, 0.37 * trajectory.step] {
                let start = (knot + phase).clamp(0.0, 0.1 - delta);
                let (before, velocity) = trajectory.sample(start);
                let (after, _) = trajectory.sample(start + delta);
                assert!(before.is_finite() && velocity.is_finite());
                assert!((momentum(before, velocity, inertia) - world_momentum).length() < 1e-8);
                for point in [DVec3::X, DVec3::Y, DVec3::Z] {
                    let displacement = (after * point - before * point).length();
                    assert!(
                        displacement <= trajectory.angular_bound() * delta * (1.0 + 1e-6),
                        "point displacement {displacement} exceeds speed bound at {start}"
                    );
                }
            }
        }
    }

    #[test]
    fn cached_spherical_spin_matches_the_analytic_solution() {
        let inverse = DMat3::IDENTITY * 0.2;
        let world_momentum = DVec3::Z * 1000.0;
        let trajectory = RotationTrajectory::new(DQuat::IDENTITY, world_momentum, inverse, 0.1);
        for sample in 0..=100 {
            let elapsed = sample as f64 * 0.001;
            let (q, w) = trajectory.sample(elapsed);
            let expected = DQuat::from_rotation_z(200.0 * elapsed);
            assert!(q.dot(expected).abs() > 1.0 - 1e-12);
            assert!(w.abs_diff_eq(DVec3::Z * 200.0, 1e-10));
        }
    }

    #[test]
    fn extreme_spin_uses_bounded_storage_and_requires_collision_envelopes() {
        let trajectory =
            RotationTrajectory::new(DQuat::IDENTITY, DVec3::Z * 1e6, DMat3::IDENTITY, 0.1);
        assert!(trajectory.requires_rotational_envelope());
        assert_eq!(trajectory.rotations.len(), MAX_SEGMENTS + 1);
        let (q, velocity) = trajectory.sample(0.053271);
        assert!(q.is_finite() && velocity.is_finite());
        assert!(velocity.abs_diff_eq(DVec3::Z * 1e6, 1e-8));
        assert!(q.dot(DQuat::from_rotation_z(53271.0)).abs() > 1.0 - 1e-10);

        let stationary = RotationTrajectory::new(q, DVec3::ZERO, DMat3::IDENTITY, 0.0);
        assert_eq!(stationary.sample(0.0), (q, DVec3::ZERO));
        assert_eq!(stationary.angular_bound(), 0.0);
        assert!(!stationary.requires_rotational_envelope());
    }
}
