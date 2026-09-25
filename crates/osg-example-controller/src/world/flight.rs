use super::*;

impl Executor {
    pub(super) fn search(
        &mut self,
        pose: &Pose,
        system: Id,
        station: Option<&Beacon>,
        entry: &ItineraryEntry,
        fuel_limit: f64,
        max_loss_ppm: f64,
    ) -> Result<Option<Plan>, i32> {
        if self.assistance.is_empty() {
            self.assistance.push(None);
        }
        if !self.beacons_loaded {
            let ProgramReply::Beacons(page) = query(&ProgramQuery::Beacons {
                after: self.beacon_after,
                limit: 64,
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            self.assistance.extend(
                page.iter()
                    .filter(|beacon| beacon.system == Some(system))
                    .map(|beacon| Some(beacon.entity)),
            );
            self.beacon_after = page.last().map(|beacon| beacon.entity);
            self.beacons_loaded = page.len() < 64;
            if !self.beacons_loaded {
                return Ok(None);
            }
        }

        let ProgramReply::Orrery(mut bodies) = query(&ProgramQuery::OrrerySystem {
            system,
            after_seconds: 0.,
        })?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        if bodies.is_empty() {
            return Ok(None);
        }
        // Large capture spheres are most likely to satisfy the risk limit.
        bodies.sort_by(|a, b| b.slip_exclusion_m.total_cmp(&a.slip_exclusion_m));
        let cursor = self.search_cursor;
        self.search_cursor = self.search_cursor.wrapping_add(1);
        // Try direct departure and clearance together for each body before
        // exploring displaced aim points and later departure windows.
        let body = &bodies[(cursor / 2) % bodies.len()];
        let Target::Destination(Destination::Relative {
            reference: Reference::Celestial(reference),
            ..
        }) = &body.reference
        else {
            return Ok(None);
        };
        let radius = body.slip_exclusion_m;
        if radius <= body.radius_m + self.own_radius {
            return Ok(None);
        }
        let clearance = cursor % 2 == 1;
        let variant = cursor / (2 * bodies.len());
        let beacon = self.assistance[variant % self.assistance.len()];
        let variant = variant / self.assistance.len();
        let wait = [0., 30., 120., self.wait_horizon][(variant / 4) % 4];
        if cursor > 0 && cursor % (bodies.len() * self.assistance.len() * 32) == 0 {
            self.wait_horizon = (self.wait_horizon * 2.).min(MAX_PREDICTION_SECONDS * 0.5);
            // Periodically refresh assistance so new installations become usable.
            self.beacons_loaded = false;
            self.beacon_after = None;
            self.assistance.clear();
        }
        let direction = body.pose.position.relative_to(pose.position);
        let space = self.navigation_environment(pose)?;
        let maneuver = if clearance {
            crate::local_guidance::outside_exclusions(
                pose.position,
                direction,
                &space,
                self.own_radius,
            )
            .filter(|target| target.position.relative_to(pose.position).length() > 1.)
        } else {
            None
        };
        let clearing = maneuver.as_ref().map_or(wait, |target| {
            wait.max(self.transfer_estimate(&contact(pose, target)).0)
        });
        let mut departure = clearing;
        let mut duration = slip::flight_seconds(direction.length(), beacon.is_some());
        let destination_ref = Destination::Relative {
            reference: Reference::Celestial(*reference),
            offset: GalacticPosition::ZERO,
            axes: Axes::Galactic,
        };
        // A single candidate, with a bounded correction for charging and the
        // destination's orbital motion. Later updates refine other candidates.
        for _ in 0..3 {
            let arrival = departure + duration;
            if arrival > MAX_PREDICTION_SECONDS {
                return Ok(None);
            }
            let body_pose = resolve_at(&destination_ref, arrival)?;
            let station_pose = station
                .map(|station| resolve_at(&Destination::Beacon(station.entity), arrival))
                .transpose()?;
            let departure_pose = maneuver.as_ref().unwrap_or(pose);
            let origin = departure_pose
                .position
                .offset_by(DVec3::from_array(departure_pose.velocity) * departure);
            if space.departure_body(origin, departure) == Some(&body.reference) {
                return Ok(None);
            }
            let toward = station_pose.as_ref().map_or(DVec3::ZERO, |station| {
                station.position.relative_to(body_pose.position)
            });
            let offset =
                planner::aim_offsets(body_pose.position.relative_to(origin), toward, radius)
                    [variant % 4];
            let destination = body_pose.position.offset_by(offset);
            let distance = destination.relative_to(origin).length();
            let loss = planner::loss_ppm(
                radius - self.own_radius,
                offset.length(),
                distance,
                beacon.is_some(),
            );
            if loss > max_loss_ppm {
                return Ok(None);
            }

            let desired = station_pose.as_ref().unwrap_or(&body_pose);
            let velocity = DVec3::from_array(departure_pose.velocity);
            let matching = DVec3::from_array(desired.velocity) - velocity;
            let delta = if (variant / 16) % 2 == 0 {
                0.
            } else {
                affordable_delta(
                    self.mass,
                    distance / slip::LY_M,
                    slip::earned_delta_v_m_s(
                        matching.length(),
                        (distance - radius).max(0.) / slip::LY_M,
                    ),
                    fuel_limit,
                )
            };
            let arrival_velocity =
                (delta > 0.).then(|| (velocity + matching.normalize_or_zero() * delta).to_array());
            let fuel = slip::transit_fuel_kg(self.mass, distance / slip::LY_M, delta);
            if fuel > fuel_limit {
                return Ok(None);
            }
            let ProgramReply::SlipEligibilityBatch(results) =
                query(&ProgramQuery::SlipEligibilityBatch(vec![SlipProbe {
                    origin,
                    destination,
                    departure_after_seconds: departure,
                    arrival_after_seconds: arrival,
                    navigation_beacon: beacon,
                    arrival_velocity,
                }]))?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            let Some(result) = results.first() else {
                return Ok(None);
            };
            if result.error.is_some() {
                return Ok(None);
            }
            let next = planner::departure_delay(result.preparation_s, clearing);
            if (next - departure).abs() >= 0.5 || (result.duration_s - duration).abs() >= 0.5 {
                departure = next;
                duration = result.duration_s;
                continue;
            }
            if !result.ready {
                return Ok(None);
            }
            let capture = destination.offset_by(
                -destination.relative_to(origin).normalize_or_zero()
                    * (radius * radius - offset.length_squared()).max(0.).sqrt(),
            );
            let arrival_pose = Pose {
                position: capture,
                velocity: arrival_velocity.unwrap_or(departure_pose.velocity),
                ..pose.clone()
            };
            let transfer = station_pose.as_ref().map_or(0., |target| {
                self.transfer_estimate(&contact(&arrival_pose, target)).0
            });
            let total = departure + duration + transfer;
            let eta = if matches!(entry.directive, Directive::DockAt(_)) {
                total
            } else {
                departure + duration
            };
            return Ok(Some(Plan {
                epoch: self.tick,
                body: *reference,
                offset,
                destination,
                departure: self.tick + (departure * TICK_RATE_HZ).ceil() as u64,
                arrival: self.tick + (eta * TICK_RATE_HZ).ceil() as u64,
                beacon,
                arrival_velocity,
                delta_v: delta,
                loss_ppm: loss,
                fuel_kg: fuel,
                maneuver,
                score: total,
                capture_radius: radius,
                max_loss_ppm,
                fuel_limit,
                station: station.map(|station| station.entity),
            }));
        }
        Ok(None)
    }

    pub(super) fn track(&mut self, pose: &Pose) -> Result<bool, i32> {
        let Some(mut plan) = self.plan.clone() else {
            return Ok(false);
        };
        if let Some(id) = plan.beacon {
            if beacon(id)?.is_none() {
                return Ok(false);
            }
        }
        let remaining = if plan.maneuver.is_some() {
            let space = self.navigation_environment(pose)?;
            let Some(target) = crate::local_guidance::outside_exclusions(
                pose.position,
                plan.destination.relative_to(pose.position),
                &space,
                self.own_radius,
            ) else {
                return Ok(false);
            };
            let elapsed = self.tick.saturating_sub(plan.epoch) as f64 * TICK_SECONDS;
            plan.score = (plan.score - elapsed).max(0.);
            plan.epoch = self.tick;
            plan.maneuver =
                (target.position.relative_to(pose.position).length() > 1.).then_some(target);
            plan.maneuver.as_ref().map_or(0., |target| {
                self.transfer_estimate(&contact(pose, target)).0
            })
        } else {
            plan.departure.saturating_sub(self.tick) as f64 * TICK_SECONDS
        };
        let origin = plan.maneuver.as_ref().map_or_else(
            || {
                pose.position
                    .offset_by(DVec3::from_array(pose.velocity) * remaining)
            },
            |maneuver| {
                let elapsed = self.tick.saturating_sub(plan.epoch) as f64 * TICK_SECONDS;
                maneuver
                    .position
                    .offset_by(DVec3::from_array(maneuver.velocity) * (elapsed + remaining))
            },
        );
        let duration = slip::flight_seconds(
            plan.destination.relative_to(origin).length(),
            plan.beacon.is_some(),
        );
        let space = self.navigation_environment(pose)?;
        if space.departure_body(origin, remaining)
            == Some(&Target::Destination(Destination::Relative {
                reference: Reference::Celestial(plan.body),
                offset: GalacticPosition::ZERO,
                axes: Axes::Galactic,
            }))
        {
            return Ok(false);
        }
        let mut target = resolve_at(
            &Destination::Relative {
                reference: Reference::Celestial(plan.body),
                offset: GalacticPosition::ZERO,
                axes: Axes::Galactic,
            },
            remaining + duration,
        )?;
        let offset = if let Some(station) = plan.station {
            let station = resolve_at(&Destination::Beacon(station), remaining + duration)?;
            let ray = target.position.relative_to(origin).normalize_or_zero();
            let toward = station.position.relative_to(target.position);
            let tangent = toward - ray * toward.dot(ray);
            tangent
                .try_normalize()
                .map_or(plan.offset, |side| side * plan.offset.length())
        } else {
            plan.offset
        };
        target.position = target.position.offset_by(offset);
        let distance = target.position.relative_to(origin).length();
        let loss = planner::loss_ppm(
            plan.capture_radius,
            plan.offset.length(),
            distance,
            plan.beacon.is_some(),
        );
        let departure_velocity = DVec3::from_array(
            plan.maneuver
                .as_ref()
                .map_or(pose.velocity, |maneuver| maneuver.velocity),
        );
        let requested = plan.arrival_velocity.map_or(DVec3::ZERO, |velocity| {
            DVec3::from_array(velocity) - departure_velocity
        });
        let delta = slip::earned_delta_v_m_s(
            requested.length(),
            (distance - plan.capture_radius).max(0.) / slip::LY_M,
        );
        let arrival_velocity = plan
            .arrival_velocity
            .map(|_| (departure_velocity + requested.normalize_or_zero() * delta).to_array());
        // A large departure-velocity change invalidates the rendezvous score.
        // Minor drift can be reflected directly in the current plan.
        if (requested.length() - delta) > (plan.delta_v * 0.1).max(1.)
            || planner::changed_materially(plan.delta_v, delta)
        {
            return Ok(false);
        }
        let fuel = slip::transit_fuel_kg(self.mass, distance / slip::LY_M, delta);
        if loss > plan.max_loss_ppm || fuel > plan.fuel_limit {
            return Ok(false);
        }
        let ProgramReply::SlipEligibilityBatch(results) =
            query(&ProgramQuery::SlipEligibilityBatch(vec![SlipProbe {
                origin,
                destination: target.position,
                departure_after_seconds: remaining,
                arrival_after_seconds: remaining + duration,
                navigation_beacon: plan.beacon,
                arrival_velocity,
            }]))?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        let Some(result) = results.first() else {
            return Ok(false);
        };
        if !result.ready || result.error.is_some() {
            return Ok(false);
        }
        plan.destination = target.position;
        plan.offset = offset;
        plan.arrival_velocity = arrival_velocity;
        plan.delta_v = delta;
        plan.fuel_kg = fuel;
        plan.departure = self
            .tick
            .saturating_add((remaining.max(result.preparation_s) * TICK_RATE_HZ).ceil() as u64);
        plan.loss_ppm = loss;
        self.plan = Some(plan);
        Ok(true)
    }

    pub(super) fn fly_plan(
        &mut self,
        pose: &Pose,
        slip_axis: [f64; 3],
        plan: Plan,
    ) -> Result<Option<abi::Contact>, i32> {
        self.status.capture_body = Some(plan.body);
        self.status.aim_offset_m = Some(plan.offset.to_array());
        self.status.departure_tick = Some(plan.departure);
        self.status.estimated_arrival_tick = Some(plan.arrival);
        self.status.planned_delta_v_m_s = plan.delta_v;
        self.status.planned_loss_ppm = plan.loss_ppm;
        self.status.planned_exotic_fuel_kg = plan.fuel_kg;
        self.status.summary = format!(
            "Capture {} / {:.3} kg exotic / {:.1} km/s arrival change",
            plan.body.body,
            plan.fuel_kg,
            plan.delta_v / 1000.,
        );
        self.status.markers = vec![PlanMarker {
            position: plan.destination,
            label: "Planned capture".into(),
        }];
        self.physical(ProgramAction::Slip {
            destination: plan.destination,
            navigation_beacon: plan.beacon,
            arrival_velocity: plan.arrival_velocity,
            not_before_tick: Some(plan.departure),
        })?;
        if let Some(target) = &plan.maneuver {
            let mut target = target.clone();
            let elapsed = self.tick.saturating_sub(plan.epoch) as f64 * TICK_SECONDS;
            target.position = target
                .position
                .offset_by(DVec3::from_array(target.velocity) * elapsed);
            let relative = contact(pose, &target);
            if DVec3::from_array(relative.position_m).length() > 20. {
                self.status.phase = FirmwarePhase::Maneuvering;
                self.status
                    .summary
                    .push_str("; clearing departure while charging");
                self.status.markers.insert(
                    0,
                    PlanMarker {
                        position: target.position,
                        label: "Departure clearance".into(),
                    },
                );
                self.publish()?;
                return self.steer(pose, &target, None).map(Some);
            }
        }
        let direction = plan
            .destination
            .relative_to(pose.position)
            .normalize_or_zero();
        self.aim_attitude = Some(
            crate::attitude::point(
                DQuat::from_array(pose.rotation),
                DVec3::from_array(slip_axis),
                direction,
            )
            .to_array(),
        );
        self.status.phase = if plan.departure > self.tick.saturating_add(10) {
            FirmwarePhase::Waiting {
                until: Some(plan.departure),
                why: "Charging while waiting for the selected departure window".into(),
            }
        } else {
            FirmwarePhase::Charging
        };
        self.publish()?;
        Ok(None)
    }
}

fn affordable_delta(mass: f64, distance_ly: f64, desired: f64, fuel: f64) -> f64 {
    if slip::transit_fuel_kg(mass, distance_ly, desired) <= fuel {
        return desired;
    }
    let mut low = 0.;
    let mut high = desired;
    for _ in 0..48 {
        let middle = (low + high) * 0.5;
        if slip::transit_fuel_kg(mass, distance_ly, middle) <= fuel {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}
