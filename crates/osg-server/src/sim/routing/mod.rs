mod graph;

#[cfg(test)]
mod tests;

use anyhow::{Result, ensure};
use bevy::math::DVec3;
use osg_model::{
    Beacon, GalacticPosition, Id, Pose,
    travel::{FuelBudget, Order, PlanningPreferences, QueuedOrder},
};

use osg_model::transfer::TransferCost;

pub const MAX_WORK: u64 = 2_400_000_000;
pub const ENVIRONMENT_WORK: u64 = 4096;
pub const MAX_ORDERS: usize = 256;

#[derive(Clone, Debug)]
pub struct FuelRate {
    pub resource: String,
    pub kg_s: f64,
    pub available_kg: f64,
}

#[derive(Clone, Debug)]
pub struct ShipPerformance {
    pub radius_m: f64,
    pub mass_kg: f64,
    pub acceleration_m_s2: f64,
    pub propellant_kg_s: f64,
    pub turn_s: f64,
    pub slip_power_w: f64,
    pub exotic_available_kg: f64,
    pub fuels: Vec<FuelRate>,
}

#[derive(Clone, Debug)]
pub struct RouteRequest {
    pub origin: Pose,
    pub performance: ShipPerformance,
    pub preferences: PlanningPreferences,
    pub tick: u64,
    pub orders: Vec<Order>,
    pub docked_at: Option<Id>,
}

#[derive(Clone, Debug)]
pub struct CaptureTarget {
    pub reference: osg_model::travel::CelestialRef,
    pub pose: Pose,
    pub radius_m: f64,
    pub surface_radius_m: f64,
    pub navigation_beacon: Option<Id>,
}

#[derive(Clone, Copy, Debug)]
pub struct SlipEstimate {
    pub ready: bool,
    pub preparation_s: f64,
    pub duration_s: f64,
}

pub trait RouteEnvironment: Send + Sync {
    fn candidates(
        &self,
        origin: GalacticPosition,
        goal: GalacticPosition,
        limit: usize,
    ) -> Result<Vec<CaptureTarget>>;
    fn capture_target(
        &self,
        destination: &osg_model::travel::Destination,
        after_s: f64,
    ) -> Result<Option<CaptureTarget>>;
    fn departure(&self, origin: &Pose, toward: GalacticPosition, after_s: f64) -> Result<Pose>;
    fn resolve(&self, destination: &osg_model::travel::Destination, after_s: f64) -> Result<Pose>;
    fn beacon(&self, id: Id) -> Result<Beacon>;
    fn contact(&self, reference: osg_model::ContactRef) -> Result<(Pose, f64)>;
    fn slip(
        &self,
        origin: GalacticPosition,
        destination: GalacticPosition,
        departure_after_s: f64,
        arrival_after_s: f64,
        speed_ly_s: f64,
    ) -> Result<SlipEstimate>;
    fn cancelled(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct RoutePlan {
    pub orders: Vec<QueuedOrder>,
    pub fuel_budget: FuelBudget,
    pub work: u64,
    pub estimated_loss_ppm: f64,
    pub beacon_assumptions: Vec<Id>,
    pub exotic_fuel_kg: f64,
}

struct Work {
    spent: u64,
    limit: u64,
    deadline: std::time::Instant,
}

impl Work {
    fn charge(&mut self, count: u64, environment: &impl RouteEnvironment) -> Result<()> {
        if count > self.limit.saturating_sub(self.spent) {
            self.spent = self.limit;
            anyhow::bail!("route computation work limit exceeded");
        }
        self.spent += count;
        ensure!(!environment.cancelled(), "route computation cancelled");
        ensure!(
            std::time::Instant::now() < self.deadline,
            "route search time budget exhausted"
        );
        Ok(())
    }
}

fn transfer(
    performance: &ShipPerformance,
    weights: TransferCost,
    origin: &Pose,
    destination: &Pose,
) -> (f64, f64) {
    let offset = destination.position.relative_to(origin.position);
    let velocity = DVec3::from_array(origin.velocity) - DVec3::from_array(destination.velocity);
    if offset.length() <= 2.0 && velocity.length() <= 0.5 {
        return (0.0, 0.0);
    }
    if performance.acceleration_m_s2 <= 0.0 {
        return (f64::INFINITY, f64::INFINITY);
    }
    let braking_s = velocity.length() / performance.acceleration_m_s2;
    let stopping_offset = velocity * (0.5 * braking_s);
    if velocity.length_squared() > 0.0 && stopping_offset.length() >= offset.length() {
        let remaining = weights.estimate(
            (offset - stopping_offset).length(),
            performance.acceleration_m_s2,
            performance.propellant_kg_s,
        );
        return (
            braking_s + remaining.0 + 2.0 * performance.turn_s,
            braking_s * performance.propellant_kg_s + remaining.1,
        );
    }
    let direction = offset.normalize_or_zero();
    let closing = velocity.dot(direction);
    let lateral = (velocity - direction * closing).length();
    let (time, fuel) = weights.remaining(
        offset.length(),
        closing,
        performance.acceleration_m_s2,
        performance.propellant_kg_s,
    );
    let correction = lateral / performance.acceleration_m_s2;
    let turns = if offset.length() > 2.0 || velocity.length() > 0.5 {
        2.0 * performance.turn_s
    } else {
        0.0
    };
    (
        time + correction + turns,
        fuel + correction * performance.propellant_kg_s,
    )
}

pub fn work_limit(request: &RouteRequest, candidate_count: usize) -> u64 {
    if request
        .orders
        .iter()
        .any(|order| matches!(order, Order::TravelTo(_) | Order::Dock(_)))
    {
        MAX_WORK
    } else {
        (1_000 + candidate_count as u64 * 2 + request.orders.len() as u64 * 100 * ENVIRONMENT_WORK)
            .saturating_mul(24)
            .min(MAX_WORK)
    }
}

pub fn plan(request: &RouteRequest, environment: &impl RouteEnvironment) -> Result<RoutePlan> {
    plan_metered(request, environment).0
}

pub fn plan_metered(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
) -> (Result<RoutePlan>, u64) {
    let mut work = Work {
        spent: 0,
        limit: work_limit(request, 0),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
    };
    let result = plan_budgeted(request, environment, &mut work);
    (result, work.spent)
}

fn plan_budgeted(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
    work: &mut Work,
) -> Result<RoutePlan> {
    let acceptable = |plan: &RoutePlan| {
        plan.fuel_budget.resources.iter().all(|resource| {
            resource.required_kg <= resource.available_kg * request.preferences.fuel_fraction + 1e-9
        })
    };
    let fastest = plan_inner(
        request,
        environment,
        work,
        TransferCost { seconds_per_kg: 0. },
    )?;
    if acceptable(&fastest) {
        return Ok(fastest);
    }
    let ratio = fastest
        .fuel_budget
        .resources
        .iter()
        .map(|resource| {
            resource.required_kg
                / (resource.available_kg * request.preferences.fuel_fraction).max(1e-9)
        })
        .fold(1_f64, f64::max);
    let mut high = ((ratio * ratio - 1.) / (2. * request.performance.propellant_kg_s.max(1e-9)))
        .max(3600. / request.performance.mass_kg.max(1.));
    for _ in 0..8 {
        let mut candidate = plan_inner(
            request,
            environment,
            work,
            TransferCost {
                seconds_per_kg: high,
            },
        )?;
        if acceptable(&candidate) {
            candidate.work = work.spent;
            return Ok(candidate);
        }
        high *= 2.;
    }
    anyhow::bail!(
        "No route fits the selected fuel allowance; increase the percentage or change the travel options"
    )
}

fn plan_inner(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
    work: &mut Work,
    cost: TransferCost,
) -> Result<RoutePlan> {
    use osg_model::travel::*;

    let performance = &request.performance;
    ensure!(request.preferences.valid(), "invalid route preferences");
    ensure!(
        request.orders.len() <= MAX_ORDERS,
        "invalid route order count"
    );
    ensure!(
        [
            performance.radius_m,
            performance.mass_kg,
            performance.acceleration_m_s2,
            performance.propellant_kg_s,
            performance.turn_s,
            performance.slip_power_w,
            performance.exotic_available_kg,
        ]
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0)
            && performance.mass_kg > 0.0,
        "invalid ship performance"
    );
    ensure!(
        performance.fuels.len() <= 256
            && performance.fuels.iter().all(|fuel| {
                !fuel.resource.is_empty()
                    && fuel.resource.len() <= 64
                    && fuel.kg_s.is_finite()
                    && fuel.kg_s >= 0.0
                    && fuel.available_kg.is_finite()
                    && fuel.available_kg >= 0.0
            }),
        "invalid fuel observations"
    );
    work.charge(1, environment)?;
    let mut builder = Builder {
        cost,
        request,
        environment,
        work,
        pose: request.origin.clone(),
        elapsed_s: 0.0,
        docked_at: request.docked_at,
        orders: Vec::new(),
        complete: true,
        log_loss: 0.0,
        exotic_fuel_kg: 0.0,
        beacon_assumptions: Vec::new(),
        risk_routes_remaining: request
            .orders
            .iter()
            .filter(|order| {
                matches!(
                    order,
                    Order::TravelTo(_) | Order::Dock(_) | Order::Slip { .. }
                )
            })
            .count(),
    };
    for (index, order) in request.orders.iter().enumerate() {
        osg_protocol::validate_order(order)?;
        if !matches!(order, Order::Undock | Order::WaitUntil(_)) && builder.docked_at.is_some() {
            builder.undock()?;
        }
        match order {
            Order::TravelTo(destination) => builder.route(destination.clone(), false)?,
            Order::Dock(station) => builder.route(Destination::Beacon(*station), true)?,
            Order::Sublight(destination) => builder.sublight(destination.clone())?,
            Order::Slip {
                destination,
                speed_ly_s,
                navigation_beacon,
            } => builder.slip(destination.clone(), *speed_ly_s, *navigation_beacon, None)?,
            Order::Undock => {
                if builder.docked_at.is_some() {
                    builder.undock()?;
                }
            }
            Order::WaitUntil(until) => {
                let current_tick = request
                    .tick
                    .saturating_add((builder.elapsed_s * 10.0).ceil() as u64);
                let seconds = until.saturating_sub(current_tick) as f64 * 0.1;
                builder.append(Order::WaitUntil(*until), seconds, 0.0)?;
                builder.pose.position = builder
                    .pose
                    .position
                    .offset_by(DVec3::from_array(builder.pose.velocity) * seconds);
            }
            Order::Guidance(guidance) => {
                if guidance.mode == GuidanceMode::Align {
                    builder.append(order.clone(), performance.turn_s, 0.0)?;
                } else if guidance.mode == GuidanceMode::Approach
                    && let Target::Destination(destination) = &guidance.target
                {
                    builder.guided_approach(destination.clone(), guidance.range_m)?;
                } else if guidance.mode == GuidanceMode::Approach
                    && let Target::Contact(reference) = guidance.target
                {
                    builder.contact_approach(reference, guidance.range_m)?;
                } else {
                    ensure!(
                        index + 1 == request.orders.len(),
                        "continuous guidance must be the final route order"
                    );
                    builder.orders.push(order.clone().into());
                    builder.complete = false;
                }
            }
        }
        if matches!(
            order,
            Order::TravelTo(_) | Order::Dock(_) | Order::Slip { .. }
        ) {
            builder.risk_routes_remaining = builder.risk_routes_remaining.saturating_sub(1);
        }
    }
    let required = builder
        .orders
        .iter()
        .filter_map(|order| order.estimated_propellant_kg)
        .sum::<f64>();
    let total_flow = performance.fuels.iter().map(|fuel| fuel.kg_s).sum::<f64>();
    let mut resources: Vec<_> = performance
        .fuels
        .iter()
        .map(|fuel| FuelRequirement {
            resource: fuel.resource.clone(),
            required_kg: if total_flow > 0.0 {
                required * fuel.kg_s / total_flow
            } else {
                0.0
            },
            available_kg: fuel.available_kg,
        })
        .collect();
    resources.push(FuelRequirement {
        resource: slip::EXOTIC_RESOURCE.to_owned(),
        required_kg: builder.exotic_fuel_kg,
        available_kg: performance.exotic_available_kg,
    });
    let fuel_budget = FuelBudget {
        resources,
        complete: builder.complete && (total_flow > 0.0 || required == 0.0),
    };
    ensure!(fuel_budget.valid(), "route fuel estimate overflow");
    Ok(RoutePlan {
        orders: builder.orders,
        fuel_budget,
        work: builder.work.spent,
        estimated_loss_ppm: slip::ppm_from_log_loss(builder.log_loss),
        beacon_assumptions: builder.beacon_assumptions,
        exotic_fuel_kg: builder.exotic_fuel_kg,
    })
}

struct Builder<'a, E> {
    cost: TransferCost,
    request: &'a RouteRequest,
    environment: &'a E,
    work: &'a mut Work,
    pose: Pose,
    elapsed_s: f64,
    docked_at: Option<Id>,
    orders: Vec<QueuedOrder>,
    complete: bool,
    log_loss: f64,
    exotic_fuel_kg: f64,
    beacon_assumptions: Vec<Id>,
    risk_routes_remaining: usize,
}

impl<E: RouteEnvironment> Builder<'_, E> {
    fn resolve(
        &mut self,
        destination: &osg_model::travel::Destination,
        after: f64,
    ) -> Result<Pose> {
        ensure!(
            after.is_finite() && (0.0..=osg_model::travel::MAX_PREDICTION_SECONDS).contains(&after),
            "route exceeds prediction horizon"
        );
        self.work.charge(ENVIRONMENT_WORK, self.environment)?;
        self.environment.resolve(destination, after)
    }

    fn beacon(&mut self, id: Id) -> Result<Beacon> {
        self.work.charge(ENVIRONMENT_WORK, self.environment)?;
        self.environment.beacon(id)
    }

    fn append(&mut self, action: Order, seconds: f64, fuel: f64) -> Result<()> {
        ensure!(
            seconds.is_finite() && seconds >= 0.0 && fuel.is_finite() && fuel >= 0.0,
            "unreachable transfer with available propulsion"
        );
        ensure!(
            self.orders.len() < MAX_ORDERS,
            "expanded route exceeds 256 orders"
        );
        let mut order = QueuedOrder::estimated(action, seconds).with_propellant(fuel);
        order.transfer_cost = self.cost;
        self.orders.push(order);
        self.elapsed_s += seconds;
        Ok(())
    }

    fn undock(&mut self) -> Result<()> {
        ensure!(self.docked_at.is_some(), "ship is already in space");
        self.append(Order::Undock, 0.1, 0.0)?;
        self.docked_at = None;
        Ok(())
    }

    fn estimate_sublight(
        &mut self,
        destination: &osg_model::travel::Destination,
    ) -> Result<(Pose, (f64, f64))> {
        let initial_target = self.resolve(&destination, self.elapsed_s)?;
        let mut target = initial_target.clone();
        let mut estimate = transfer(&self.request.performance, self.cost, &self.pose, &target);
        for _ in 0..4 {
            let next = self.resolve(&destination, self.elapsed_s + estimate.0)?;
            let mut inertial_target = next.clone();
            inertial_target.position = next
                .position
                .offset_by(-DVec3::from_array(initial_target.velocity) * estimate.0);
            let next_estimate = transfer(
                &self.request.performance,
                self.cost,
                &self.pose,
                &inertial_target,
            );
            target = next;
            let converged = (next_estimate.0 - estimate.0).abs() < 0.01;
            estimate = next_estimate;
            if converged {
                break;
            }
        }
        Ok((target, estimate))
    }

    fn sublight(&mut self, destination: osg_model::travel::Destination) -> Result<()> {
        let (target, estimate) = self.estimate_sublight(&destination)?;
        if estimate.0 > 0.0 || estimate.1 > 0.0 {
            self.append(Order::Sublight(destination), estimate.0, estimate.1)?;
        }
        self.pose = target;
        Ok(())
    }

    fn slip(
        &mut self,
        destination: osg_model::travel::Destination,
        mut speed_ly_s: f64,
        navigation_beacon: Option<Id>,
        max_leg_loss_ppm: Option<f64>,
    ) -> Result<()> {
        use osg_model::travel::slip;
        ensure!(
            self.request.preferences.allow_slipdrive,
            "Slipdrive disabled in route preferences"
        );
        ensure!(
            self.request.preferences.max_loss_ppm > 0.0,
            "zero destruction risk excludes slip travel"
        );
        ensure!(
            self.request.performance.slip_power_w > 0.0,
            "ship has no available slipdrive"
        );
        let mut preparation_s = 0.0;
        let mut duration_s = 0.0;
        for _ in 0..8 {
            let departure = self.elapsed_s + preparation_s;
            let arrival = departure + duration_s;
            let origin = self
                .pose
                .position
                .offset_by(DVec3::from_array(self.pose.velocity) * preparation_s);
            let target = self.resolve(&destination, arrival)?;
            if let Some(max_loss_ppm) = max_leg_loss_ppm {
                let capture = self
                    .environment
                    .capture_target(&destination, arrival)?
                    .ok_or_else(|| anyhow::anyhow!("slip aim has no natural capture target"))?;
                ensure!(
                    capture.radius_m > capture.surface_radius_m + self.request.performance.radius_m,
                    "target surface extends into its capture boundary"
                );
                speed_ly_s = slip::fastest_speed_ly_s(
                    capture.radius_m,
                    target.position.relative_to(origin).length(),
                    max_loss_ppm,
                    navigation_beacon.is_some(),
                )
                .ok_or_else(|| anyhow::anyhow!("moving target exceeds capture floor"))?;
            }
            self.work.charge(ENVIRONMENT_WORK, self.environment)?;
            let estimate =
                self.environment
                    .slip(origin, target.position, departure, arrival, speed_ly_s)?;
            ensure!(
                estimate.preparation_s.is_finite()
                    && estimate.preparation_s >= 0.0
                    && estimate.duration_s.is_finite()
                    && estimate.duration_s >= 0.0,
                "invalid slip prediction"
            );
            let converged = (estimate.preparation_s - preparation_s).abs() < 0.001
                && (estimate.duration_s - duration_s).abs() < 0.001;
            preparation_s = estimate.preparation_s;
            duration_s = estimate.duration_s;
            if converged {
                ensure!(
                    estimate.ready,
                    "slip departure is inside a natural exclusion"
                );
                let capture = self
                    .environment
                    .capture_target(&destination, arrival)?
                    .ok_or_else(|| anyhow::anyhow!("slip aim has no natural capture target"))?;
                ensure!(
                    navigation_beacon.is_none() || navigation_beacon == capture.navigation_beacon,
                    "navigation beacon unavailable for this capture"
                );
                let distance = target.position.relative_to(origin).length();
                let loss_ppm = slip::capture_loss_ppm(
                    capture.radius_m,
                    distance,
                    speed_ly_s,
                    navigation_beacon.is_some(),
                );
                self.log_loss += slip::log_loss_from_ppm(loss_ppm);
                ensure!(
                    self.log_loss
                        <= slip::log_loss_from_ppm(self.request.preferences.max_loss_ppm)
                            * (1.0 + 1e-9),
                    "slip itinerary exceeds destruction risk allowance"
                );
                self.exotic_fuel_kg +=
                    slip::exotic_fuel_kg(self.request.performance.mass_kg, distance / slip::LY_M);
                if let Some(beacon) = navigation_beacon {
                    if !self.beacon_assumptions.contains(&beacon) {
                        self.beacon_assumptions.push(beacon);
                    }
                }
                self.append(
                    Order::Slip {
                        destination,
                        speed_ly_s,
                        navigation_beacon,
                    },
                    preparation_s + duration_s,
                    0.0,
                )?;
                let outward = origin.relative_to(target.position).normalize_or_zero();
                self.pose.position = target.position.offset_by(outward * capture.radius_m);
                self.docked_at = None;
                return Ok(());
            }
        }
        anyhow::bail!("moving slip endpoint did not converge within prediction budget")
    }

    fn contact_approach(&mut self, reference: osg_model::ContactRef, range_m: f64) -> Result<()> {
        use osg_model::travel::{Guidance, GuidanceMode, Target};

        self.work.charge(ENVIRONMENT_WORK, self.environment)?;
        let (observed, radius) = self.environment.contact(reference)?;
        let range_m = range_m.max(radius + self.request.performance.radius_m + 2.0);
        let target_at = |elapsed: f64, origin: GalacticPosition| {
            let mut target = observed.clone();
            target.position = observed
                .position
                .offset_by(DVec3::from_array(observed.velocity) * elapsed);
            let outward = origin
                .relative_to(target.position)
                .try_normalize()
                .unwrap_or(DVec3::Z);
            target.position = target.position.offset_by(outward * range_m);
            target
        };
        let mut target = target_at(self.elapsed_s, self.pose.position);
        let estimate = transfer(&self.request.performance, self.cost, &self.pose, &target);
        self.append(
            Order::Guidance(Guidance {
                mode: GuidanceMode::Approach,
                target: Target::Contact(reference),
                range_m,
            }),
            estimate.0,
            estimate.1,
        )?;
        target.position = target
            .position
            .offset_by(DVec3::from_array(target.velocity) * estimate.0);
        self.pose = target;
        Ok(())
    }

    fn estimate_approach(
        &mut self,
        destination: &osg_model::travel::Destination,
        range_m: f64,
    ) -> Result<(Pose, (f64, f64))> {
        use osg_model::travel::{Axes, Destination, Reference};

        let centre = self.resolve(destination, self.elapsed_s)?;
        let outward = self
            .pose
            .position
            .relative_to(centre.position)
            .try_normalize()
            .unwrap_or(DVec3::Z);
        let endpoint = match destination {
            Destination::Beacon(id) => Destination::Relative {
                reference: Reference::Beacon(*id),
                offset: GalacticPosition::from_meters(outward * range_m),
                axes: Axes::Galactic,
            },
            _ => Destination::Galactic(centre.position.offset_by(outward * range_m)),
        };
        self.estimate_sublight(&endpoint)
    }

    fn guided_approach(
        &mut self,
        destination: osg_model::travel::Destination,
        range_m: f64,
    ) -> Result<()> {
        use osg_model::travel::{Guidance, GuidanceMode, Target};

        let (pose, estimate) = self.estimate_approach(&destination, range_m)?;
        self.append(
            Order::Guidance(Guidance {
                mode: GuidanceMode::Approach,
                target: Target::Destination(destination),
                range_m,
            }),
            estimate.0,
            estimate.1,
        )?;
        self.pose = pose;
        Ok(())
    }

    fn arrive_at_beacon(&mut self, beacon: &Beacon, dock: bool) -> Result<()> {
        use osg_model::travel::Destination;

        let destination = Destination::Beacon(beacon.entity);
        let range_m =
            beacon.radius_m + self.request.performance.radius_m + if dock { 50.0 } else { 100.0 };
        if dock {
            let (pose, estimate) = self.estimate_approach(&destination, range_m)?;
            self.append(Order::Dock(beacon.entity), estimate.0 + 0.1, estimate.1)?;
            self.pose = pose;
        } else {
            self.guided_approach(destination, range_m)?;
        }
        Ok(())
    }

    fn route(&mut self, mut destination: osg_model::travel::Destination, dock: bool) -> Result<()> {
        use osg_model::travel::{Axes, Destination, Reference};

        let station = if let Destination::Beacon(id) = destination {
            let beacon = self.beacon(id)?;
            if dock {
                ensure!(!beacon.bays.is_empty(), "no accessible docking berth");
            }
            let clearance = beacon.radius_m
                + self.request.performance.radius_m
                + if dock { 50.0 } else { 100.0 };
            let outward = self
                .pose
                .position
                .relative_to(beacon.pose.position)
                .try_normalize()
                .unwrap_or(DVec3::Z);
            destination = Destination::Relative {
                reference: Reference::Beacon(id),
                offset: GalacticPosition::from_meters(outward * clearance),
                axes: Axes::Galactic,
            };
            Some(beacon)
        } else {
            ensure!(!dock, "docking destination needs a public station beacon");
            None
        };
        let target = self.resolve(&destination, self.elapsed_s)?;
        let remaining =
            (osg_model::travel::slip::log_loss_from_ppm(self.request.preferences.max_loss_ppm)
                - self.log_loss)
                .max(0.0)
                / self.risk_routes_remaining.max(1) as f64;
        let path = graph::search(
            self.request,
            self.environment,
            self.work,
            self.cost,
            &self.pose,
            &target,
            &destination,
            self.elapsed_s,
            remaining,
        )?;
        for leg in path {
            for attempt in 0..16 {
                let departure = self.environment.departure(
                    &self.pose,
                    leg.target.pose.position,
                    self.elapsed_s,
                )?;
                if departure.position == self.pose.position {
                    break;
                }
                ensure!(attempt < 15, "departure clearance did not converge");
                self.sublight(Destination::Galactic(departure.position))?;
            }
            self.slip(
                leg.destination(),
                leg.speed_ly_s,
                leg.target.navigation_beacon,
                Some(leg.max_loss_ppm),
            )?;
        }
        if let Some(station) = &station {
            self.arrive_at_beacon(station, dock)?;
        } else {
            self.sublight(destination)?;
        }
        if dock {
            let station = station.unwrap();
            let pose = self.resolve(&Destination::Beacon(station.entity), self.elapsed_s)?;
            self.pose = pose;
            self.docked_at = Some(station.entity);
        }
        Ok(())
    }
}
