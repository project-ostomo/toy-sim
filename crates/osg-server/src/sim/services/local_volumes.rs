use super::*;

impl ShipScan<'_> {
    pub(super) fn orrery_system(
        &self,
        system: Id,
        after_seconds: f64,
    ) -> Result<Vec<LocalObstacle>> {
        let universe = self
            .universe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("universe unavailable"))?;
        let definition = universe.registry.universe.resolve(system.0)?;
        let epoch = self.prediction_epoch(after_seconds)?;
        let solver = &definition.solver;
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
                    reference: Reference::Celestial(super::super::registry::model_reference(
                        definition.body_id(&body.name).expect("body identity"),
                    )),
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
                slip_exclusion_m: slip::exclusion_radius_m(body.mass),
                hill_radius_m: crate::sim::location::HillSphere::radius_for_body(&definition, body)
                    .unwrap_or(0.),
            });
        }
        ensure!(
            bodies.len() <= osg_model::local_space::MAX_LOCAL_OBSTACLES,
            "orrery exceeds body budget"
        );
        Ok(bodies)
    }

    pub(super) fn orrery(&self, reference: GalacticPosition) -> Result<Vec<LocalObstacle>> {
        let Some(universe) = &self.universe else {
            return Ok(Vec::new());
        };
        let registry = &universe.registry.universe;
        let epoch = self.prediction_epoch(0.)?;
        let mut bodies = Vec::new();
        for system in registry.containing_segment(reference, DVec3::ZERO) {
            let definition = registry.resolve_index(system)?;
            let solver = &definition.solver;
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
                        reference: Reference::Celestial(super::super::registry::model_reference(
                            definition.body_id(&body.name).expect("body identity"),
                        )),
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
                    slip_exclusion_m: osg_model::travel::slip::exclusion_radius_m(body.mass),
                    hill_radius_m: crate::sim::location::HillSphere::radius_for_body(
                        &definition,
                        body,
                    )
                    .unwrap_or(0.),
                });
            }
        }
        ensure!(
            bodies.len() <= osg_model::local_space::MAX_LOCAL_OBSTACLES,
            "orrery exceeds body budget"
        );
        Ok(bodies)
    }
}
