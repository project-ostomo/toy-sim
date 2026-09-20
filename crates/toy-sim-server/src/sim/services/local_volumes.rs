use super::*;

#[cfg(test)]
mod flight_tests;

impl FusedScan {
    pub(super) fn orrery(&self, reference: GalacticPosition) -> Result<Vec<LocalObstacle>> {
        let Some(universe) = &self.universe else {
            return Ok(Vec::new());
        };
        let registry = &universe.registry.universe;
        let Some(system) = registry.index.nearest(reference) else {
            return Ok(Vec::new());
        };
        let solver = &registry.systems[system].solver;
        let epoch = self.prediction_epoch(0.)?;
        let mut bodies = Vec::new();
        for body in solver.iter() {
            if matches!(
                body.class_params,
                super::super::orrery::BodyClass::Barycenter
            ) {
                continue;
            }
            let Some(position) = solver.solve_position(&body.name, epoch) else {
                continue;
            };
            bodies.push(LocalObstacle {
                reference: Target::Destination(Destination::Relative {
                    reference: Reference::Celestial(super::super::registry::identity(&body.name)),
                    offset: GalacticPosition::ZERO,
                    axes: Axes::Galactic,
                }),
                pose: Pose {
                    position,
                    velocity: solver
                        .solve_velocity(&body.name, epoch)
                        .unwrap_or_default()
                        .to_array(),
                    ..Default::default()
                },
                radius_m: body.radius,
                slip_exclusion_m: (2. * super::super::physics::GRAVITATIONAL_CONSTANT * body.mass
                    / SLIP_CURVATURE_LIMIT)
                    .cbrt(),
            });
        }
        let system_id = super::super::registry::system_identity(&solver.name);
        for beacon in self.beacons.values().chain(self.orbital.beacons.values()) {
            if beacon.system != system_id {
                continue;
            }
            bodies.push(LocalObstacle {
                reference: Target::Destination(Destination::Beacon(beacon.beacon.entity)),
                pose: self.orbital_pose(&beacon.beacon.pose, beacon.orbit.as_deref()),
                radius_m: beacon.beacon.radius_m,
                slip_exclusion_m: beacon.beacon.exclusion_m,
            });
        }
        ensure!(
            bodies.len() <= toy_sim_model::local_space::MAX_LOCAL_OBSTACLES,
            "orrery exceeds body budget"
        );
        Ok(bodies)
    }
}
