mod graph;

#[cfg(test)]
mod tests;

use anyhow::{Result, ensure};
use bevy::math::DVec3;
use toy_sim_model::{
    Beacon, GalacticPosition, Id, NavigationGate, Pose,
    travel::{FuelBudget, Order, PlanningPreferences, QueuedOrder},
};

pub const MAX_WORK: u64 = 200_000_000;
pub const GAS_PER_WORK: u64 = 100;
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
pub struct RouteGate {
    pub navigation: NavigationGate,
    pub aperture_radius_m: f64,
    pub exclusion_m: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct SlipEstimate {
    pub ready: bool,
    pub preparation_s: f64,
    pub duration_s: f64,
}

pub trait RouteEnvironment: Send + Sync {
    fn gates(&self) -> &[RouteGate];
    fn resolve(
        &self,
        destination: &toy_sim_model::travel::Destination,
        after_s: f64,
    ) -> Result<Pose>;
    fn beacon(&self, id: Id) -> Result<Beacon>;
    fn contact(&self, reference: toy_sim_model::ContactRef) -> Result<(Pose, f64)>;
    fn slip(
        &self,
        origin: GalacticPosition,
        destination: GalacticPosition,
        departure_after_s: f64,
        arrival_after_s: f64,
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
}

struct Work {
    spent: u64,
    limit: u64,
}

impl Work {
    fn charge(&mut self, count: u64, environment: &impl RouteEnvironment) -> Result<()> {
        if count > self.limit.saturating_sub(self.spent) {
            self.spent = self.limit;
            anyhow::bail!("route computation work limit exceeded");
        }
        self.spent += count;
        ensure!(!environment.cancelled(), "route computation cancelled");
        Ok(())
    }
}

fn transfer(
    performance: &ShipPerformance,
    preferences: PlanningPreferences,
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
    let weights = preferences.cost(performance.mass_kg);
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

fn slip_rendezvous(
    performance: &ShipPerformance,
    preferences: PlanningPreferences,
    origin: &Pose,
    destination: &Pose,
) -> (f64, f64) {
    let mut arrival = destination.clone();
    arrival.velocity = origin.velocity;
    transfer(performance, preferences, &arrival, destination)
}

pub fn work_limit(request: &RouteRequest, gate_count: usize) -> u64 {
    if request
        .orders
        .iter()
        .any(|order| matches!(order, Order::TravelTo(_) | Order::Dock(_)))
    {
        MAX_WORK
    } else {
        (1_000 + gate_count as u64 * 2 + request.orders.len() as u64 * 100 * ENVIRONMENT_WORK)
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
        limit: work_limit(request, environment.gates().len()),
    };
    let result = plan_inner(request, environment, &mut work);
    (result, work.spent)
}

fn plan_inner(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
    work: &mut Work,
) -> Result<RoutePlan> {
    use toy_sim_model::travel::*;

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
            performance.slip_power_w
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
        request,
        environment,
        work,
        pose: request.origin.clone(),
        elapsed_s: 0.0,
        docked_at: request.docked_at,
        orders: Vec::new(),
        complete: true,
    };
    for (index, order) in request.orders.iter().enumerate() {
        toy_sim_protocol::validate_order(order)?;
        if !matches!(order, Order::Undock | Order::WaitUntil(_)) && builder.docked_at.is_some() {
            builder.undock()?;
        }
        match order {
            Order::TravelTo(destination) => builder.route(destination.clone(), false)?,
            Order::Dock(station) => builder.route(Destination::Beacon(*station), true)?,
            Order::Sublight(destination) => builder.sublight(destination.clone())?,
            Order::Jump(entry) => builder.jump(*entry)?,
            Order::Slip { destination } => builder.slip(destination.clone())?,
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
    }
    let required = builder
        .orders
        .iter()
        .filter_map(|order| order.estimated_propellant_kg)
        .sum::<f64>();
    let total_flow = performance.fuels.iter().map(|fuel| fuel.kg_s).sum::<f64>();
    let resources = performance
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
    let fuel_budget = FuelBudget {
        resources,
        complete: builder.complete && (total_flow > 0.0 || required == 0.0),
    };
    ensure!(fuel_budget.valid(), "route fuel estimate overflow");
    Ok(RoutePlan {
        orders: builder.orders,
        fuel_budget,
        work: builder.work.spent,
    })
}

struct Builder<'a, E> {
    request: &'a RouteRequest,
    environment: &'a E,
    work: &'a mut Work,
    pose: Pose,
    elapsed_s: f64,
    docked_at: Option<Id>,
    orders: Vec<QueuedOrder>,
    complete: bool,
}

impl<E: RouteEnvironment> Builder<'_, E> {
    fn resolve(
        &mut self,
        destination: &toy_sim_model::travel::Destination,
        after: f64,
    ) -> Result<Pose> {
        ensure!(
            after.is_finite()
                && (0.0..=toy_sim_model::travel::MAX_PREDICTION_SECONDS).contains(&after),
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
        self.orders
            .push(QueuedOrder::estimated(action, seconds).with_propellant(fuel));
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
        destination: &toy_sim_model::travel::Destination,
    ) -> Result<(Pose, (f64, f64))> {
        let initial_target = self.resolve(&destination, self.elapsed_s)?;
        let mut target = initial_target.clone();
        let mut estimate = transfer(
            &self.request.performance,
            self.request.preferences,
            &self.pose,
            &target,
        );
        for _ in 0..4 {
            let next = self.resolve(&destination, self.elapsed_s + estimate.0)?;
            let mut inertial_target = next.clone();
            inertial_target.position = next
                .position
                .offset_by(-DVec3::from_array(initial_target.velocity) * estimate.0);
            let next_estimate = transfer(
                &self.request.performance,
                self.request.preferences,
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

    fn sublight(&mut self, destination: toy_sim_model::travel::Destination) -> Result<()> {
        let (target, estimate) = self.estimate_sublight(&destination)?;
        if estimate.0 > 0.0 || estimate.1 > 0.0 {
            self.append(Order::Sublight(destination), estimate.0, estimate.1)?;
        }
        self.pose = target;
        Ok(())
    }

    fn jump(&mut self, entry_id: Id) -> Result<()> {
        use toy_sim_model::travel::Destination;

        let entry = self.beacon(entry_id)?;
        let exit_id = entry
            .gate_exit
            .ok_or_else(|| anyhow::anyhow!("destination is not a gate"))?;
        let exit = self.beacon(exit_id)?;
        ensure!(exit.gate_exit == Some(entry_id), "gate pair unavailable");
        ensure!(
            entry.radius_m > self.request.performance.radius_m + 2.0
                && exit.radius_m > self.request.performance.radius_m + 2.0,
            "ship does not fit gate aperture"
        );
        let destination = self.resolve(&Destination::Beacon(entry_id), self.elapsed_s)?;
        let estimate = transfer(
            &self.request.performance,
            self.request.preferences,
            &self.pose,
            &destination,
        );
        self.append(Order::Jump(entry_id), estimate.0 + 0.1, estimate.1)?;
        self.pose = self.resolve(&Destination::Beacon(exit_id), self.elapsed_s)?;
        self.docked_at = None;
        Ok(())
    }

    fn slip(&mut self, destination: toy_sim_model::travel::Destination) -> Result<()> {
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
            self.work.charge(ENVIRONMENT_WORK, self.environment)?;
            let estimate = self
                .environment
                .slip(origin, target.position, departure, arrival)?;
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
                if !estimate.ready {
                    self.complete = false;
                }
                self.append(Order::Slip { destination }, preparation_s + duration_s, 0.0)?;
                self.pose.position = target.position;
                self.docked_at = None;
                return Ok(());
            }
        }
        anyhow::bail!("moving slip endpoint did not converge within prediction budget")
    }

    fn contact_approach(
        &mut self,
        reference: toy_sim_model::ContactRef,
        range_m: f64,
    ) -> Result<()> {
        use toy_sim_model::travel::{Guidance, GuidanceMode, Target};

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
        let estimate = transfer(
            &self.request.performance,
            self.request.preferences,
            &self.pose,
            &target,
        );
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
        destination: &toy_sim_model::travel::Destination,
        range_m: f64,
    ) -> Result<(Pose, (f64, f64))> {
        use toy_sim_model::travel::{Axes, Destination, Reference};

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
        destination: toy_sim_model::travel::Destination,
        range_m: f64,
    ) -> Result<()> {
        use toy_sim_model::travel::{Guidance, GuidanceMode, Target};

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
        use toy_sim_model::travel::Destination;

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

    fn route(
        &mut self,
        mut destination: toy_sim_model::travel::Destination,
        dock: bool,
    ) -> Result<()> {
        use toy_sim_model::travel::{Axes, Destination, Reference};

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
        self.work.charge(
            self.environment.gates().len() as u64 * 8 + 1,
            self.environment,
        )?;
        let mut graph = graph::Graph::new(
            self.pose.clone(),
            target,
            destination.clone(),
            self.environment.gates(),
            &self.request.performance,
        )?;
        let path = graph.search(self.request, self.environment, self.work)?;
        let mut escape_time = 0.0;
        let mut escape_fuel = 0.0;
        for edge in path {
            match edge.kind {
                graph::EdgeKind::Jump(index) => {
                    self.jump(self.environment.gates()[index].navigation.entity)?;
                    escape_time = 0.0;
                    escape_fuel = 0.0;
                }
                graph::EdgeKind::Sublight if edge.to != 1 => {
                    escape_time += edge.time_s;
                    escape_fuel += edge.fuel_kg;
                }
                graph::EdgeKind::Sublight => {
                    // The last local transfer is represented by the final strategic action.
                }
                graph::EdgeKind::Slip => {
                    let semantic = if edge.to == 1 {
                        station.as_ref().map_or_else(
                            || destination.clone(),
                            |beacon| Destination::Beacon(beacon.entity),
                        )
                    } else {
                        let gate = graph.nodes[edge.to]
                            .gate
                            .ok_or_else(|| anyhow::anyhow!("slip endpoint has no public anchor"))?;
                        Destination::Beacon(self.environment.gates()[gate].navigation.entity)
                    };
                    let rendezvous = slip_rendezvous(
                        &self.request.performance,
                        self.request.preferences,
                        &graph.nodes[edge.from].pose,
                        &graph.nodes[edge.to].pose,
                    );
                    self.append(
                        Order::Slip {
                            destination: semantic,
                        },
                        escape_time + (edge.time_s - rendezvous.0).max(0.0),
                        escape_fuel,
                    )?;
                    let velocity = self.pose.velocity;
                    self.pose = self.resolve(&graph.nodes[edge.to].destination, self.elapsed_s)?;
                    self.pose.velocity = velocity;
                    self.docked_at = None;
                    escape_time = 0.0;
                    escape_fuel = 0.0;
                }
            }
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
