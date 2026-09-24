use super::*;

impl Executor {
    pub(super) fn search(
        &mut self,
        pose: &Pose,
        system: Id,
        station: Option<&Beacon>,
        entry: &ItineraryEntry,
        available: f64,
    ) -> Result<(Option<Plan>, bool), i32> {
        let mut assistance = vec![None];
        let mut after = None;
        loop {
            let ProgramReply::Beacons(page) = query(&ProgramQuery::Beacons { after, limit: 64 })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            for beacon in &page {
                if beacon.system == Some(system) {
                    assistance.push(Some(beacon.entity));
                }
            }
            if page.len() < 64 {
                break;
            }
            after = page.last().map(|beacon| beacon.entity);
        }
        let space = self.navigation_environment(pose)?;
        let fuel_limit =
            available.min((entry.fuel_allowance_kg - self.status.spent_exotic_fuel_kg).max(0.));
        let max_loss_ppm = RiskBudget {
            max_log_loss: slip::log_loss_from_ppm(entry.max_loss_ppm),
            spent_log_loss: slip::log_loss_from_ppm(self.status.spent_loss_ppm),
        }
        .remaining_ppm();
        let mut best: Option<Plan> = None;
        let mut has_capture = false;
        let mut affordable_capture = false;
        let windows = [
            0.,
            30.,
            120.,
            self.wait_horizon * 0.25,
            self.wait_horizon * 0.5,
            self.wait_horizon,
        ];

        for wait in windows {
            let ProgramReply::Orrery(bodies) = query(&ProgramQuery::OrrerySystem {
                system,
                after_seconds: wait,
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            for body in bodies {
                let Target::Destination(Destination::Relative {
                    reference: Reference::Celestial(reference),
                    ..
                }) = body.reference
                else {
                    continue;
                };
                let radius = body.slip_exclusion_m;
                if radius <= body.radius_m + self.own_radius {
                    continue;
                }
                has_capture = true;
                let minimum_distance =
                    (body.pose.position.relative_to(pose.position).length() - radius).max(0.);
                affordable_capture |=
                    slip::exotic_fuel_kg(self.mass, minimum_distance / slip::LY_M) <= fuel_limit;
                let destination_ref = Destination::Relative {
                    reference: Reference::Celestial(reference),
                    offset: GalacticPosition::ZERO,
                    axes: Axes::Galactic,
                };
                let direction = body.pose.position.relative_to(pose.position);
                let toward = station.map_or(DVec3::ZERO, |station| {
                    station.pose.position.relative_to(body.pose.position)
                });
                let offsets = planner::aim_offsets(direction, toward, radius);
                let departure_position = crate::local_guidance::outside_exclusions(
                    pose.position,
                    direction,
                    &space,
                    self.own_radius,
                );
                // Evaluate coasting into a future window and an active escape.
                let mut departures = vec![(None, wait)];
                if let Some(position) = departure_position {
                    if position.relative_to(pose.position).length() > 1. {
                        let velocity = space
                            .obstacles
                            .iter()
                            .filter(|obstacle| obstacle.slip_exclusion_m > 0.)
                            .min_by(|a, b| {
                                let gap = |obstacle: &LocalObstacle| {
                                    (position.relative_to(obstacle.pose.position).length()
                                        - obstacle.slip_exclusion_m)
                                        .abs()
                                };
                                gap(a).total_cmp(&gap(b))
                            })
                            .map_or(pose.velocity, |obstacle| obstacle.pose.velocity);
                        let maneuver = Pose {
                            position,
                            velocity,
                            ..pose.clone()
                        };
                        let seconds = self.transfer_estimate(&contact(pose, &maneuver)).0;
                        if seconds.is_finite() && seconds < MAX_PREDICTION_SECONDS * 0.5 {
                            departures.push((Some(maneuver), wait.max(seconds)));
                        }
                    }
                }
                for (maneuver, clearing_s) in departures {
                    for &navigation_beacon in &assistance {
                        for offset in offsets {
                            // Compare a coast arrival and a paid match. The
                            // latter may be fuel or distance limited.
                            {
                                let mut departure_s = clearing_s;
                                let mut duration_s = slip::flight_seconds(
                                    direction.length(),
                                    navigation_beacon.is_some(),
                                );
                                let mut candidate = None;
                                for _ in 0..3 {
                                    let arrival_s = departure_s + duration_s;
                                    if arrival_s > MAX_PREDICTION_SECONDS {
                                        break;
                                    }
                                    let future_body = resolve_at(&destination_ref, arrival_s)?;
                                    let future_station = station
                                        .map(|station| {
                                            resolve_at(
                                                &Destination::Beacon(station.entity),
                                                arrival_s,
                                            )
                                        })
                                        .transpose()?;
                                    let origin = maneuver.as_ref().map_or_else(
                                        || {
                                            pose.position.offset_by(
                                                DVec3::from_array(pose.velocity) * departure_s,
                                            )
                                        },
                                        |maneuver| {
                                            maneuver.position.offset_by(
                                                DVec3::from_array(maneuver.velocity) * departure_s,
                                            )
                                        },
                                    );
                                    let offset =
                                        future_station.as_ref().map_or(offset, |station| {
                                            let ray = future_body
                                                .position
                                                .relative_to(origin)
                                                .normalize_or_zero();
                                            let toward =
                                                station.position.relative_to(future_body.position);
                                            let tangent = toward - ray * toward.dot(ray);
                                            tangent
                                                .try_normalize()
                                                .map_or(offset, |side| side * offset.length())
                                        });
                                    let destination = future_body.position.offset_by(offset);
                                    let distance = destination.relative_to(origin).length();
                                    let loss = planner::loss_ppm(
                                        radius,
                                        offset.length(),
                                        distance,
                                        navigation_beacon.is_some(),
                                    );
                                    if loss > max_loss_ppm {
                                        break;
                                    }
                                    let distance_ly = distance / slip::LY_M;
                                    let departure_velocity = DVec3::from_array(
                                        maneuver
                                            .as_ref()
                                            .map_or(pose.velocity, |maneuver| maneuver.velocity),
                                    );
                                    let desired = future_station.as_ref().unwrap_or(&future_body);
                                    let desired_delta =
                                        DVec3::from_array(desired.velocity) - departure_velocity;
                                    let max_delta = slip::earned_delta_v_m_s(
                                        desired_delta.length(),
                                        ((distance - radius).max(0.)) / slip::LY_M,
                                    );
                                    let delta = affordable_delta(
                                        self.mass,
                                        distance_ly,
                                        max_delta,
                                        fuel_limit,
                                    );
                                    let arrival_velocity = (delta > 0.).then(|| {
                                        (departure_velocity
                                            + desired_delta.normalize_or_zero() * delta)
                                            .to_array()
                                    });
                                    let fuel = slip::transit_fuel_kg(self.mass, distance_ly, delta);
                                    if fuel > fuel_limit {
                                        break;
                                    }
                                    let probe = SlipProbe {
                                        origin,
                                        destination,
                                        departure_after_seconds: departure_s,
                                        arrival_after_seconds: arrival_s,
                                        navigation_beacon,
                                        arrival_velocity,
                                    };
                                    let mut probes = vec![SlipProbe {
                                        arrival_velocity: None,
                                        ..probe.clone()
                                    }];
                                    if arrival_velocity.is_some() {
                                        probes.push(probe);
                                    }
                                    // Compare both velocity choices in one service
                                    // admission. Two probes fit the host's gas slice.
                                    let ProgramReply::SlipEligibilityBatch(results) =
                                        query(&ProgramQuery::SlipEligibilityBatch(probes.clone()))?
                                    else {
                                        return Err(abi::ERR_ARGUMENT);
                                    };
                                    let Some(result) = results.first() else {
                                        return Err(abi::ERR_ARGUMENT);
                                    };
                                    if result.error.is_some() {
                                        break;
                                    }
                                    let next_departure =
                                        planner::departure_delay(result.preparation_s, clearing_s);
                                    let converged = (next_departure - departure_s).abs() < 0.5
                                        && (result.duration_s - duration_s).abs() < 0.5;
                                    departure_s = next_departure;
                                    duration_s = result.duration_s;
                                    if !converged {
                                        continue;
                                    }
                                    if !result.ready {
                                        break;
                                    }
                                    let approach =
                                        destination.relative_to(origin).normalize_or_zero();
                                    let capture = destination.offset_by(
                                        -approach
                                            * (radius * radius - offset.length_squared())
                                                .max(0.)
                                                .sqrt(),
                                    );
                                    for (probe, result) in probes.iter().zip(&results) {
                                        if !result.ready || result.error.is_some() {
                                            continue;
                                        }
                                        let arrival_velocity = probe.arrival_velocity;
                                        let delta = arrival_velocity.map_or(0., |velocity| {
                                            (DVec3::from_array(velocity) - departure_velocity)
                                                .length()
                                        });
                                        let fuel =
                                            slip::transit_fuel_kg(self.mass, distance_ly, delta);
                                        let arrival_pose = Pose {
                                            position: capture,
                                            velocity: arrival_velocity
                                                .unwrap_or(departure_velocity.to_array()),
                                            ..pose.clone()
                                        };
                                        let matching_s = (DVec3::from_array(arrival_pose.velocity)
                                            - DVec3::from_array(future_body.velocity))
                                        .length()
                                            / self.acceleration;
                                        let transfer =
                                            future_station.as_ref().map_or(matching_s, |target| {
                                                self.transfer_estimate(&contact(
                                                    &arrival_pose,
                                                    target,
                                                ))
                                                .0
                                            });
                                        let total_s = departure_s + duration_s + transfer;
                                        let eta_s =
                                            if matches!(entry.directive, Directive::DockAt(_)) {
                                                total_s
                                            } else {
                                                departure_s + duration_s
                                            };
                                        // Equal arrival times favor lower exotic use.
                                        let score = total_s + fuel * 0.001;
                                        let option = Plan {
                                            epoch: self.tick,
                                            body: reference,
                                            offset,
                                            destination,
                                            departure: self.tick.saturating_add(
                                                (departure_s * TICK_RATE_HZ).ceil() as u64,
                                            ),
                                            arrival: self.tick.saturating_add(
                                                (eta_s * TICK_RATE_HZ).ceil() as u64,
                                            ),
                                            beacon: navigation_beacon,
                                            arrival_velocity,
                                            delta_v: delta,
                                            loss_ppm: loss,
                                            fuel_kg: fuel,
                                            maneuver: maneuver.clone(),
                                            score,
                                            mass: self.mass,
                                            fuel_available: available,
                                            capture_radius: radius,
                                            max_loss_ppm,
                                            fuel_limit,
                                            station: station.map(|station| station.entity),
                                        };
                                        if candidate
                                            .as_ref()
                                            .is_none_or(|best: &Plan| option.score < best.score)
                                        {
                                            candidate = Some(option);
                                        }
                                    }
                                    break;
                                }
                                if let Some(candidate) = candidate {
                                    if best
                                        .as_ref()
                                        .is_none_or(|best| candidate.score < best.score)
                                    {
                                        best = Some(candidate);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok((best, has_capture && !affordable_capture))
    }

    pub(super) fn track(&mut self, pose: &Pose) -> Result<bool, i32> {
        let Some(plan) = self.plan.as_ref() else {
            return Ok(false);
        };
        if let Some(id) = plan.beacon {
            if beacon(id)?.is_none() {
                return Ok(false);
            }
        }
        let remaining = plan.departure.saturating_sub(self.tick) as f64 * TICK_SECONDS;
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
        let plan = self.plan.as_mut().unwrap();
        plan.destination = target.position;
        plan.offset = offset;
        plan.arrival_velocity = arrival_velocity;
        plan.delta_v = delta;
        plan.fuel_kg = fuel;
        plan.departure = plan.departure.max(
            self.tick
                .saturating_add((result.preparation_s * TICK_RATE_HZ).ceil() as u64),
        );
        plan.loss_ppm = loss;
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
                self.status.markers.push(PlanMarker {
                    position: target.position,
                    label: "Departure clearance".into(),
                });
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
