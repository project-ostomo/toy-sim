//! Angular-momentum kick followed by a symmetric free-rigid-body drift.
//!
//! Split H(L) = 1/2 Lᵀ I⁻¹ L into three rank-one quadratic Hamiltonians using
//! I⁻¹ = C Cᵀ. Each H_i = 1/2 (c_i · L)² has an exact flow: rotate the body
//! about c_i while keeping world angular momentum fixed. Composing these flows
//! as A/2, B/2, C, B/2, A/2 gives a second-order free drift. Unlike explicit
//! Euler's gyroscopic update, this cannot invent unbounded angular momentum.
//! Cholesky handles non-diagonal inertia without storing principal axes.
use bevy::math::{DMat3, DQuat, DVec3};

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
        let cat = osg_ships::Catalogue::builtin();
        let d = osg_ships::starter(osg_ships::EXAMPLE_CONTROLLER.to_vec())
            .compile(&cat)
            .unwrap();
        let mut hardware = osg_ships::ShipState::new(&d, &cat);
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
}
