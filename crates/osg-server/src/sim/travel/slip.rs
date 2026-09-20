use super::*;
use crate::sim::{hardware::ShipInventory, vessel::ShipCatalogue};
use anyhow::Context;
use hifitime::{Duration, Epoch};
use osg_model::travel::slip as math;
use rand::RngExt;

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SlipDrive {
    pub power_w: f64,
    pub preparation: Option<Preparation>,
    /// Fuel already consumed from the next inventory gram.
    pub fuel_fraction_g: f64,
}

impl Default for SlipDrive {
    fn default() -> Self {
        Self {
            power_w: 500e6,
            preparation: None,
            fuel_fraction_g: 0.0,
        }
    }
}

#[derive(Component, Default)]
pub struct SlipChargingPower(pub f64);

/// Seconds after the beginning of this tick at which ordinary physics resumes.
#[derive(Component, Clone, Copy, Debug)]
pub struct ArrivalOffset(pub f64);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Preparation {
    pub destination: GalacticPosition,
    pub speed_ly_s: f64,
    pub navigation_beacon: Option<EntityId>,
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
    pub departure_mass_kg: f64,
    pub distance_ly: f64,
    pub consumed_fuel_g: f64,
    pub navigation_beacon: Option<EntityId>,
    pub beacon_lost: bool,
    pub departure_error: [f64; 2],
    pub beacon_loss_error: Option<[f64; 2]>,
    pub intended_capture: Option<CelestialRef>,
    pub risk_target: Option<GalacticPosition>,
    pub capture_radius_m: f64,
    pub planned_log_loss: f64,
}

impl Transit {
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
        + Duration::from_seconds(tick(world) as f64 * 0.1)
}

fn slip_flight_seconds(
    origin: GalacticPosition,
    destination: GalacticPosition,
    speed_ly_s: f64,
) -> f64 {
    origin.relative_to(destination).length() / (speed_ly_s * math::LY_M)
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
    let fraction = world
        .get::<SlipDrive>(ship)
        .map_or(0.0, |drive| drive.fuel_fraction_g);
    (grams as f64 - fraction).max(0.0)
}

fn consume_fuel(world: &mut World, ship: Entity, grams: f64) {
    let Some(index) = fuel_index(world) else {
        return;
    };
    let fraction = world
        .get::<SlipDrive>(ship)
        .map_or(0.0, |drive| drive.fuel_fraction_g);
    let total = fraction + grams.max(0.0);
    let whole = total.floor() as u64;
    let paid = if let Some(mut inventory) = world.get_mut::<ShipInventory>(ship)
        && let Some(quantity) = inventory.0.quantities.get_mut(index)
    {
        let paid = whole.min(*quantity);
        *quantity -= paid;
        paid
    } else {
        0
    };
    if let Some(mut drive) = world.get_mut::<SlipDrive>(ship) {
        drive.fuel_fraction_g = if paid == whole {
            total - whole as f64
        } else {
            0.0
        };
    }
}

pub fn prepare_slip(
    world: &mut World,
    ship: Entity,
    destination: GalacticPosition,
    speed_ly_s: f64,
    navigation_beacon: Option<EntityId>,
) -> Result<()> {
    osg_protocol::validate_order(&Order::Slip {
        destination: Destination::Galactic(destination),
        speed_ly_s,
        navigation_beacon,
    })?;
    ensure!(active(world, ship), "slip requires a ship in space");
    let pose = ship_pose(world, ship)?;
    let radius = radius(world, ship)?;
    ensure!(pose.position != destination, "slip direction is undefined");
    ensure!(
        slip_admissible(world, ship, pose.position, radius),
        "inadmissible slip departure"
    );
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
        preparation.speed_ly_s = speed_ly_s;
        preparation.navigation_beacon = navigation_beacon;
        preparation.required_j = math::charging_energy_j(
            preparation.mass,
            destination.relative_to(pose.position).length() / math::LY_M,
        )
        .ceil();
    } else {
        drive.preparation = Some(Preparation {
            destination,
            speed_ly_s,
            navigation_beacon,
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
    let mut rng = rand::rng();
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
    (direction + across * error[0].tan() + up * error[1].tan()).normalize()
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
) -> Option<Capture> {
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
        for index in universe
            .index
            .containing_segment(origin, velocity * duration)
        {
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
        for (pose, celestial) in world
            .query::<(&PreciseTransform, &CelestialState)>()
            .iter(world)
        {
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
                        origin.relative_to(pose.translation_um),
                        velocity - celestial.velocity,
                        radius,
                        duration,
                    )
                {
                    consider(seconds, Some(celestial.reference), exclusion, physical);
                }
            }
        }
    }
    earliest
}

/// A bounded forecast records risk without constraining controller commands.
fn departure_forecast(
    world: &World,
    target: CelestialRef,
    origin: GalacticPosition,
    direction: DVec3,
    speed_ly_s: f64,
    assisted: bool,
    fuel_range_ly: f64,
) -> Option<(GalacticPosition, f64, f64)> {
    let universe = world.get_resource::<crate::sim::orrery::Universe>()?;
    let system = universe.resolve(target.system.0).ok()?;
    let body = system.body(target.body.0)?;
    if matches!(body.class_params, crate::sim::orrery::BodyClass::Barycenter) {
        return None;
    }
    let start_epoch = epoch(world);
    let current = system.solver.solve_position(&body.name, start_epoch)?;
    let duration = slip_flight_seconds(origin, current, speed_ly_s);
    if !duration.is_finite() || duration > MAX_PREDICTION_SECONDS {
        return None;
    }
    let center = system
        .solver
        .solve_position(&body.name, start_epoch + Duration::from_seconds(duration))?;
    let offset = center.relative_to(origin);
    let distance = offset.dot(direction);
    let exclusion = math::exclusion_radius_m(body.mass);
    let margin = (exclusion - (offset - direction * distance).length()).max(0.0);
    let log_loss =
        if distance <= 0.0 || distance / math::LY_M > fuel_range_ly || body.radius >= exclusion {
            f64::INFINITY
        } else {
            math::log_loss_from_ppm(math::capture_loss_ppm(
                margin, distance, speed_ly_s, assisted,
            ))
        };
    Some((center, margin, log_loss))
}

fn depart(world: &mut World, ship: Entity, preparation: &Preparation) -> Result<()> {
    let pose = ship_pose(world, ship)?;
    let mass = world
        .get::<MassProps>(ship)
        .context("ship mass unavailable")?
        .mass;
    let speed = preparation.speed_ly_s;
    let assisted = preparation.navigation_beacon.is_some_and(|beacon| {
        crate::sim::infrastructure::authenticated_navigation_beacon(world, ship, beacon)
    });
    ensure!(
        preparation.navigation_beacon.is_none() || assisted,
        "navigation beacon lost during charging"
    );
    let target = world.get::<Travel>(ship).and_then(|travel| {
        match &travel.0.orders.get(travel.0.order)?.action {
            Order::Slip {
                destination:
                    Destination::Relative {
                        reference: Reference::Celestial(reference),
                        ..
                    },
                ..
            } => Some(*reference),
            _ => None,
        }
    });
    let destination = preparation.destination;
    let aim = destination.relative_to(pose.position);
    ensure!(
        aim.is_finite() && aim.length_squared() > 0.0,
        "slip direction is undefined"
    );
    let direction = aim.normalize();
    let duration = slip_flight_seconds(pose.position, destination, speed);
    let forecast = target.and_then(|target| {
        departure_forecast(
            world,
            target,
            pose.position,
            direction,
            speed,
            assisted,
            math::exotic_range_ly(mass, fuel_grams(world, ship) * 0.001),
        )
    });
    let risk_target = forecast.map(|forecast| forecast.0);
    let capture_radius_m = forecast.map_or(0.0, |forecast| forecast.1);
    let log_loss = forecast.map_or(f64::INFINITY, |forecast| forecast.2);
    let sigma = math::dispersion_rad(speed, assisted);
    let departure_error = gaussian_pair().map(|sample| sample * sigma);
    let beacon_loss_error = assisted.then(|| {
        let blind = math::dispersion_rad(speed, false);
        let additional = (blind * blind - sigma * sigma).sqrt();
        gaussian_pair().map(|sample| sample * additional)
    });
    let direction = deflected(direction, departure_error);
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
        departure_mass_kg: mass,
        distance_ly: 0.0,
        consumed_fuel_g: 0.0,
        navigation_beacon: preparation.navigation_beacon,
        beacon_lost: false,
        departure_error,
        beacon_loss_error,
        intended_capture: target,
        risk_target,
        capture_radius_m,
        planned_log_loss: log_loss,
    });
    world.get_mut::<SlipDrive>(ship).unwrap().preparation = None;
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        travel.0.risk_budget.spent_log_loss += log_loss;
        travel.0.estimated_arrival_tick = duration
            .is_finite()
            .then(|| now.saturating_add((duration * 10.0).ceil() as u64));
    }
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
    let blind = math::dispersion_rad(transit.speed_ly_s, false);
    let assisted = math::dispersion_rad(transit.speed_ly_s, true);
    let additional = (blind * blind - assisted * assisted).sqrt();
    let error = transit
        .beacon_loss_error
        .expect("assisted transit stores its loss error");
    transit.direction = deflected(DVec3::from_array(transit.direction), error).to_array();
    transit.beacon_lost = true;
    if let Some(target) = transit.risk_target {
        let remaining = target.relative_to(transit.position).length();
        let total = target.relative_to(transit.origin).length();
        let variance = (total * assisted).powi(2) + (remaining * additional).powi(2);
        let loss_ppm = if variance > 0.0 {
            (-transit.capture_radius_m.powi(2) / (2.0 * variance)).exp() * 1e6
        } else {
            0.0
        };
        let revised = math::log_loss_from_ppm(loss_ppm);
        if revised > transit.planned_log_loss
            && let Some(mut travel) = world.get_mut::<Travel>(ship)
        {
            travel.0.risk_budget.spent_log_loss += revised - transit.planned_log_loss;
        }
    }
    emit(world, ship, "slip-beacon-lost", Some(transit.position));
}

fn arrive(world: &mut World, ship: Entity, transit: &Transit, capture: Capture) {
    world
        .get_mut::<PreciseTransform>(ship)
        .unwrap()
        .translation_um = transit.position;
    world.entity_mut(ship).remove::<Transit>();
    world
        .entity_mut(ship)
        .insert(Velocity(DVec3::from_array(transit.retained_velocity)));
    set_active(world, ship);
    if let Some(mut software) = world.get_mut::<crate::sim::vessel::ShipSoftware>(ship) {
        software.world_actions.clear();
    }
    world
        .entity_mut(ship)
        .insert(ArrivalOffset(capture.seconds));
    let now = tick(world);
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        let arrived_system = capture.body.is_some_and(|body| {
            matches!(travel.0.goals.first(), Some(Order::TravelToSystem(system)) if *system == body.system)
        });
        let intended = capture.body == transit.intended_capture || arrived_system;
        if intended {
            let remaining_goals = travel.0.goals.len();
            complete_order(&mut travel.0, now);
            if arrived_system && travel.0.goals.len() == remaining_goals {
                travel.0.goals.remove(0);
            }
        } else {
            let index = travel.0.order;
            if let Some(stage) = travel.0.orders.get_mut(index) {
                if let Order::Slip { destination, .. } = &stage.action {
                    stage.action = Order::TravelTo(destination.clone());
                    stage.label = stage.action.label();
                }
            }
        }
        if travel.0.order < travel.0.orders.len() && travel.0.autopilot_enabled {
            travel.0.status = Status::Planning;
            travel.0.estimated_arrival_tick = None;
        }
    }
    emit(world, ship, "slip-arrived", Some(transit.position));
}

fn advance_transit(world: &mut World, ship: Entity, mut transit: Transit) {
    let now = tick(world);
    if transit.advanced_tick > now {
        return;
    }
    lose_beacon(world, ship, &mut transit);
    let fuel = fuel_grams(world, ship);
    let paid_kg = transit.consumed_fuel_g * 0.001;
    let max_distance = math::exotic_range_ly(transit.departure_mass_kg, paid_kg + fuel * 0.001);
    let seconds_left = ((max_distance - transit.distance_ly) / transit.speed_ly_s).max(0.0);
    let duration = 0.1_f64.min(seconds_left);
    let velocity = DVec3::from_array(transit.direction) * transit.speed_ly_s * math::LY_M;
    let capture = first_capture(
        world,
        transit.position,
        velocity,
        duration,
        epoch(world),
        radius(world, ship).unwrap_or(0.0),
    );
    let elapsed = capture.map_or(duration, |capture| capture.seconds);
    transit.position = transit.position.offset_by(velocity * elapsed);
    transit.distance_ly += transit.speed_ly_s * elapsed;
    let cumulative = math::exotic_fuel_kg(transit.departure_mass_kg, transit.distance_ly) * 1000.0;
    consume_fuel(
        world,
        ship,
        (cumulative - transit.consumed_fuel_g).min(fuel),
    );
    transit.consumed_fuel_g = cumulative;
    transit.advanced_tick = now + 1;
    world
        .get_mut::<PreciseTransform>(ship)
        .unwrap()
        .translation_um = transit.position;
    if let Some(capture) = capture {
        if capture.physical {
            destroy(world, ship);
            emit(world, ship, "slip-collision", Some(transit.position));
        } else {
            arrive(world, ship, &transit, capture);
        }
    } else if seconds_left <= 0.1 {
        destroy(world, ship);
        emit(world, ship, "slip-fuel-exhausted", Some(transit.position));
    } else {
        world.entity_mut(ship).insert(transit);
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
        if !active(world, ship)
            || mass > preparation.mass * 1.001
            || !slip_admissible(world, ship, pose.position, ship_radius)
        {
            cancel_pending(world, ship);
            blocked(world, ship, "Slip preparation invalidated".into());
            continue;
        }
        preparation.required_j = math::charging_energy_j(
            preparation.mass,
            preparation.destination.relative_to(pose.position).length() / math::LY_M,
        )
        .ceil();
        let requested = (power * 0.1)
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
            .insert(SlipChargingPower(paid as f64 * 10.0));
        crate::sim::hardware::add_travel_heat(world, ship, paid as f64 * 0.2, 0.1);
        let mut drive = world.get_mut::<SlipDrive>(ship).unwrap();
        let stored = drive.preparation.as_mut().unwrap();
        stored.required_j = preparation.required_j;
        stored.work_j += paid as f64;
        let work = preparation.work_j + paid as f64;
        if work >= preparation.required_j && now >= preparation.started + 100 {
            if let Err(error) = depart(world, ship, &preparation) {
                cancel_pending(world, ship);
                blocked(world, ship, error.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::orrery::orrery_cfg::{Body, BodyClass};
    use std::sync::{Arc, OnceLock};

    fn fixture(fuel_g: u64) -> (World, Entity) {
        static CATALOGUE: OnceLock<osg_ships::Catalogue> = OnceLock::new();
        static DESIGN: OnceLock<Arc<osg_ships::CompiledShipDesign>> = OnceLock::new();
        let catalogue = CATALOGUE.get_or_init(osg_ships::Catalogue::builtin);
        let design =
            DESIGN.get_or_init(|| Arc::new(osg_ships::armed_starter().compile(catalogue).unwrap()));
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
        world.init_resource::<identity::IdentityIndex>();
        world.insert_resource(ShipCatalogue(catalogue.clone()));
        let ship = world
            .spawn((
                ShipDesign(design.clone()),
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::NEG_X * 1000.0),
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
        identity::register(&mut world, ship, Id::new());
        (world, ship)
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
        reference
    }

    fn transit(
        position: GalacticPosition,
        speed_ly_s: f64,
        target: Option<CelestialRef>,
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
            departure_mass_kg: 1000.0,
            distance_ly: 0.0,
            consumed_fuel_g: 0.0,
            navigation_beacon: None,
            beacon_lost: false,
            departure_error: [0.0; 2],
            beacon_loss_error: None,
            intended_capture: target,
            risk_target: Some(GalacticPosition::ZERO),
            capture_radius_m: 100.0,
            planned_log_loss: 0.0,
        }
    }

    #[test]
    fn retargeting_charge_scales_energy_with_distance_without_resetting_paid_work() {
        let (mut world, ship) = fixture(1000);
        let origin = ship_pose(&world, ship).unwrap().position;
        prepare_slip(
            &mut world,
            ship,
            origin.offset_by(DVec3::X * math::LY_M),
            0.01,
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
            0.01,
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
    fn charge_only_departure_captures_between_tick_endpoints_and_retains_velocity() {
        let (mut world, ship) = fixture(1000);
        natural_body(&mut world, GalacticPosition::ZERO, 10.0);
        prepare_slip(
            &mut world,
            ship,
            GalacticPosition::ZERO,
            20_000.0 / math::LY_M,
            None,
        )
        .unwrap();
        for tick in 0..100 {
            world.resource_mut::<SimulationCounters>().ticks = tick;
            advance(&mut world);
            assert!(world.get::<Transit>(ship).is_none());
            assert!(world.get::<SlipDrive>(ship).unwrap().preparation.is_some());
        }
        world.resource_mut::<SimulationCounters>().ticks = 100;
        advance(&mut world);
        assert!(world.get::<SlipDrive>(ship).unwrap().preparation.is_none());
        assert!(world.get::<Transit>(ship).is_none());
        assert_eq!(world.get::<PresenceState>(ship).unwrap().0, Presence::Space);
        assert_eq!(world.get::<Velocity>(ship).unwrap().0, DVec3::Y * 7.0);
        assert!((world.get::<ArrivalOffset>(ship).unwrap().0 - 0.045).abs() < 1e-7);
        let position = world.get::<PreciseTransform>(ship).unwrap().translation_um;
        assert!((position.to_meters_64().length() - 100.0).abs() < 1e-3);
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
        let expected = math::exotic_range_ly(flight.departure_mass_kg, 0.001) * math::LY_M;
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
    fn cumulative_fuel_keeps_fractional_grams_across_ticks_and_jumps() {
        let (mut world, ship) = fixture(1000);
        let mut previous = 0.0;
        for distance in [0.01, 0.03, 0.1, 0.2] {
            let cumulative = math::exotic_fuel_kg(1000.0, distance) * 1000.0;
            consume_fuel(&mut world, ship, cumulative - previous);
            previous = cumulative;
        }
        let remaining = fuel_grams(&world, ship);
        let (mut other, other_ship) = fixture(1000);
        consume_fuel(&mut other, other_ship, previous);
        assert!((remaining - fuel_grams(&other, other_ship)).abs() < 1e-12);
        consume_fuel(&mut world, ship, 0.25);
        assert!((fuel_grams(&world, ship) - remaining + 0.25).abs() < 1e-12);
    }

    #[test]
    fn beacon_loss_is_sampled_once_and_survives_serialization() {
        let (mut world, ship) = fixture(1000);
        let mut flight = transit(
            GalacticPosition::from_meters(DVec3::NEG_X * math::LY_M),
            0.05,
            None,
        );
        flight.navigation_beacon = Some(Id::new());
        flight.beacon_loss_error = Some([1e-6, -2e-6]);
        let before_loss = postcard::to_stdvec(&flight).unwrap();
        lose_beacon(&mut world, ship, &mut flight);
        let saved = postcard::to_stdvec(&flight).unwrap();
        let mut restored: Transit = postcard::from_bytes(&before_loss).unwrap();
        lose_beacon(&mut world, ship, &mut restored);
        lose_beacon(&mut world, ship, &mut restored);
        assert_eq!(postcard::to_stdvec(&restored).unwrap(), saved);
        assert!(restored.beacon_lost);
        assert!(restored.beacon_loss_error.is_some());
        assert_eq!(restored.speed_ly_s, 0.05);
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
        let primary = universe.systems[0].primary;
        let reference = CelestialRef {
            system: Id(primary.system),
            body: Id(primary.local),
        };
        world.insert_resource(universe);
        world.get_mut::<Travel>(ship).unwrap().0.orders = vec![
            Order::Slip {
                destination: Destination::Relative {
                    reference: Reference::Celestial(reference),
                    offset: GalacticPosition::ZERO,
                    axes: Axes::Galactic,
                },
                speed_ly_s: 1.0,
                navigation_beacon: None,
            }
            .into(),
        ];
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
        identity::register(&mut world, beacon, beacon_id);
        let origin = world.get::<PreciseTransform>(ship).unwrap().translation_um;
        // The controller aims away from its queued star, beyond its prediction horizon.
        // Neither the route's risk budget nor inadequate range prevents commitment.
        let commanded = origin.offset_by(DVec3::NEG_X * 1e10 * math::LY_M);
        prepare_slip(&mut world, ship, commanded, 1.0, Some(beacon_id)).unwrap();
        let preparation = world
            .get::<SlipDrive>(ship)
            .unwrap()
            .preparation
            .clone()
            .unwrap();
        depart(&mut world, ship, &preparation).unwrap();
        let flight = world.get::<Transit>(ship).unwrap().clone();
        assert_eq!(flight.destination, commanded);
        assert_eq!(flight.intended_capture, Some(reference));
        assert_eq!(flight.navigation_beacon, Some(beacon_id));
        assert!(DVec3::from_array(flight.direction).dot(DVec3::NEG_X) > 0.99);
        assert!(flight.planned_log_loss.is_infinite());
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
