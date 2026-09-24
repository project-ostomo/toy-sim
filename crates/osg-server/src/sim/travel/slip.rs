use super::*;
use crate::sim::{hardware::ShipInventory, vessel::ShipCatalogue};
use anyhow::Context;
use hifitime::{Duration, Epoch};
use osg_model::travel::slip as math;
use rand::RngExt;

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SlipDrive {
    pub power_w: f64,
    pub axis: [f64; 3],
    pub preparation: Option<Preparation>,
}

impl Default for SlipDrive {
    fn default() -> Self {
        Self {
            power_w: 500e6,
            axis: [0.0, 0.0, -1.0],
            preparation: None,
        }
    }
}

#[derive(Component, Default)]
pub struct SlipChargingPower(pub f64);

/// Captures remain outside ordinary physics until the completed tick boundary.
#[derive(Component)]
struct PendingArrival {
    transit: Transit,
    capture: Capture,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Preparation {
    pub destination: GalacticPosition,
    pub navigation_beacon: Option<EntityId>,
    pub arrival_velocity: Option<[f64; 3]>,
    pub not_before_tick: Option<u64>,
    pub started: u64,
    pub mass: f64,
    pub work_j: f64,
    pub required_j: f64,
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Transit {
    pub origin: GalacticPosition,
    pub position: GalacticPosition,
    pub destination: GalacticPosition,
    pub departed: u64,
    pub advanced_tick: u64,
    pub direction: [f64; 3],
    pub speed_ly_s: f64,
    pub retained_velocity: [f64; 3],
    pub requested_delta_v: [f64; 3],
    pub departure_mass_kg: f64,
    pub distance_ly: f64,
    pub consumed_fuel_g: u64,
    pub navigation_beacon: Option<EntityId>,
    pub beacon_lost: bool,
    pub nominal_direction: [f64; 3],
    pub variance_m2: f64,
}

impl Transit {
    pub fn arrival_velocity(&self) -> DVec3 {
        let requested = DVec3::from_array(self.requested_delta_v);
        DVec3::from_array(self.retained_velocity)
            + requested.normalize_or_zero()
                * math::earned_delta_v_m_s(requested.length(), self.distance_ly)
    }

    pub fn pose(&self, rotation: [f64; 4]) -> Pose {
        Pose {
            position: self.position,
            rotation,
            velocity: (DVec3::from_array(self.direction) * self.speed_ly_s * math::LY_M).to_array(),
            angular_velocity: [0.0; 3],
        }
    }
}

pub(crate) fn epoch(world: &World) -> Epoch {
    if let Some(time) = world.get_resource::<Time<Fixed>>() {
        return crate::sim::physics::sim_time(time) - Duration::from_seconds(time.delta_secs_f64());
    }
    Epoch::from_mjd_utc(osg_universe::SIMULATION_EPOCH_MJD_UTC)
        + Duration::from_seconds(tick(world) as f64 * osg_model::TICK_SECONDS)
}

fn fuel_index(world: &World) -> Option<usize> {
    world
        .get_resource::<ShipCatalogue>()?
        .0
        .resources
        .iter()
        .position(|resource| resource.id == math::EXOTIC_RESOURCE)
}

fn fuel_grams(world: &World, ship: Entity) -> f64 {
    let Some(index) = fuel_index(world) else {
        return 0.0;
    };
    let grams = world
        .get::<ShipInventory>(ship)
        .and_then(|inventory| inventory.0.quantities.get(index))
        .copied()
        .unwrap_or(0);
    grams as f64
}

fn consume_fuel(world: &mut World, ship: Entity, grams: u64) {
    let Some(index) = fuel_index(world) else {
        return;
    };
    if let Some(mut inventory) = world.get_mut::<ShipInventory>(ship)
        && let Some(quantity) = inventory.0.quantities.get_mut(index)
    {
        let paid = grams.min(*quantity);
        *quantity -= paid;
    }
}

pub fn prepare_slip(
    world: &mut World,
    ship: Entity,
    destination: GalacticPosition,
    navigation_beacon: Option<EntityId>,
    arrival_velocity: Option<[f64; 3]>,
    not_before_tick: Option<u64>,
) -> Result<()> {
    ensure!(
        arrival_velocity.is_none_or(|velocity| DVec3::from_array(velocity).length().is_finite()),
        "arrival velocity must be finite"
    );
    ensure!(active(world, ship), "slip requires a ship in space");
    let pose = ship_pose(world, ship)?;
    ensure!(pose.position != destination, "slip direction is undefined");
    ensure!(fuel_grams(world, ship) > 0.0, "no exotic fuel available");
    if let Some(beacon) = navigation_beacon {
        ensure!(
            crate::sim::infrastructure::authenticated_navigation_beacon(world, ship, beacon),
            "navigation beacon unavailable or unauthorized"
        );
    }
    let mass = world
        .get::<MassProps>(ship)
        .context("ship mass unavailable")?
        .mass;
    let now = tick(world);
    let mut drive = world.get_mut::<SlipDrive>(ship).context("no slipdrive")?;
    if let Some(preparation) = &mut drive.preparation {
        ensure!(
            mass <= preparation.mass * 1.001,
            "ship mass increased during charging"
        );
        preparation.destination = destination;
        preparation.navigation_beacon = navigation_beacon;
        preparation.arrival_velocity = arrival_velocity;
        preparation.not_before_tick = not_before_tick;
        preparation.required_j = math::charging_energy_j(
            preparation.mass,
            destination.relative_to(pose.position).length() / math::LY_M,
        )
        .ceil();
    } else {
        drive.preparation = Some(Preparation {
            destination,
            navigation_beacon,
            arrival_velocity,
            not_before_tick,
            started: now,
            mass,
            work_j: 0.0,
            required_j: math::charging_energy_j(
                mass,
                destination.relative_to(pose.position).length() / math::LY_M,
            )
            .ceil(),
        });
    }
    Ok(())
}

fn gaussian_pair() -> [f64; 2] {
    gaussian_pair_with(&mut rand::rng())
}

fn gaussian_pair_with(rng: &mut impl RngExt) -> [f64; 2] {
    let radius = (-2.0 * (1.0 - rng.random::<f64>()).ln()).sqrt();
    let angle = std::f64::consts::TAU * rng.random::<f64>();
    [radius * angle.cos(), radius * angle.sin()]
}

fn deflected(direction: DVec3, error: [f64; 2]) -> DVec3 {
    let axis = if direction.x.abs() < 0.9 {
        DVec3::X
    } else {
        DVec3::Y
    };
    let across = direction.cross(axis).normalize();
    let up = direction.cross(across);
    (direction + across * error[0] + up * error[1]).normalize()
}

#[derive(Clone, Copy, Debug)]
struct Capture {
    seconds: f64,
    body: Option<CelestialRef>,
    radius_m: f64,
    physical: bool,
}

/// Stable closest-approach calculation; avoids cancellation in the quadratic.
fn sphere_entry(
    offset: DVec3,
    relative_velocity: DVec3,
    radius: f64,
    duration: f64,
) -> Option<f64> {
    if offset.length_squared() <= radius * radius {
        return Some(0.0);
    }
    let speed = relative_velocity.length();
    if speed == 0.0 {
        return None;
    }
    let direction = relative_velocity / speed;
    let along = offset.dot(direction);
    let perpendicular = offset - direction * along;
    let square = radius * radius - perpendicular.length_squared();
    if square < 0.0 {
        return None;
    }
    let entry = (-along - square.sqrt()) / speed;
    (entry >= 0.0 && entry <= duration).then_some(entry)
}

fn moving_entry(
    origin: GalacticPosition,
    velocity: DVec3,
    radius: f64,
    from: f64,
    to: f64,
    center: &impl Fn(f64) -> Option<GalacticPosition>,
    acceleration_bound: f64,
    depth: u8,
) -> Option<f64> {
    let start = center(from)?;
    let end = center(to)?;
    let duration = to - from;
    if duration <= 0.0 {
        return None;
    }
    let relative_velocity = velocity - end.relative_to(start) / duration;
    let offset = origin.offset_by(velocity * from).relative_to(start);
    let deviation_bound = acceleration_bound * duration * duration / 8.0;
    sphere_entry(
        offset,
        relative_velocity,
        radius + deviation_bound,
        duration,
    )?;
    if depth < 30 && deviation_bound > (radius * 1e-10).max(0.001) {
        let midpoint = (from + to) * 0.5;
        return moving_entry(
            origin,
            velocity,
            radius,
            from,
            midpoint,
            center,
            acceleration_bound,
            depth + 1,
        )
        .or_else(|| {
            moving_entry(
                origin,
                velocity,
                radius,
                midpoint,
                to,
                center,
                acceleration_bound,
                depth + 1,
            )
        });
    }
    sphere_entry(offset, relative_velocity, radius, duration).map(|entry| from + entry)
}

fn acceleration_bound(
    solver: &osg_universe::solver::Orrery,
    body: &osg_universe::orrery_cfg::Body,
) -> f64 {
    let mut body = Some(body);
    let mut acceleration = 0.0;
    while let Some(current) = body {
        let orbit = current.orbit;
        if orbit.semi_major > 0.0 && orbit.period > 0.0 {
            acceleration += orbit.semi_major * (std::f64::consts::TAU / orbit.period).powi(2)
                / (1.0 - orbit.eccentricity).powi(2);
        }
        body = current
            .parent
            .as_ref()
            .and_then(|name| solver.get_body(name));
    }
    acceleration
}

fn first_capture(
    world: &mut World,
    origin: GalacticPosition,
    velocity: DVec3,
    duration: f64,
    start_epoch: Epoch,
    ship_radius: f64,
) -> Result<Option<Capture>, osg_spatial::QueryError> {
    let mut earliest: Option<Capture> = None;
    let mut consider = |seconds, body, radius_m, physical| {
        if earliest.is_none_or(|old| seconds < old.seconds || seconds == old.seconds && physical) {
            earliest = Some(Capture {
                seconds,
                body,
                radius_m,
                physical,
            });
        }
    };
    if let Some(universe) = world.get_resource::<crate::sim::orrery::Universe>() {
        for index in universe.capture_candidates(
            origin,
            velocity * duration,
            ship_radius,
            &mut osg_spatial::QueryBudget::new(1_000_000),
        )? {
            let system = universe
                .resolve_index(index)
                .expect("valid capture candidate");
            for body in system.solver.iter() {
                if matches!(body.class_params, crate::sim::orrery::BodyClass::Barycenter) {
                    continue;
                }
                let reference = system.body_id(&body.name).map(|id| CelestialRef {
                    system: Id(id.system),
                    body: Id(id.local),
                });
                let exclusion = math::exclusion_radius_m(body.mass);
                let center = |seconds| {
                    system
                        .solver
                        .solve_position(&body.name, start_epoch + Duration::from_seconds(seconds))
                };
                for (radius, physical) in [(exclusion, false), (body.radius + ship_radius, true)] {
                    if radius > 0.0
                        && let Some(seconds) = moving_entry(
                            origin,
                            velocity,
                            radius,
                            0.0,
                            duration,
                            &center,
                            acceleration_bound(&system.solver, body),
                            0,
                        )
                    {
                        consider(seconds, reference, exclusion, physical);
                    }
                }
            }
        }
    } else {
        let Some(scene) = world.get_resource::<crate::sim::spatial::SpatialIndex>() else {
            return Ok(None);
        };
        for entity in scene.geometry().segment_candidates(
            origin,
            velocity * duration,
            ship_radius,
            &mut osg_spatial::QueryBudget::new(1_000_000),
        )? {
            let crate::sim::spatial::SpatialKey::Entity(entity) = entity else {
                continue;
            };
            let Some(celestial) = world.get::<CelestialState>(entity) else {
                continue;
            };
            if matches!(
                celestial.body.class_params,
                crate::sim::orrery::BodyClass::Barycenter
            ) {
                continue;
            }
            let exclusion = math::exclusion_radius_m(celestial.body.mass);
            for (radius, physical) in [
                (exclusion, false),
                (celestial.body.radius + ship_radius, true),
            ] {
                if radius > 0.0
                    && let Some(seconds) = sphere_entry(
                        origin.relative_to(
                            scene.objects()[scene.object_index(entity).expect("indexed celestial")]
                                .position,
                        ),
                        velocity - scene.velocity(entity),
                        radius,
                        duration,
                    )
                {
                    consider(seconds, Some(celestial.reference), exclusion, physical);
                }
            }
        }
    }
    Ok(earliest)
}

fn depart(world: &mut World, ship: Entity, preparation: &Preparation) -> Result<()> {
    let pose = ship_pose(world, ship)?;
    let mass = world
        .get::<MassProps>(ship)
        .context("ship mass unavailable")?
        .mass;
    let assisted = preparation.navigation_beacon.is_some_and(|beacon| {
        crate::sim::infrastructure::authenticated_navigation_beacon(world, ship, beacon)
    });
    ensure!(
        preparation.navigation_beacon.is_none() || assisted,
        "navigation beacon lost during charging"
    );
    ensure!(
        slip_admissible(world, ship, pose.position, radius(world, ship)?),
        "inadmissible slip departure"
    );
    ensure!(
        preparation
            .not_before_tick
            .is_none_or(|earliest| tick(world) >= earliest),
        "departure window has not opened"
    );
    let destination = preparation.destination;
    let aim = destination.relative_to(pose.position);
    ensure!(
        aim.is_finite() && aim.length_squared() > 0.0,
        "slip direction is undefined"
    );
    let direction = aim.normalize();
    ensure!(
        rings_aligned(world, ship, direction),
        "slip rings must align within one degree"
    );
    let mut speed = math::cruise_speed_ly_s(assisted);
    // Fit short transits to three seconds using the actual first capture surface.
    // Iteration accounts for a moving capture body at the slower transit speed.
    for _ in 0..8 {
        let Some(capture) = first_capture(
            world,
            pose.position,
            direction * speed * math::LY_M,
            math::MIN_TRANSIT_SECONDS,
            epoch(world),
            radius(world, ship)?,
        )?
        else {
            break;
        };
        if capture.physical || capture.seconds >= math::MIN_TRANSIT_SECONDS - 1e-8 {
            break;
        }
        speed *= (capture.seconds / math::MIN_TRANSIT_SECONDS).max(1e-9);
    }
    let now = tick(world);
    world.entity_mut(ship).insert(Transit {
        origin: pose.position,
        position: pose.position,
        destination,
        departed: now,
        advanced_tick: now,
        direction: direction.to_array(),
        speed_ly_s: speed,
        retained_velocity: pose.velocity,
        requested_delta_v: preparation.arrival_velocity.map_or([0.0; 3], |arrival| {
            (DVec3::from_array(arrival) - DVec3::from_array(pose.velocity)).to_array()
        }),
        departure_mass_kg: mass,
        distance_ly: 0.0,
        consumed_fuel_g: 0,
        navigation_beacon: preparation.navigation_beacon,
        beacon_lost: false,
        nominal_direction: direction.to_array(),
        variance_m2: 0.0,
    });
    world.get_mut::<SlipDrive>(ship).unwrap().preparation = None;
    crate::sim::slip_effects::record_transition(
        world,
        ship,
        pose.position,
        pose.velocity,
        direction.to_array(),
        false,
        now * osg_model::TICK_NS,
    );
    set_dormant(world, ship, Presence::SlipTransit(Id::new()));
    emit(world, ship, "slip-departed", None);
    Ok(())
}

fn lose_beacon(world: &mut World, ship: Entity, transit: &mut Transit) {
    if transit.beacon_lost || transit.navigation_beacon.is_none() {
        return;
    }
    if transit.navigation_beacon.is_some_and(|beacon| {
        crate::sim::infrastructure::authenticated_navigation_beacon(world, ship, beacon)
    }) {
        return;
    }
    transit.beacon_lost = true;
    transit.speed_ly_s *= 0.1;
    emit(world, ship, "slip-beacon-lost", Some(transit.position));
}

fn walk_direction(transit: &Transit, distance: f64, samples: [f64; 2]) -> (DVec3, f64) {
    let axis = DVec3::from_array(transit.nominal_direction);
    let progress = transit
        .position
        .relative_to(transit.origin)
        .dot(axis)
        .max(0.0);
    let variance = math::walk_variance(
        progress,
        distance,
        transit.navigation_beacon.is_some() && !transit.beacon_lost,
    );
    let scale = if distance > 0.0 {
        variance.sqrt() / distance
    } else {
        0.0
    };
    (
        deflected(axis, samples.map(|sample| sample * scale)),
        variance,
    )
}

fn advance_transit(world: &mut World, ship: Entity, mut transit: Transit) {
    let now = tick(world);
    if transit.advanced_tick > now {
        return;
    }
    lose_beacon(world, ship, &mut transit);
    let fuel = fuel_grams(world, ship);
    let paid_kg = transit.consumed_fuel_g as f64 * 0.001;
    let requested_delta_v = DVec3::from_array(transit.requested_delta_v).length();
    let max_distance = math::transit_range_ly(
        transit.departure_mass_kg,
        paid_kg + fuel * 0.001,
        requested_delta_v,
    );
    let seconds_left = ((max_distance - transit.distance_ly) / transit.speed_ly_s).max(0.0);
    let duration = osg_model::TICK_SECONDS.min(seconds_left);
    let speed = transit.speed_ly_s * math::LY_M;
    let mut elapsed = 0.0;
    let mut capture = None;
    let mut query_stalled = false;
    while elapsed < duration {
        let mut step = duration - elapsed;
        let remaining = transit
            .destination
            .relative_to(transit.position)
            .dot(DVec3::from_array(transit.nominal_direction));
        if remaining > speed * 1e-10 {
            step = step.min(remaining / speed);
        }
        let (direction, variance) = walk_direction(&transit, speed * step, gaussian_pair());
        let velocity = direction * speed;
        let hit = match first_capture(
            world,
            transit.position,
            velocity,
            step,
            epoch(world) + Duration::from_seconds(elapsed),
            radius(world, ship).unwrap_or(0.0),
        ) {
            Ok(hit) => hit,
            Err(error) => {
                warn!(?ship, %error, "slip motion deferred: capture query incomplete");
                query_stalled = true;
                break;
            }
        };
        let travelled = hit.map_or(step, |hit| hit.seconds);
        let previous = transit.position;
        transit.position = transit.position.offset_by(velocity * travelled);
        transit.direction = direction.to_array();
        transit.variance_m2 += variance * (travelled / step).powi(2);
        crate::sim::slip_effects::record_span(
            world,
            ship,
            previous,
            transit.position,
            now * osg_model::TICK_NS + (elapsed * 1e9).round() as u64,
            now * osg_model::TICK_NS + ((elapsed + travelled) * 1e9).round() as u64,
            transit.retained_velocity,
        );
        if let Some(mut hit) = hit {
            hit.seconds += elapsed;
            elapsed += travelled;
            capture = Some(hit);
            break;
        }
        elapsed += travelled;
    }
    transit.distance_ly += transit.speed_ly_s * elapsed;
    let cumulative = (math::transit_fuel_kg(
        transit.departure_mass_kg,
        transit.distance_ly,
        requested_delta_v,
    ) * 1000.0)
        .ceil() as u64;
    let paid = cumulative
        .saturating_sub(transit.consumed_fuel_g)
        .min(fuel as u64);
    consume_fuel(world, ship, paid);
    transit.consumed_fuel_g += paid;
    transit.advanced_tick = now + 1;
    // Thermal exposure uses the actual time spent in slipspace. A captured ship
    // remains dormant for the rest of this tick and materializes at its boundary.
    if let Ok((design, mut hull, mut thermal)) = world
        .query::<(
            &ShipDesign,
            &mut crate::sim::hardware::Hull,
            &mut crate::sim::hardware::ShipThermal,
        )>()
        .get_mut(world, ship)
    {
        thermal.0.advance_in_environment(
            &mut hull.0,
            design.0.as_ref().into(),
            if query_stalled {
                osg_model::TICK_SECONDS
            } else {
                elapsed
            },
            osg_ships::thermal::SLIPSPACE_K,
        );
    }
    world
        .get_mut::<PreciseTransform>(ship)
        .unwrap()
        .translation_um = transit.position;
    if world
        .get::<crate::sim::hardware::Hull>(ship)
        .is_some_and(|hull| hull.0 <= 0.0)
    {
        destroy(world, ship);
        emit(world, ship, "slip-overheated", Some(transit.position));
    } else if let Some(capture) = capture {
        if capture.physical {
            destroy(world, ship);
            emit(world, ship, "slip-collision", Some(transit.position));
        } else {
            world
                .entity_mut(ship)
                .insert(PendingArrival { transit, capture });
        }
    } else if seconds_left <= osg_model::TICK_SECONDS {
        destroy(world, ship);
        emit(world, ship, "slip-fuel-exhausted", Some(transit.position));
    } else {
        world.entity_mut(ship).insert(transit);
    }
}

#[derive(bevy::ecs::query::QueryData)]
#[query_data(mutable)]
pub(crate) struct Arrival {
    entity: Entity,
    pending: &'static PendingArrival,
    pose: &'static mut PreciseTransform,
    design: &'static ShipDesign,
    program: Option<&'static mut crate::sim::vessel::ProgramWorld>,
    hull: Option<&'static mut crate::sim::hardware::Hull>,
    thermal: Option<&'static mut crate::sim::hardware::ShipThermal>,
}

pub(crate) fn finish_arrivals(
    mut commands: Commands,
    clock: Res<SimulationCounters>,
    mut ships: Query<Arrival>,
    mut history: ResMut<crate::sim::slip_effects::SlipHistory>,
    mut events: ResMut<TravelEvents>,
) {
    let now = clock.ticks + 1;
    for mut ship in &mut ships {
        let transit = &ship.pending.transit;
        let capture = ship.pending.capture;
        if let (Some(hull), Some(thermal)) = (ship.hull.as_mut(), ship.thermal.as_mut()) {
            thermal.0.advance(
                &mut hull.0,
                ship.design.0.as_ref().into(),
                (osg_model::TICK_SECONDS - capture.seconds).max(0.0),
            );
        }
        history.push_transition(osg_model::slip_visual::SlipTransition {
            view: 0,
            id: Id::new(),
            time_ns: now * osg_model::TICK_NS,
            position: transit.position,
            drift_m_s: transit.arrival_velocity().to_array(),
            direction: transit.direction,
            arriving: true,
            radius_m: ship.design.0.radius,
            seed: rand::random(),
        });
        ship.pose.translation_um = transit.position;
        let entity = ship.entity;
        let velocity = transit.arrival_velocity();
        commands.queue(move |world: &mut World| {
            world
                .entity_mut(entity)
                .remove::<(Transit, PendingArrival)>()
                .insert(Velocity(velocity));
            set_active(world, entity);
        });
        if let Some(program) = ship.program.as_mut() {
            program.world_actions.clear();
        }
        events
            .0
            .push((ship.entity, "slip-arrived", Some(transit.position)));
    }
}

pub(super) fn advance(world: &mut World) {
    let now = tick(world);
    let preparing: Vec<_> = world
        .query::<(Entity, &SlipDrive)>()
        .iter(world)
        .filter_map(|(ship, drive)| {
            drive
                .preparation
                .clone()
                .map(|preparation| (ship, drive.power_w, preparation))
        })
        .collect();
    for (ship, power, mut preparation) in preparing {
        let Ok(pose) = ship_pose(world, ship) else {
            continue;
        };
        let mass = world
            .get::<MassProps>(ship)
            .map_or(f64::INFINITY, |mass| mass.mass);
        let ship_radius = radius(world, ship).unwrap_or(f64::INFINITY);
        if !active(world, ship) || mass > preparation.mass * 1.001 {
            cancel_pending(world, ship);
            emit(
                world,
                ship,
                "slip-preparation-invalidated",
                Some(pose.position),
            );
            continue;
        }
        preparation.required_j = math::charging_energy_j(
            preparation.mass,
            preparation.destination.relative_to(pose.position).length() / math::LY_M,
        )
        .ceil();
        let requested = (power * osg_model::TICK_SECONDS)
            .min(preparation.required_j - preparation.work_j)
            .max(0.0)
            .ceil() as u64;
        let paid = if let Some(mut inventory) = world.get_mut::<ShipInventory>(ship) {
            let paid = requested.min(inventory.0.energy_j);
            inventory.0.energy_j -= paid;
            paid
        } else {
            0
        };
        world
            .entity_mut(ship)
            .insert(SlipChargingPower(paid as f64 * osg_model::TICK_RATE_HZ));
        crate::sim::hardware::add_travel_heat(
            world,
            ship,
            paid as f64 * 0.2,
            osg_model::TICK_SECONDS,
        );
        let mut drive = world.get_mut::<SlipDrive>(ship).unwrap();
        let stored = drive.preparation.as_mut().unwrap();
        stored.required_j = preparation.required_j;
        stored.work_j += paid as f64;
        let work = preparation.work_j + paid as f64;
        if work >= preparation.required_j
            && preparation
                .not_before_tick
                .is_none_or(|earliest| now >= earliest)
            && slip_admissible(world, ship, pose.position, ship_radius)
            && (now.saturating_sub(preparation.started) as f64 * osg_model::TICK_SECONDS)
                >= math::MIN_CHARGE_SECONDS
            && rings_aligned(
                world,
                ship,
                preparation
                    .destination
                    .relative_to(pose.position)
                    .normalize_or_zero(),
            )
        {
            if let Err(error) = depart(world, ship, &preparation) {
                cancel_pending(world, ship);
                warn!(?ship, %error, "slip departure deferred");
                emit(world, ship, "slip-departure-deferred", Some(pose.position));
            }
        }
    }
    let transits: Vec<_> = world
        .query::<(Entity, &Transit)>()
        .iter(world)
        .map(|(ship, transit)| (ship, transit.clone()))
        .collect();
    for (ship, transit) in transits {
        if world
            .get::<crate::sim::hardware::Hull>(ship)
            .is_some_and(|hull| hull.0 <= 0.0)
        {
            destroy(world, ship);
        } else {
            advance_transit(world, ship, transit);
        }
    }
}

fn rings_aligned(world: &World, ship: Entity, direction: DVec3) -> bool {
    let Ok(pose) = ship_pose(world, ship) else {
        return false;
    };
    let rotation = bevy::math::DQuat::from_array(pose.rotation);
    let aligned = |axis: DVec3| (rotation * axis).dot(direction) >= 1.0_f64.to_radians().cos();
    world
        .get::<crate::sim::vessel::ShipDesign>(ship)
        .map_or_else(
            || aligned(DVec3::NEG_Z),
            |design| {
                design
                    .0
                    .parts
                    .iter()
                    .filter(|part| {
                        matches!(
                            part.definition.equipment,
                            osg_ships::Equipment::Utility {
                                utility: osg_ships::utilities::UtilityDef::SlipDrive { .. }
                            }
                        )
                    })
                    .all(|part| aligned(part.rotation * DVec3::NEG_Z))
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::orrery::orrery_cfg::{Body, BodyClass};
    use std::sync::{Arc, OnceLock};

    fn fixture(fuel_g: u64) -> (World, Entity) {
        static CATALOGUE: OnceLock<osg_ships::Catalogue> = OnceLock::new();
        static DESIGN: OnceLock<Arc<osg_ships::CompiledShipDesign>> = OnceLock::new();
        let catalogue = CATALOGUE.get_or_init(osg_ships::Catalogue::builtin);
        let design = DESIGN
            .get_or_init(|| Arc::new(osg_ships::expedition_patrol().compile(catalogue).unwrap()));
        let mut inventory = osg_ships::Inventory::empty(catalogue);
        let index = catalogue
            .resources
            .iter()
            .position(|resource| resource.id == math::EXOTIC_RESOURCE)
            .unwrap();
        inventory.quantities[index] = fuel_g;
        inventory.energy_j = 1_000_000_000;
        let mut world = World::new();
        world.init_resource::<SimulationCounters>();
        world.init_resource::<TravelEvents>();
        world.init_resource::<crate::sim::slip_effects::SlipHistory>();
        world.init_resource::<identity::IdentityIndex>();
        world.insert_resource(ShipCatalogue(catalogue.clone()));
        let ship = world
            .spawn((
                ShipDesign(design.clone()),
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::NEG_X * 1000.0),
                    rotation: DQuat::from_rotation_arc(DVec3::NEG_Z, DVec3::X),
                    ..Default::default()
                },
                Velocity(DVec3::Y * 7.0),
                AngularVelocity(DVec3::ZERO),
                MassProps {
                    mass: 1000.0,
                    ..Default::default()
                },
                SlipDrive::default(),
                ShipInventory(inventory),
                Travel::default(),
                PresenceState::default(),
            ))
            .id();
        identity::register(&mut world, ship, Id::new()).unwrap();
        crate::sim::spatial::rebuild(&mut world);
        (world, ship)
    }

    #[test]
    fn full_starter_tank_buys_one_hundred_kilometers_per_second() {
        let catalogue = osg_ships::Catalogue::builtin();
        let design = osg_ships::expedition_patrol().compile(&catalogue).unwrap();
        let mut inventory = osg_ships::Inventory::for_design(&design, &catalogue);
        let fuel = catalogue
            .resources
            .iter()
            .position(|resource| resource.id == math::EXOTIC_RESOURCE)
            .unwrap();
        inventory.quantities[fuel] = (inventory.tank_capacities_m3[fuel]
            / catalogue.resources[fuel].volume_m3)
            .floor() as u64;
        let mass = design.dry_mass
            + inventory.mass(&catalogue)
            + design.shield_deployed_kg
            + design.shield_reserve_capacity_kg;
        let fuel_kg = inventory.quantities[fuel] as f64 * catalogue.resources[fuel].mass_kg;
        let delta_v = fuel_kg / (math::DELTA_V_FUEL_KG_PER_KG_M_S * mass);
        assert!(
            (delta_v - 100_000.0).abs() < 1.0,
            "mass={mass}, delta_v={delta_v}"
        );
    }

    #[test]
    fn transit_pays_combined_cost_and_arrival_uses_earned_velocity() {
        let (mut world, ship) = fixture(1000);
        let start = ship_pose(&world, ship).unwrap().position;
        let mut flight = transit(start, 0.1, None);
        flight.requested_delta_v = [100_000.0, 0.0, 0.0];
        set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        advance_transit(&mut world, ship, flight);
        let flight = world.get::<Transit>(ship).unwrap().clone();
        let cost =
            (math::transit_fuel_kg(1000.0, flight.distance_ly, 100_000.0) * 1000.0).ceil() as u64;
        assert_eq!(flight.consumed_fuel_g, cost);
        assert_eq!(fuel_grams(&world, ship), (1000 - cost) as f64);
        let expected = flight.arrival_velocity();
        assert!((expected.x - 100.0).abs() < 1e-10);
        world.entity_mut(ship).insert(PendingArrival {
            transit: flight,
            capture: Capture {
                seconds: osg_model::TICK_SECONDS,
                body: None,
                radius_m: 0.0,
                physical: false,
            },
        });
        world.run_system_cached(finish_arrivals).unwrap();
        assert_eq!(world.get::<Velocity>(ship).unwrap().0, expected);
    }

    fn natural_body(
        world: &mut World,
        position: GalacticPosition,
        physical_radius: f64,
    ) -> CelestialRef {
        let reference = CelestialRef {
            system: Id::new(),
            body: Id::new(),
        };
        world.spawn((
            PreciseTransform {
                translation_um: position,
                ..Default::default()
            },
            crate::sim::spatial::SpatialBody {
                radius_m: physical_radius,
                occludes: true,
            },
            CelestialState {
                reference,
                body: Body {
                    key: "capture".into(),
                    name: "Capture".into(),
                    mass: math::SOLAR_MASS_KG
                        * (100.0 / math::exclusion_radius_m(math::SOLAR_MASS_KG)).powi(3),
                    radius: physical_radius,
                    class_params: BodyClass::Planet,
                    ..Default::default()
                },
                system: 0,
                anchor: position,
                influence: 10000.0,
                velocity: DVec3::ZERO,
            },
        ));
        crate::sim::spatial::rebuild(world);
        reference
    }

    fn transit(
        position: GalacticPosition,
        speed_ly_s: f64,
        _target: Option<CelestialRef>,
    ) -> Transit {
        Transit {
            origin: position,
            position,
            destination: GalacticPosition::ZERO,
            departed: 0,
            advanced_tick: 0,
            direction: DVec3::X.to_array(),
            speed_ly_s,
            retained_velocity: (DVec3::Y * 7.0).to_array(),
            requested_delta_v: [0.0; 3],
            departure_mass_kg: 1000.0,
            distance_ly: 0.0,
            consumed_fuel_g: 0,
            navigation_beacon: None,
            beacon_lost: false,
            nominal_direction: DVec3::X.to_array(),
            variance_m2: 0.0,
        }
    }

    #[test]
    fn charge_overlaps_clearance_and_waits_for_departure_window() {
        let (mut world, ship) = fixture(1000);
        natural_body(&mut world, GalacticPosition::ZERO, 10.0);
        world
            .get_mut::<PreciseTransform>(ship)
            .unwrap()
            .translation_um = GalacticPosition::from_meters(DVec3::NEG_X * 50.0);
        crate::sim::spatial::rebuild(&mut world);
        prepare_slip(
            &mut world,
            ship,
            GalacticPosition::ZERO,
            None,
            None,
            Some(200),
        )
        .unwrap();
        for tick in 0..=100 {
            world.resource_mut::<SimulationCounters>().ticks = tick;
            advance(&mut world);
        }
        let charge = world
            .get::<SlipDrive>(ship)
            .unwrap()
            .preparation
            .as_ref()
            .unwrap();
        assert!(charge.work_j >= charge.required_j);
        assert!(world.get::<Transit>(ship).is_none());
        world
            .get_mut::<PreciseTransform>(ship)
            .unwrap()
            .translation_um = GalacticPosition::from_meters(DVec3::NEG_X * 1000.0);
        crate::sim::spatial::rebuild(&mut world);
        world.resource_mut::<SimulationCounters>().ticks = 199;
        advance(&mut world);
        assert!(world.get::<Transit>(ship).is_none());
        world.resource_mut::<SimulationCounters>().ticks = 200;
        advance(&mut world);
        assert!(world.get::<Transit>(ship).is_some());
    }

    #[test]
    fn charged_drive_waits_until_ring_is_within_one_degree() {
        let (mut world, ship) = fixture(1000);
        natural_body(&mut world, GalacticPosition::ZERO, 10.0);
        let aligned = DQuat::from_rotation_arc(DVec3::NEG_Z, DVec3::X);
        world.get_mut::<PreciseTransform>(ship).unwrap().rotation =
            DQuat::from_rotation_y(1.1_f64.to_radians()) * aligned;
        prepare_slip(&mut world, ship, GalacticPosition::ZERO, None, None, None).unwrap();
        for tick in 0..=100 {
            world.resource_mut::<SimulationCounters>().ticks = tick;
            advance(&mut world);
        }
        assert!(world.get::<Transit>(ship).is_none());
        assert!(world.get::<SlipDrive>(ship).unwrap().preparation.is_some());

        world.get_mut::<PreciseTransform>(ship).unwrap().rotation =
            DQuat::from_rotation_y(0.9_f64.to_radians()) * aligned;
        world.resource_mut::<SimulationCounters>().ticks = 101;
        advance(&mut world);
        assert!(world.get::<Transit>(ship).is_some());
        assert!(world.get::<SlipDrive>(ship).unwrap().preparation.is_none());
    }

    #[test]
    fn retargeting_charge_scales_energy_with_distance_without_resetting_paid_work() {
        let (mut world, ship) = fixture(1000);
        let origin = ship_pose(&world, ship).unwrap().position;
        prepare_slip(
            &mut world,
            ship,
            origin.offset_by(DVec3::X * math::LY_M),
            None,
            None,
            None,
        )
        .unwrap();
        let initial = world
            .get::<SlipDrive>(ship)
            .unwrap()
            .preparation
            .clone()
            .unwrap();
        assert_eq!(initial.required_j, 5_000_000.0);
        world
            .get_mut::<SlipDrive>(ship)
            .unwrap()
            .preparation
            .as_mut()
            .unwrap()
            .work_j = 123.0;
        prepare_slip(
            &mut world,
            ship,
            origin.offset_by(DVec3::X * 10.0 * math::LY_M),
            None,
            None,
            None,
        )
        .unwrap();
        let changed = world
            .get::<SlipDrive>(ship)
            .unwrap()
            .preparation
            .as_ref()
            .unwrap();
        assert_eq!(changed.required_j, initial.required_j * 10.0);
        assert_eq!(changed.work_j, 123.0);
        assert_eq!(changed.started, initial.started);
    }

    #[test]
    fn short_slips_take_three_seconds_and_retain_velocity() {
        let (mut world, ship) = fixture(1000);
        natural_body(&mut world, GalacticPosition::ZERO, 10.0);
        prepare_slip(&mut world, ship, GalacticPosition::ZERO, None, None, None).unwrap();
        for tick in 0..100 {
            world.resource_mut::<SimulationCounters>().ticks = tick;
            advance(&mut world);
            assert!(world.get::<Transit>(ship).is_none());
            assert!(world.get::<SlipDrive>(ship).unwrap().preparation.is_some());
        }
        world.resource_mut::<SimulationCounters>().ticks = 100;
        advance(&mut world);
        assert!(world.get::<SlipDrive>(ship).unwrap().preparation.is_none());
        for tick in 101..129 {
            assert!(world.get::<Transit>(ship).is_some());
            world.resource_mut::<SimulationCounters>().ticks = tick;
            advance(&mut world);
        }
        assert!(world.get::<Transit>(ship).is_some());
        world.resource_mut::<SimulationCounters>().ticks = 129;
        advance(&mut world);
        world.run_system_cached(finish_arrivals).unwrap();
        if world.get::<Transit>(ship).is_some() {
            world.resource_mut::<SimulationCounters>().ticks = 130;
            advance(&mut world);
            world.run_system_cached(finish_arrivals).unwrap();
        }
        assert!(world.get::<Transit>(ship).is_none());
        assert_eq!(world.get::<PresenceState>(ship).unwrap().0, Presence::Space);
        assert_eq!(world.get::<Velocity>(ship).unwrap().0, DVec3::Y * 7.0);
        let elapsed = (world.resource::<SimulationCounters>().ticks + 1 - 100) as f64
            * osg_model::TICK_SECONDS;
        assert!((3.0..=3.1).contains(&elapsed));
        let position = world.get::<PreciseTransform>(ship).unwrap().translation_um;
        assert!((position.to_meters_64().length() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn arrival_applies_slip_heat_only_until_capture() {
        use crate::sim::hardware::{Hull, ShipThermal};
        let (mut world, ship) = fixture(1000);
        let target = natural_body(&mut world, GalacticPosition::ZERO, 10.0);
        let design = world.get::<ShipDesign>(ship).unwrap().0.clone();
        let model = design.as_ref().into();
        let state = osg_ships::thermal::ThermalState::new(model);
        world
            .entity_mut(ship)
            .insert((Hull(design.hull), ShipThermal(state)));
        let start = world.get::<PreciseTransform>(ship).unwrap().translation_um;
        set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        advance_transit(
            &mut world,
            ship,
            transit(start, 18_000.0 / math::LY_M, Some(target)),
        );
        let elapsed = world.get::<PendingArrival>(ship).unwrap().capture.seconds;
        assert!((elapsed - 0.05).abs() < 1e-6);
        assert!(world.get::<Dormant>(ship).is_some());
        assert!(world.get::<crate::sim::physics::RigidBody>(ship).is_none());
        let mut expected = state;
        let mut hull = design.hull;
        expected.advance_in_environment(&mut hull, model, elapsed, osg_ships::thermal::SLIPSPACE_K);
        let actual = &world.get::<ShipThermal>(ship).unwrap().0;
        assert!(actual.hull_energy_j > 0.0);
        assert_eq!(actual.hull_energy_j, expected.hull_energy_j);
        assert_eq!(world.get::<Hull>(ship).unwrap().0, hull);
        world.run_system_cached(finish_arrivals).unwrap();
        expected.advance(&mut hull, model, osg_model::TICK_SECONDS - elapsed);
        assert_eq!(
            world.get::<ShipThermal>(ship).unwrap().0.hull_energy_j,
            expected.hull_energy_j
        );
        assert!(world.get::<Dormant>(ship).is_none());
    }

    #[test]
    fn physical_collision_and_exhaustion_precede_farther_captures() {
        let (mut world, ship) = fixture(1000);
        let target = natural_body(&mut world, GalacticPosition::ZERO, 200.0);
        let start = world.get::<PreciseTransform>(ship).unwrap().translation_um;
        set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        advance_transit(
            &mut world,
            ship,
            transit(start, 20_000.0 / math::LY_M, Some(target)),
        );
        assert_eq!(
            world.get::<PresenceState>(ship).unwrap().0,
            Presence::Destroyed
        );

        let (mut world, ship) = fixture(1);
        let start = GalacticPosition::ZERO;
        world
            .get_mut::<PreciseTransform>(ship)
            .unwrap()
            .translation_um = start;
        let target = natural_body(
            &mut world,
            start.offset_by(DVec3::X * 0.01 * math::LY_M),
            10.0,
        );
        let mut flight = transit(start, 1.0, Some(target));
        flight.departure_mass_kg = 1e6;
        flight.requested_delta_v = [100_000.0, 0.0, 0.0];
        let expected =
            math::transit_range_ly(flight.departure_mass_kg, 0.001, 100_000.0) * math::LY_M;
        set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        advance_transit(&mut world, ship, flight);
        assert_eq!(
            world.get::<PresenceState>(ship).unwrap().0,
            Presence::Destroyed
        );
        let actual = world
            .get::<PreciseTransform>(ship)
            .unwrap()
            .translation_um
            .relative_to(start)
            .length();
        assert!((actual - expected).abs() < expected * 1e-12);
        assert!(fuel_grams(&world, ship) < 1e-12);
    }

    #[test]
    fn cumulative_fuel_rounds_once_per_transit() {
        let (mut world, ship) = fixture(1000);
        let mut previous = 0;
        for distance in [0.01, 0.03, 0.1, 0.2] {
            let cumulative = (math::exotic_fuel_kg(1000.0, distance) * 1000.0).ceil() as u64;
            consume_fuel(&mut world, ship, cumulative - previous);
            previous = cumulative;
        }
        let remaining = fuel_grams(&world, ship);
        let (mut other, other_ship) = fixture(1000);
        consume_fuel(&mut other, other_ship, previous);
        assert!((remaining - fuel_grams(&other, other_ship)).abs() < 1e-12);
        consume_fuel(&mut world, ship, 1);
        assert_eq!(fuel_grams(&world, ship), remaining - 1.0);
    }

    #[test]
    fn random_walk_matches_swept_capture_odds_across_step_sizes() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(90210);
        let distance = 1e12;
        for assisted in [false, true] {
            for steps in [8, 64] {
                for expected_loss in [0.1_f64, 0.5] {
                    let sigma = math::dispersion_rad(assisted) * distance;
                    let radius = sigma * (-2.0 * expected_loss.ln()).sqrt();
                    let mut misses = 0;
                    let trials = 10_000;
                    for _ in 0..trials {
                        let mut flight = transit(
                            GalacticPosition::from_meters(DVec3::NEG_X * distance),
                            0.03,
                            None,
                        );
                        flight.navigation_beacon = assisted.then(Id::new);
                        let mut captured = false;
                        for _ in 0..steps {
                            let (direction, variance) = walk_direction(
                                &flight,
                                distance / steps as f64,
                                gaussian_pair_with(&mut rng),
                            );
                            let delta = direction * (distance / steps as f64);
                            captured |= sphere_entry(
                                flight.position.relative_to(GalacticPosition::ZERO),
                                delta,
                                radius,
                                1.0,
                            )
                            .is_some();
                            flight.position = flight.position.offset_by(delta);
                            flight.variance_m2 += variance;
                        }
                        misses += usize::from(!captured);
                    }
                    let actual = misses as f64 / trials as f64;
                    assert!(
                        (actual - expected_loss).abs() < 0.02,
                        "assisted={assisted}, steps={steps}: {actual} != {expected_loss}"
                    );
                }
            }
        }
    }

    #[test]
    fn earned_velocity_and_fuel_survive_serialization() {
        let mut flight = transit(
            GalacticPosition::from_meters(DVec3::NEG_X * 1e12),
            0.03,
            None,
        );
        flight.requested_delta_v = [100_000.0, 0.0, 0.0];
        assert_eq!(flight.arrival_velocity(), DVec3::Y * 7.0);
        flight.distance_ly = 0.5;
        flight.consumed_fuel_g = 123;
        assert_eq!(flight.arrival_velocity(), DVec3::new(5000.0, 7.0, 0.0));
        let restored: Transit =
            postcard::from_bytes(&postcard::to_stdvec(&flight).unwrap()).unwrap();
        assert_eq!(restored.arrival_velocity(), flight.arrival_velocity());
        assert_eq!(restored.consumed_fuel_g, 123);
        let (left, _) = walk_direction(&restored, 1e9, [-1.0, 0.0]);
        let (right, _) = walk_direction(&restored, 1e9, [1.0, 0.0]);
        assert_ne!(left, right);
    }

    #[test]
    fn beacon_loss_changes_future_diffusion_once_and_survives_serialization() {
        let (mut world, ship) = fixture(1000);
        let mut flight = transit(
            GalacticPosition::from_meters(DVec3::NEG_X * math::LY_M),
            0.05,
            None,
        );
        flight.navigation_beacon = Some(Id::new());
        let before_loss = postcard::to_stdvec(&flight).unwrap();
        lose_beacon(&mut world, ship, &mut flight);
        let saved = postcard::to_stdvec(&flight).unwrap();
        let mut restored: Transit = postcard::from_bytes(&before_loss).unwrap();
        lose_beacon(&mut world, ship, &mut restored);
        lose_beacon(&mut world, ship, &mut restored);
        assert_eq!(postcard::to_stdvec(&restored).unwrap(), saved);
        assert!(restored.beacon_lost);
        assert_eq!(restored.direction, DVec3::X.to_array());
        assert!((restored.speed_ly_s - 0.005).abs() < 1e-12);
    }

    #[test]
    fn moving_body_intersection_follows_curved_ephemeris() {
        let center = |time: f64| {
            Some(GalacticPosition::from_meters(DVec3::new(
                0.0,
                (time - 0.5).powi(2) * 100.0,
                0.0,
            )))
        };
        let hit = moving_entry(
            GalacticPosition::from_meters(DVec3::NEG_X * 100.0),
            DVec3::X * 200.0,
            1.0,
            0.0,
            1.0,
            &center,
            200.0,
            0,
        )
        .unwrap();
        assert!(hit > 0.494 && hit < 0.496);
        let position = DVec3::new(-100.0 + hit * 200.0, 0.0, 0.0);
        assert!((position - center(hit).unwrap().to_meters_64()).length() <= 1.001);
    }

    #[test]
    fn transit_queries_dormant_system_without_spawning_celestials() {
        let mut world = World::new();
        world.init_resource::<SimulationCounters>();
        let universe = crate::sim::orrery::Universe::init(osg_universe::orrery_cfg::OrreryCfg {
            key: "test/capture".into(),
            name: "Capture".into(),
            position_um: GalacticPosition::ZERO,
            bodies: vec![Body {
                key: "star".into(),
                name: "Capture star".into(),
                mass: math::SOLAR_MASS_KG,
                radius: 696_000_000.0,
                class_params: BodyClass::Star { lumens: 3.8e26 },
                ..Default::default()
            }],
        })
        .unwrap();
        world.insert_resource(universe);
        let radius = math::exclusion_radius_m(math::SOLAR_MASS_KG);
        let start_epoch = epoch(&world);
        let capture = first_capture(
            &mut world,
            GalacticPosition::from_meters(DVec3::NEG_X * 2.0 * radius),
            DVec3::X * 40.0 * radius,
            0.1,
            start_epoch,
            10.0,
        )
        .unwrap()
        .unwrap();
        assert!((capture.seconds - 0.025).abs() < 1e-12);
        assert!(!capture.physical);
        assert!(capture.body.is_some());
        assert_eq!(world.query::<&CelestialState>().iter(&world).count(), 0);
    }

    #[test]
    fn controller_aim_can_miss_queued_target_and_exhaust_fuel() {
        let (mut world, ship) = fixture(1);
        world.get_mut::<MassProps>(ship).unwrap().mass = 1e6;
        let destination = GalacticPosition::from_meters(DVec3::X * 10.0 * math::LY_M);
        let universe = crate::sim::orrery::Universe::init(osg_universe::orrery_cfg::OrreryCfg {
            key: "test/admission".into(),
            name: "Admission".into(),
            position_um: destination,
            bodies: vec![Body {
                key: "star".into(),
                name: "Admission star".into(),
                mass: math::SOLAR_MASS_KG,
                radius: 696_000_000.0,
                class_params: BodyClass::Star { lumens: 3.8e26 },
                ..Default::default()
            }],
        })
        .unwrap();
        world.insert_resource(universe);
        world.init_resource::<crate::sim::ownership::Directory>();
        let owner =
            crate::sim::ownership::AssetOwner(osg_model::ownership::Principal::Player(Id::new()));
        world.entity_mut(ship).insert(owner);
        let beacon = world
            .spawn((
                owner,
                identity::NavigationBeaconEmitter,
                PreciseTransform {
                    translation_um: destination.offset_by(DVec3::Y * math::AU_M),
                    ..Default::default()
                },
            ))
            .id();
        let beacon_id = Id::new();
        identity::register(&mut world, beacon, beacon_id).unwrap();
        let origin = world.get::<PreciseTransform>(ship).unwrap().translation_um;
        // The controller aims away from its queued star, beyond its prediction horizon.
        // Neither the route's risk budget nor inadequate range prevents commitment.
        let commanded = origin.offset_by(DVec3::NEG_X * 1e10 * math::LY_M);
        world.get_mut::<PreciseTransform>(ship).unwrap().rotation =
            DQuat::from_rotation_arc(DVec3::NEG_Z, DVec3::NEG_X);
        prepare_slip(&mut world, ship, commanded, Some(beacon_id), None, None).unwrap();
        let preparation = world
            .get::<SlipDrive>(ship)
            .unwrap()
            .preparation
            .clone()
            .unwrap();
        depart(&mut world, ship, &preparation).unwrap();
        let flight = world.get::<Transit>(ship).unwrap().clone();
        assert_eq!(flight.destination, commanded);
        assert_eq!(flight.navigation_beacon, Some(beacon_id));
        assert!(DVec3::from_array(flight.direction).dot(DVec3::NEG_X) > 0.99);
        advance_transit(&mut world, ship, flight);
        assert_eq!(
            world.get::<PresenceState>(ship).unwrap().0,
            Presence::Destroyed
        );
        let displacement = world
            .get::<PreciseTransform>(ship)
            .unwrap()
            .translation_um
            .relative_to(origin);
        assert!(displacement.x < 0.0);
        assert!(fuel_grams(&world, ship) < 1e-12);
    }
}
