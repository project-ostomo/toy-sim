use super::*;
use toy_sim_model::local_space::{MAX_LOCAL_OBSTACLES, QUERY_WORK};

#[cfg(test)]
mod flight_tests;

const EPHEMERIS_WORK: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Public(Id),
    Contact(GroupId, TrackId),
}

struct Budget {
    remaining: usize,
    truncated: bool,
}

impl Budget {
    fn charge(&mut self, work: usize) -> bool {
        if work > self.remaining {
            self.truncated = true;
            false
        } else {
            self.remaining -= work;
            true
        }
    }

    fn candidates(
        &mut self,
        index: &ApertureIndex,
        centre: GalacticPosition,
        range_m: f64,
        after_seconds: f64,
    ) -> Vec<u32> {
        let tidal_reach = tidal_radius(index.mass);
        let radius = range_m + index.max_speed * after_seconds + tidal_reach;
        let mut cursor = index.spatial.range_cursor(centre, radius, true);
        let mut candidates = Vec::new();

        loop {
            let traversal_work = self.remaining.saturating_sub(EPHEMERIS_WORK);
            let batch = index.spatial.advance_range(&mut cursor, traversal_work, 1);
            self.remaining = self
                .remaining
                .checked_sub(batch.stats.work())
                .expect("spatial cursor respected its work allowance");

            if batch.invalidated || (!batch.complete && batch.stats.work() == 0) {
                self.truncated = true;
                return candidates;
            }

            for id in batch.ids {
                assert!(self.charge(EPHEMERIS_WORK), "candidate work was reserved");
                candidates.push(id);
            }

            if batch.complete {
                return candidates;
            }
            if traversal_work == 0 || candidates.len() == MAX_LOCAL_OBSTACLES * 2 {
                self.truncated = true;
                return candidates;
            }
        }
    }
}

fn tidal_radius(mass: f64) -> f64 {
    (2.0 * super::super::physics::GRAVITATIONAL_CONSTANT * mass / SLIP_CURVATURE_LIMIT).cbrt()
}

fn reference_target(reference: &Reference) -> Target {
    Target::Destination(match reference {
        Reference::Beacon(id) => Destination::Beacon(*id),
        Reference::Celestial(_) => Destination::Relative {
            reference: reference.clone(),
            offset: GalacticPosition::ZERO,
            axes: Axes::Galactic,
        },
    })
}

fn retain_near(
    volumes: &mut BTreeMap<Key, LocalObstacle>,
    key: Key,
    obstacle: LocalObstacle,
    centre: GalacticPosition,
    range_m: f64,
) {
    let radius = obstacle.radius_m.max(obstacle.slip_exclusion_m);
    if obstacle.pose.position.relative_to(centre).length() <= radius + range_m {
        volumes.entry(key).or_insert(obstacle);
    }
}

impl FusedScan {
    pub(super) fn local_space(
        &self,
        destination: GalacticPosition,
        range_m: f64,
        after_seconds: f64,
    ) -> Result<LocalSpace> {
        toy_sim_protocol::local_space::validate_request(destination, range_m, after_seconds)?;
        let epoch = self.prediction_epoch(after_seconds)?;
        let published_after_seconds = after_seconds + self.publication_age_seconds();
        let origin = self
            .pose
            .position
            .offset_by(DVec3::from_array(self.pose.velocity) * after_seconds);
        let mut volumes = BTreeMap::new();
        let mut truncated = false;

        for centre in [origin, destination] {
            let mut budget = Budget {
                remaining: QUERY_WORK as usize / 2,
                truncated: false,
            };
            for index in [self.apertures.as_ref(), &self.orbital.apertures] {
                self.local_apertures(
                    index,
                    centre,
                    range_m + self.radius,
                    published_after_seconds,
                    epoch,
                    &mut budget,
                    &mut volumes,
                );
            }
            self.local_celestials(
                centre,
                range_m + self.radius,
                epoch,
                &mut budget,
                &mut volumes,
            );
            for (group, snapshot) in [(self.group, &self.snapshot), (PUBLIC_GROUP, &self.public)] {
                let candidates = snapshot.nearby_volumes(
                    centre,
                    range_m + self.radius,
                    after_seconds,
                    budget.remaining / 2,
                );
                budget.remaining = budget
                    .remaining
                    .checked_sub(candidates.work)
                    .expect("contact query respected its work allowance");
                budget.truncated |= !candidates.complete;
                for track in candidates.tracks {
                    if track.entity == Some(self.own) {
                        continue;
                    }
                    let mut pose = track.pose.clone();
                    pose.position = pose
                        .position
                        .offset_by(DVec3::from_array(pose.velocity) * after_seconds);
                    let obstacle = LocalObstacle {
                        reference: Target::Contact(ContactRef {
                            group,
                            track: track.id,
                        }),
                        pose,
                        radius_m: track.radius_m.unwrap_or(1.0)
                            + 3.0
                                * (track.position_sigma_m
                                    + after_seconds * track.velocity_sigma_m_s),
                        slip_exclusion_m: 0.0,
                    };
                    let key = track
                        .entity
                        .map_or(Key::Contact(group, track.id), Key::Public);
                    retain_near(&mut volumes, key, obstacle, centre, range_m + self.radius);
                }
            }
            truncated |= budget.truncated;
        }

        let clearance = |obstacle: &LocalObstacle| {
            let radius = obstacle.radius_m.max(obstacle.slip_exclusion_m);
            [origin, destination]
                .into_iter()
                .map(|centre| obstacle.pose.position.relative_to(centre).length() - radius)
                .fold(f64::INFINITY, f64::min)
        };
        let mut obstacles = volumes.into_iter().collect::<Vec<_>>();
        obstacles.sort_by(|(a_key, a), (b_key, b)| {
            clearance(a).total_cmp(&clearance(b)).then(a_key.cmp(b_key))
        });
        truncated |= obstacles.len() > MAX_LOCAL_OBSTACLES;
        obstacles.truncate(MAX_LOCAL_OBSTACLES);
        let reply = LocalSpace {
            obstacles: obstacles
                .into_iter()
                .map(|(_, obstacle)| obstacle)
                .collect(),
            truncated,
        };
        toy_sim_protocol::local_space::validate_reply(&reply)?;
        Ok(reply)
    }

    fn local_apertures(
        &self,
        index: &ApertureIndex,
        centre: GalacticPosition,
        range_m: f64,
        after_seconds: f64,
        epoch: hifitime::Epoch,
        budget: &mut Budget,
        volumes: &mut BTreeMap<Key, LocalObstacle>,
    ) {
        for slot in budget.candidates(index, centre, range_m, after_seconds) {
            let aperture = &index.bodies[slot as usize];
            let Some(reference) = &aperture.reference else {
                continue;
            };
            let id = match reference {
                Reference::Celestial(id) | Reference::Beacon(id) => *id,
            };
            if id == self.own {
                continue;
            }
            let pose = if let Reference::Beacon(id) = reference {
                self.beacons
                    .get(id)
                    .or_else(|| self.orbital.beacons.get(id))
                    .map(|beacon| self.pose_at(&beacon.beacon.pose, beacon.orbit.as_deref(), epoch))
            } else {
                self.celestial
                    .get(&id)
                    .map(|pose| self.pose_at(pose, None, epoch))
            };
            let pose = pose.unwrap_or_else(|| Pose {
                position: aperture
                    .position
                    .offset_by(aperture.velocity * after_seconds),
                velocity: aperture.velocity.to_array(),
                ..Default::default()
            });
            let physical_radius = self
                .beacons
                .get(&id)
                .filter(|beacon| beacon.beacon.gate_exit.is_none() && !beacon.bays.is_empty())
                .map_or(aperture.radius, |_| {
                    toy_sim_ships::thermal::shield_radius(aperture.radius + 3.0_f64.sqrt())
                });
            retain_near(
                volumes,
                Key::Public(id),
                LocalObstacle {
                    reference: reference_target(reference),
                    pose,
                    radius_m: physical_radius,
                    slip_exclusion_m: aperture.exclusion.max(tidal_radius(aperture.mass)),
                },
                centre,
                range_m,
            );
        }
    }

    fn local_celestials(
        &self,
        centre: GalacticPosition,
        range_m: f64,
        epoch: hifitime::Epoch,
        budget: &mut Budget,
        volumes: &mut BTreeMap<Key, LocalObstacle>,
    ) {
        let Some(universe) = &self.universe else {
            return;
        };
        // System envelopes include every orbit. A local lookup therefore needs
        // ephemerides only for intersecting public systems, never all 3000.
        for slot in budget.candidates(&universe.index, centre, range_m, 0.0) {
            let system = universe.index.bodies[slot as usize].system.unwrap();
            let solver = &universe.registry.universe.systems[system].solver;
            for body in solver.iter() {
                if !budget.charge(EPHEMERIS_WORK) {
                    return;
                }
                if matches!(
                    body.class_params,
                    super::super::orrery::BodyClass::Barycenter
                ) {
                    continue;
                }
                let Some(position) = solver.solve_position(&body.name, epoch) else {
                    budget.truncated = true;
                    continue;
                };
                let id = super::super::registry::identity(&body.name);
                let pose = Pose {
                    position,
                    velocity: solver
                        .solve_velocity(&body.name, epoch)
                        .unwrap_or_default()
                        .to_array(),
                    ..Default::default()
                };
                retain_near(
                    volumes,
                    Key::Public(id),
                    LocalObstacle {
                        reference: reference_target(&Reference::Celestial(id)),
                        pose,
                        radius_m: body.radius,
                        // Individual tidal limits are necessary exclusions.
                        // SlipEligibility also checks their combined curvature.
                        slip_exclusion_m: tidal_radius(body.mass),
                    },
                    centre,
                    range_m,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::services::tests::{source, track, universe_source};
    use toy_sim_ship_wasm::ScanSource;

    fn aperture(id: Id, position: DVec3, radius: f64, exclusion: f64) -> Aperture {
        Aperture {
            reference: Some(Reference::Beacon(id)),
            entity: Entity::PLACEHOLDER,
            position: GalacticPosition::from_meters(position),
            velocity: DVec3::ZERO,
            radius,
            mass: 0.0,
            exclusion,
            system: None,
            orbit: None,
            envelope_m: 0.0,
        }
    }

    #[test]
    fn enclosing_exclusions_and_remote_endpoint_geometry_are_returned() {
        let mut source = source();
        let containing = Id::new();
        let arrival = Id::new();
        let endpoint = GalacticPosition::from_meters(DVec3::X * 1e9);
        source.apertures = Arc::new(ApertureIndex::build(vec![
            aperture(containing, DVec3::X * 10_000.0, 10.0, 20_000.0),
            aperture(arrival, DVec3::X * 1e9, 100.0, 0.0),
            aperture(Id::new(), DVec3::X * 5e8, 100.0, 0.0),
        ]));
        let reply = source.local_space(endpoint, 10.0, 0.0).unwrap();
        assert!(!reply.truncated);
        assert_eq!(reply.obstacles.len(), 2);
        assert!(reply.obstacles.iter().any(|obstacle| {
            obstacle.reference == Target::Destination(Destination::Beacon(containing))
                && obstacle.slip_exclusion_m == 20_000.0
        }));
        assert!(reply.obstacles.iter().any(|obstacle| {
            obstacle.reference == Target::Destination(Destination::Beacon(arrival))
        }));
    }

    #[test]
    fn previous_tick_publication_is_extrapolated_before_local_lookup() {
        let mut source = source();
        let id = Id::new();
        let mut station = aperture(id, DVec3::ZERO, 100.0, 0.0);
        station.velocity = DVec3::X * 100_000.0;
        source.apertures = Arc::new(ApertureIndex::build(vec![station]));
        source.tick = 1;
        source.epoch += hifitime::Duration::from_seconds(0.1);
        let destination = GalacticPosition::from_meters(DVec3::X * 10_000.0);
        let reply = source.local_space(destination, 10.0, 0.0).unwrap();
        assert!(!reply.truncated);
        assert_eq!(reply.obstacles.len(), 1);
        assert_eq!(reply.obstacles[0].pose.position, destination);
        let resolved = source.pose_at(
            &Pose {
                velocity: [100_000.0, 0.0, 0.0],
                ..Default::default()
            },
            None,
            source.epoch,
        );
        assert_eq!(resolved.position, destination);
    }

    #[test]
    fn only_fused_observations_are_extrapolated_and_capacity_is_recoverable() {
        let mut source = source();
        let mut observed = track(Id::new(), DVec3::X * 1000.0, false);
        observed.pose.velocity = [-100.0, 0.0, 0.0];
        let contact = observed.id;
        Arc::make_mut(&mut source.snapshot).put(observed);

        let mut private = toy_sim_intel::Snapshot::default();
        private.put(track(Id::new(), DVec3::ZERO, false));
        let query = ProgramQuery::LocalSpace {
            destination: GalacticPosition::ZERO,
            range_m: 10.0,
            after_seconds: 10.0,
        };
        assert!(
            source
                .query(query.clone(), false, 0)
                .unwrap_err()
                .is::<toy_sim_ship_wasm::WorldQueryError>()
        );
        let ProgramReply::LocalSpace(reply) = source.query(query, false, 65_536).unwrap() else {
            panic!("local volume reply required");
        };
        assert!(!reply.truncated);
        assert_eq!(reply.obstacles.len(), 1);
        assert_eq!(
            reply.obstacles[0].reference,
            Target::Contact(ContactRef {
                group: source.group,
                track: contact,
            })
        );
        assert_eq!(reply.obstacles[0].pose.position, GalacticPosition::ZERO);
        assert!(source.handles.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn inactive_celestial_volume_uses_public_ephemeris() {
        let source = universe_source();
        assert!(source.celestial.is_empty());
        let universe = &source.universe.as_ref().unwrap().registry.universe;
        let body = universe
            .iter()
            .find(|body| body.name.contains("Neris"))
            .unwrap();
        let position = universe.systems[0]
            .solver
            .solve_position(&body.name, source.epoch)
            .unwrap();
        let reply = source.local_space(position, 0.0, 0.0).unwrap();
        assert!(!reply.truncated);
        let id = crate::sim::registry::identity(&body.name);
        assert!(reply.obstacles.iter().any(|obstacle| {
            obstacle.reference == reference_target(&Reference::Celestial(id))
                && obstacle.radius_m == body.radius
                && obstacle.pose.position == position
        }));
    }

    #[test]
    fn dense_known_geometry_reports_explicit_work_and_result_truncation() {
        let mut source = source();
        source.apertures = Arc::new(ApertureIndex::build(
            (0..10_000)
                .map(|index| aperture(Id((index as u128).to_be_bytes()), DVec3::ZERO, 100.0, 0.0))
                .collect(),
        ));
        let reply = source
            .local_space(GalacticPosition::ZERO, 100.0, 0.0)
            .unwrap();
        assert!(reply.truncated);
        assert_eq!(reply.obstacles.len(), MAX_LOCAL_OBSTACLES);
        assert_eq!(
            reply,
            source
                .local_space(GalacticPosition::ZERO, 100.0, 0.0)
                .unwrap()
        );
        let invalid = ProgramQuery::LocalSpace {
            destination: GalacticPosition::ZERO,
            range_m: f64::INFINITY,
            after_seconds: 0.0,
        };
        assert!(source.query_work(&invalid).is_err());
    }

    #[test]
    fn populated_neris_observations_are_complete_and_include_known_blockers() {
        use crate::sim::{infrastructure::Landmark, npc, session, vessel};

        let mut app = crate::sim::provision(&[Id::new()], None, None).unwrap();
        npc::seed::populate(app.world_mut()).unwrap();
        app.update();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let (station, station_id) = world
            .query::<(Entity, &Identity, &Landmark)>()
            .iter(world)
            .find(|(_, _, landmark)| landmark.name == "Neris Anchorage")
            .map(|(entity, id, _)| (entity, id.0))
            .unwrap();
        let destination = session::ship_pose(world, station).unwrap().position;
        let source = super::super::current_fused_source(world, ship).unwrap();
        let reply = source.local_space(destination, 1000.0, 0.0).unwrap();
        assert!(
            !reply.truncated,
            "ordinary Neris approach needs a complete local observation"
        );
        assert!(
            reply.obstacles.iter().any(|obstacle| {
                obstacle.reference == Target::Destination(Destination::Beacon(station_id))
            }),
            "station absent: tick={}, published={}, public_beacon={}, indexed={}, returned={:?}",
            source.tick,
            source.publication_tick,
            source.beacons.contains_key(&station_id),
            source
                .apertures
                .bodies
                .iter()
                .any(|body| body.reference == Some(Reference::Beacon(station_id))),
            reply.obstacles
        );

        let containing = source
            .orbital
            .beacons
            .iter()
            .filter(|(_, published)| {
                let pose = source.orbital_pose(&published.beacon.pose, published.orbit.as_deref());
                let radius = published.beacon.exclusion_m + source.radius;
                radius > 0.0
                    && [source.pose.position, destination]
                        .iter()
                        .any(|point| pose.position.relative_to(*point).length() <= radius)
            })
            .map(|(&id, _)| id)
            .collect::<Vec<_>>();
        for id in containing {
            assert!(
                reply.obstacles.iter().any(|obstacle| {
                    obstacle.reference == Target::Destination(Destination::Beacon(id))
                        && obstacle.slip_exclusion_m > 0.0
                }),
                "containing public gate exclusion missing"
            );
        }
        let universe = &source.universe.as_ref().unwrap().registry.universe;
        let neris = universe
            .iter()
            .find(|body| body.name == "Helion I Neris")
            .unwrap();
        let solver = &universe
            .systems
            .iter()
            .find(|system| system.solver.get_body(&neris.name).is_some())
            .unwrap()
            .solver;
        let centre = solver.solve_position(&neris.name, source.epoch).unwrap();
        if source.pose.position.relative_to(centre).length() < tidal_radius(neris.mass) {
            let id = crate::sim::registry::identity(&neris.name);
            assert!(
                reply.obstacles.iter().any(|obstacle| {
                    obstacle.reference == reference_target(&Reference::Celestial(id))
                }),
                "containing planetary tidal exclusion missing"
            );
        }
    }

    #[test]
    fn hash_and_contact_work_stay_within_even_tiny_allowances() {
        let index = ApertureIndex::build(vec![aperture(Id::new(), DVec3::ZERO, 100.0, 0.0)]);
        let mut snapshot = toy_sim_intel::Snapshot::default();
        snapshot.put(track(Id::new(), DVec3::ZERO, false));
        for allowance in 0..32 {
            let mut budget = Budget {
                remaining: allowance,
                truncated: false,
            };
            budget.candidates(&index, GalacticPosition::ZERO, 1.0, 0.0);
            assert!(budget.remaining <= allowance);
            let contacts = snapshot.nearby_volumes(GalacticPosition::ZERO, 1.0, 0.0, allowance);
            assert!(contacts.work <= allowance);
        }
    }
}
