use glam::{DQuat, DVec3};
use osg_ship_api::abi;

const NAVIGATION_GAIN: f64 = 3.5;
const CRUISE_CLOSING_SPEED_M_S: f64 = 1_200.0;

pub fn guide(observation: &abi::MissileObservation) -> abi::MissileControl {
    let rotation = DQuat::from_array(observation.rotation);
    let rotation = if rotation.is_finite() && rotation.length_squared() > 1e-12 {
        rotation.normalize()
    } else {
        DQuat::IDENTITY
    };
    let forward = rotation * DVec3::NEG_Z;
    let coast = abi::MissileControl {
        direction: forward.to_array(),
        throttle: 0.0,
    };
    let offset = DVec3::from_array(observation.target_offset_m);
    let relative_velocity = DVec3::from_array(observation.target_relative_velocity_m_s);
    let acceleration = observation.maximum_acceleration_m_s2;
    if observation.target_visible == 0
        || observation.fuel_units == 0
        || !offset.is_finite()
        || !relative_velocity.is_finite()
        || !acceleration.is_finite()
        || acceleration <= 0.0
        || !observation.dt_s.is_finite()
        || observation.dt_s <= 0.0
        || !observation.turn_rate_rad_s.is_finite()
        || observation.turn_rate_rad_s <= 0.0
        || !observation.target_uncertainty_m.is_finite()
    {
        return coast;
    }

    let range = offset.length();
    if range < 1.0 {
        return coast;
    }
    let line_of_sight = offset / range;
    let radial_speed = relative_velocity.dot(line_of_sight);
    let closing_speed = -radial_speed;
    let desired_closing_speed = (2.0 * acceleration * range)
        .sqrt()
        .clamp(150.0, CRUISE_CLOSING_SPEED_M_S);

    // Proportional navigation cancels line-of-sight rotation. During acquisition,
    // a small assumed closing speed starts leading a crossing target immediately.
    let effective_closing_speed = closing_speed.max(0.25 * desired_closing_speed);
    let transverse_velocity = relative_velocity - line_of_sight * radial_speed;
    let mut lateral = NAVIGATION_GAIN * effective_closing_speed * transverse_velocity / range;
    let time_to_go = (range / effective_closing_speed).max(observation.dt_s);
    let uncertainty_acceleration = observation.target_uncertainty_m.max(0.0) / time_to_go.powi(2);
    if lateral.length() < uncertainty_acceleration.max(0.05) {
        lateral = DVec3::ZERO;
    }

    // Once closing sufficiently fast, coast and spend fuel only on corrections.
    let approach = ((desired_closing_speed - closing_speed) / 2.0).clamp(0.0, acceleration);
    let commanded_acceleration = lateral + line_of_sight * approach;
    let magnitude = commanded_acceleration.length();
    if !magnitude.is_finite() || magnitude < 0.05 {
        return coast;
    }
    let direction = commanded_acceleration / magnitude;
    let angular_velocity = DVec3::from_array(observation.angular_velocity_rad_s);
    let predicted_forward = if angular_velocity.is_finite() {
        DQuat::from_scaled_axis(angular_velocity * observation.dt_s.min(0.2))
            * rotation
            * DVec3::NEG_Z
    } else {
        forward
    };
    let alignment = forward
        .dot(direction)
        .min(predicted_forward.dot(direction))
        .clamp(0.0, 1.0);

    abi::MissileControl {
        direction: direction.to_array(),
        throttle: (magnitude / acceleration).min(1.0) * alignment.powi(4),
    }
}

#[cfg(all(target_arch = "wasm32", feature = "firmware"))]
#[unsafe(no_mangle)]
extern "C" fn missile_tick(handle: u64) {
    use osg_ship_api::sdk;

    // Only callback-local state is used; a suspended callback never borrows the
    // parent's Computer or another missile's guidance state.
    let observation = sdk::missile().expect("missile observation failed");
    assert_eq!(observation.handle, handle);
    sdk::control_missile(&guide(&observation)).expect("missile control failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> abi::MissileObservation {
        abi::MissileObservation {
            handle: 1,
            target_visible: 1,
            target_offset_m: [0.0, 0.0, -5_000.0],
            rotation: DQuat::IDENTITY.to_array(),
            maximum_acceleration_m_s2: 62.5,
            turn_rate_rad_s: 3.0,
            fuel_units: 240,
            dt_s: 0.1,
            ..Default::default()
        }
    }

    #[test]
    fn guidance_coasts_without_a_target_fuel_or_a_needed_correction() {
        let mut sample = observation();
        assert!(guide(&sample).throttle > 0.9);

        sample.target_visible = 0;
        assert_eq!(guide(&sample).throttle, 0.0);
        sample.target_visible = 1;
        sample.fuel_units = 0;
        assert_eq!(guide(&sample).throttle, 0.0);

        sample.fuel_units = 240;
        sample.target_relative_velocity_m_s = [0.0, 0.0, 1_300.0];
        assert_eq!(guide(&sample).throttle, 0.0);
    }

    #[test]
    fn turning_limits_burn_and_guidance_is_independent_between_missiles() {
        let mut sample = observation();
        sample.target_offset_m = [5_000.0, 0.0, 0.0];
        let initial = guide(&sample);
        assert_eq!(initial.throttle, 0.0);
        assert!(DVec3::from_array(initial.direction).dot(DVec3::X) > 0.999);

        let mut other = observation();
        other.handle = 99;
        other.target_relative_velocity_m_s = [-400.0, 0.0, 0.0];
        let _ = guide(&other);
        assert_eq!(guide(&sample), initial);

        sample.target_visible = 0;
        sample.target_offset_m = [f64::NAN; 3];
        let hidden = guide(&sample);
        assert!(DVec3::from_array(hidden.direction).is_finite());
        assert_eq!(hidden.throttle, 0.0);
    }

    #[test]
    fn world_space_roll_does_not_reduce_a_correctly_pointed_burn() {
        let rotation = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        let forward = rotation * DVec3::NEG_Z;
        let sample = abi::MissileObservation {
            rotation: rotation.to_array(),
            target_offset_m: (forward * 5_000.0).to_array(),
            angular_velocity_rad_s: (forward * 3.0).to_array(),
            ..observation()
        };
        assert!(guide(&sample).throttle > 0.999);
    }

    #[test]
    fn finite_fuel_and_turn_rate_intercept_crossing_and_receding_targets() {
        for (position, velocity) in [
            (DVec3::NEG_Z * 1_000.0, DVec3::ZERO),
            (DVec3::NEG_Z * 5_000.0, DVec3::X * 200.0),
            (DVec3::NEG_Z * 20_000.0, DVec3::NEG_Z * 250.0),
            (DVec3::X * 5_000.0, DVec3::Z * 200.0),
        ] {
            let (miss, fuel) = intercept(position, velocity, 90.0);
            assert!(
                miss < 5.0,
                "missed {position:?} at {velocity:?}: {miss:.1} m"
            );
            assert!(fuel > 20.0, "needlessly exhausted fuel: {fuel:.1} kg");
        }
    }

    #[test]
    fn long_range_intercept_coasts_and_preserves_correction_fuel() {
        let (miss, fuel) = intercept(DVec3::NEG_Z * 1_000_000.0, DVec3::ZERO, 900.0);
        assert!(
            miss < 5.0,
            "missed stationary distant target by {miss:.1} m"
        );
        assert!(fuel > 80.0, "did not coast after acquisition: {fuel:.1} kg");
    }

    fn intercept(mut target: DVec3, target_velocity: DVec3, duration: f64) -> (f64, f64) {
        let dt = 0.02;
        let mut position = DVec3::ZERO;
        let mut velocity = DVec3::NEG_Z * 60.0;
        let mut rotation = DQuat::IDENTITY;
        let mut fuel = 240.0;
        let mut closest = f64::INFINITY;
        let mut control = abi::MissileControl::default();
        for step in 0..(duration / dt) as usize {
            let offset = target - position;
            let acceleration = 25_000.0 / (160.0 + fuel);
            let sample = abi::MissileObservation {
                rotation: rotation.to_array(),
                target_offset_m: offset.to_array(),
                target_relative_velocity_m_s: (target_velocity - velocity).to_array(),
                maximum_acceleration_m_s2: acceleration,
                fuel_units: fuel as u64,
                dt_s: 0.1,
                time_s: step as f64 * dt,
                ..observation()
            };
            if step % 5 == 0 {
                control = guide(&sample);
            }
            assert!((0.0..=1.0).contains(&control.throttle));
            let forward = rotation * DVec3::NEG_Z;
            let direction = DVec3::from_array(control.direction);
            let turn = DQuat::from_rotation_arc(forward, direction);
            let (axis, angle) = turn.to_axis_angle();
            rotation = (DQuat::from_axis_angle(axis, angle.min(3.0 * dt)) * rotation).normalize();
            let consumed = (25_000.0 / 3_000.0 * control.throttle * dt).min(fuel);
            fuel -= consumed;
            velocity += rotation * DVec3::NEG_Z * acceleration * control.throttle * dt;
            position += velocity * dt;
            target += target_velocity * dt;

            let segment = target - position - offset;
            let fraction =
                (-offset.dot(segment) / segment.length_squared().max(1e-12)).clamp(0.0, 1.0);
            closest = closest.min((offset + fraction * segment).length());
            if closest < 5.0 {
                break;
            }
        }
        (closest, fuel)
    }
}
