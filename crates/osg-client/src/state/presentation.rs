use super::*;

pub(super) fn interpolate(
    time: Res<Time<Fixed>>,
    mut clock: ResMut<RenderTime>,
    mut poses: Query<(&PoseSamples, &mut DisplayPose)>,
    mut visuals: Query<(&VisualSamples, &mut DisplayVisual)>,
    mut lights: Query<&mut OpticalLight>,
) {
    let alpha = time.overstep_fraction_f64();
    clock.display_ns = clock
        .previous_ns
        .saturating_add((clock.current_ns.saturating_sub(clock.previous_ns) as f64 * alpha) as u64);
    poses.par_iter_mut().for_each(|(samples, mut pose)| {
        pose.0 = samples.current.clone();
        pose.0.position = samples.previous.position.offset_by(
            samples
                .current
                .position
                .relative_to(samples.previous.position)
                * alpha,
        );
        pose.0.rotation = glam::DQuat::from_array(samples.previous.rotation)
            .slerp(glam::DQuat::from_array(samples.current.rotation), alpha)
            .to_array();
        for axis in 0..3 {
            pose.0.velocity[axis] = samples.previous.velocity[axis]
                + (samples.current.velocity[axis] - samples.previous.velocity[axis]) * alpha;
            pose.0.angular_velocity[axis] = samples.previous.angular_velocity[axis]
                + (samples.current.angular_velocity[axis]
                    - samples.previous.angular_velocity[axis])
                    * alpha;
        }
    });
    for mut light in &mut lights {
        light.display_w = light.previous + (light.current - light.previous) * alpha;
    }
    visuals.par_iter_mut().for_each(|(samples, mut visual)| {
        visual.0 = interpolate_visual(&samples.previous, &samples.current, alpha);
    });
}

fn interpolate_visual(previous: &ShipVisual, current: &ShipVisual, alpha: f64) -> ShipVisual {
    let mut visual = current.clone();
    visual.slip_readiness =
        previous.slip_readiness + (current.slip_readiness - previous.slip_readiness) * alpha;
    for turret in &mut visual.turrets {
        if let Some(old) = previous.turrets.iter().find(|old| old.part == turret.part) {
            let delta = (turret.yaw_rad - old.yaw_rad + std::f64::consts::PI)
                .rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            turret.yaw_rad = old.yaw_rad + delta * alpha;
            turret.pitch_rad = old.pitch_rad + (turret.pitch_rad - old.pitch_rad) * alpha;
        }
    }
    for engine in &mut visual.engines {
        if let Some(old) = previous.engines.iter().find(|old| old.part == engine.part) {
            engine.thrust_fraction =
                old.thrust_fraction + (engine.thrust_fraction - old.thrust_fraction) * alpha;
            for axis in 0..3 {
                engine.thrust_n[axis] =
                    old.thrust_n[axis] + (engine.thrust_n[axis] - old.thrust_n[axis]) * alpha;
            }
        }
    }
    if let (Some(old), Some(shield)) = (&previous.shield, &mut visual.shield) {
        shield.temperature_k =
            old.temperature_k + (shield.temperature_k - old.temperature_k) * alpha;
        shield.coverage = old.coverage + (shield.coverage - old.coverage) * alpha;
    }
    visual
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_interpolation_wraps_turret_yaw() {
        let visual = |yaw| ShipVisual {
            slip_readiness: 0.0,
            engines: Vec::new(),
            shield: None,
            turrets: vec![TurretVisual {
                part: 99,
                yaw_rad: yaw,
                pitch_rad: 0.4,
            }],
        };
        let interpolated = interpolate_visual(
            &visual(179_f64.to_radians()),
            &visual(-179_f64.to_radians()),
            0.5,
        );
        assert!((interpolated.turrets[0].yaw_rad.abs() - std::f64::consts::PI).abs() < 1e-12);
        assert_eq!(interpolated.turrets[0].pitch_rad, 0.4);
    }
}
