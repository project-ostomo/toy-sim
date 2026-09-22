use super::*;
use osg_model::travel::{Axes, CelestialRef, Destination, Reference, slip};
use std::collections::{BTreeMap, BinaryHeap};

#[derive(Clone)]
pub(super) struct Leg {
    pub target: CaptureTarget,
}

impl Leg {
    pub fn destination(&self) -> Destination {
        Destination::Relative {
            reference: Reference::Celestial(self.target.reference),
            offset: GalacticPosition::default(),
            axes: Axes::Galactic,
        }
    }
}

struct State {
    pose: Pose,
    parent: Option<(usize, Leg)>,
    depth: usize,
    seconds: f64,
    fuel: f64,
    exotic: f64,
    loss: f64,
}

// Edges enter the queue using cheap catalogue estimates. Resolve clearance and
// forecast the transit only when an edge reaches the front of the queue.
struct Edge {
    priority: f64,
    parent: usize,
    target: CaptureTarget,
    departure: Option<(Pose, (f64, f64))>,
}

impl PartialEq for Edge {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}
impl Eq for Edge {}
impl PartialOrd for Edge {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Edge {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.priority.total_cmp(&self.priority)
    }
}

fn itinerary(states: &[State], mut index: usize) -> Vec<Leg> {
    let mut legs = Vec::new();
    while let Some((parent, leg)) = &states[index].parent {
        legs.push(leg.clone());
        index = *parent;
    }
    legs.reverse();
    legs
}

// Goal-directed estimate: the final capture's guidance determines its cruise speed.
// Assisted intermediate stops can still improve on this estimate.
fn remaining_seconds(position: GalacticPosition, goal: &Pose, assisted: bool) -> f64 {
    slip::flight_seconds(goal.position.relative_to(position).length(), assisted)
}

pub(super) fn search(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
    work: &mut Work,
    weights: TransferCost,
    origin: &Pose,
    goal: &Pose,
    destination: &Destination,
    arrival_system: Option<Id>,
    start_s: f64,
    remaining_log_loss: f64,
) -> Result<Vec<Leg>> {
    let performance = &request.performance;
    let local = transfer(performance, weights, origin, goal);
    let mut best_cost = local.0 + weights.seconds_per_kg * local.1;
    if arrival_system.is_some() || start_s + local.0 > osg_model::travel::MAX_PREDICTION_SECONDS {
        best_cost = f64::INFINITY;
    }
    let mut best = Vec::new();
    if !request.preferences.allow_slipdrive
        || performance.slip_power_w <= 0.0
        || remaining_log_loss <= 0.0
    {
        ensure!(
            best_cost.is_finite(),
            "destination unreachable with available propulsion"
        );
        return Ok(best);
    }

    let unlimited_risk = remaining_log_loss.is_infinite();
    let mut goal_targets = if let Some(system) = arrival_system {
        environment.system_targets(system, start_s)?
    } else {
        environment
            .capture_target(destination, start_s)?
            .into_iter()
            .collect()
    };
    goal_targets.retain(|target| target.radius_m > target.surface_radius_m + performance.radius_m);
    ensure!(
        arrival_system.is_none() || !goal_targets.is_empty(),
        "destination system has no safe slip capture body outside its surface"
    );
    let target = goal_targets
        .iter()
        .max_by(|a, b| a.radius_m.total_cmp(&b.radius_m))
        .cloned();
    let goal_reference = target.as_ref().map(|target| target.reference);
    let charge_seconds = |distance_m: f64| {
        (slip::charging_energy_j(performance.mass_kg, distance_m / slip::LY_M)
            / performance.slip_power_w)
            .max(slip::MIN_CHARGE_SECONDS)
    };
    let deadline = work.deadline - std::time::Duration::from_millis(500);
    let mut queue = BinaryHeap::new();
    let mut states = vec![State {
        pose: origin.clone(),
        parent: None,
        depth: 0,
        seconds: 0.0,
        fuel: 0.0,
        exotic: 0.0,
        loss: 0.0,
    }];
    let mut candidates: BTreeMap<Option<CelestialRef>, Vec<CaptureTarget>> = BTreeMap::new();
    let mut expand = Some(0);
    let mut forecasts = 0;
    let mut first_solution = None;
    let search_limit = work
        .limit
        .saturating_sub(ENVIRONMENT_WORK * MAX_ORDERS as u64 * 32);

    loop {
        ensure!(!environment.cancelled(), "route computation cancelled");
        if std::time::Instant::now() >= deadline
            || forecasts >= 512
            || work.spent + ENVIRONMENT_WORK >= search_limit
            || first_solution
                .is_some_and(|time: std::time::Instant| time.elapsed().as_millis() >= 100)
        {
            break;
        }
        if let Some(parent) = expand.take() {
            let state = &states[parent];
            if state.depth < 32 && (unlimited_risk || state.loss < remaining_log_loss) {
                let key = state.parent.as_ref().map(|(_, leg)| leg.target.reference);
                if !candidates.contains_key(&key) {
                    let mut neighbors =
                        environment.candidates(state.pose.position, goal.position, 96)?;
                    for target in &goal_targets {
                        neighbors.retain(|neighbor| neighbor.reference != target.reference);
                        neighbors.push(target.clone());
                    }
                    candidates.insert(key, neighbors);
                }
                for next in &candidates[&key] {
                    let mut ancestor = Some(parent);
                    let mut cycle = false;
                    while let Some(index) = ancestor {
                        ancestor = states[index].parent.as_ref().and_then(|(previous, leg)| {
                            cycle |= leg.target.reference == next.reference;
                            (!cycle).then_some(*previous)
                        });
                    }
                    if cycle || next.radius_m <= next.surface_radius_m + performance.radius_m {
                        continue;
                    }
                    let distance = next.pose.position.relative_to(state.pose.position).length();
                    let loss = slip::log_loss_from_ppm(slip::capture_loss_ppm(
                        next.radius_m,
                        distance,
                        next.navigation_beacon.is_some(),
                    ));
                    if state.loss + loss > remaining_log_loss {
                        continue;
                    }
                    let priority = state.seconds
                        + weights.seconds_per_kg * state.fuel
                        + charge_seconds(distance)
                        + slip::flight_seconds(distance, next.navigation_beacon.is_some())
                        + remaining_seconds(
                            next.pose.position,
                            goal,
                            target
                                .as_ref()
                                .is_some_and(|target| target.navigation_beacon.is_some()),
                        )
                        + charge_seconds(goal.position.relative_to(next.pose.position).length());
                    queue.push(Edge {
                        priority,
                        parent,
                        target: next.clone(),
                        departure: None,
                    });
                }
            }
        }
        if queue.len() > 8192 {
            queue = queue
                .into_sorted_vec()
                .into_iter()
                .rev()
                .take(4096)
                .collect();
        }
        let Some(mut edge) = queue.pop() else { break };
        work.charge(ENVIRONMENT_WORK, environment)?;
        let state = &states[edge.parent];
        if edge.departure.is_none() {
            let mut departure = state.pose.clone();
            let mut burn = (0.0, 0.0);
            let mut cleared = false;
            for _ in 0..16 {
                work.charge(ENVIRONMENT_WORK, environment)?;
                let Some(destination) = environment.departure(
                    &departure,
                    edge.target.pose.position,
                    start_s + state.seconds + burn.0,
                )?
                else {
                    cleared = true;
                    break;
                };
                let start = start_s + state.seconds + burn.0;
                let (next, step) =
                    intercept_transfer(performance, weights, &departure, |seconds| {
                        let after = start + seconds;
                        ensure!(
                            after.is_finite()
                                && (0.0..=osg_model::travel::MAX_PREDICTION_SECONDS)
                                    .contains(&after),
                            "route exceeds prediction horizon"
                        );
                        work.charge(ENVIRONMENT_WORK, environment)?;
                        environment.resolve(&destination, after)
                    })?;
                burn.0 += step.0;
                burn.1 += step.1;
                departure = next;
            }
            if !cleared {
                continue;
            }
            // Clearance can dominate the entire journey. Put its cost back into
            // the frontier before expanding this arrival or accepting a solution.
            edge.priority += burn.0 + weights.seconds_per_kg * burn.1;
            edge.departure = Some((departure, burn));
            queue.push(edge);
            continue;
        }
        let (departure, burn) = edge.departure.take().unwrap();
        let charge = charge_seconds(
            edge.target
                .pose
                .position
                .relative_to(departure.position)
                .length(),
        );
        let departure_after = start_s + state.seconds + burn.0 + charge;
        let destination = Destination::Relative {
            reference: Reference::Celestial(edge.target.reference),
            offset: GalacticPosition::default(),
            axes: Axes::Galactic,
        };
        let target = environment
            .capture_target(&destination, departure_after)?
            .unwrap_or(edge.target);
        let distance = target
            .pose
            .position
            .relative_to(departure.position)
            .length();
        let loss = slip::log_loss_from_ppm(slip::capture_loss_ppm(
            target.radius_m,
            distance,
            target.navigation_beacon.is_some(),
        ));
        if state.loss + loss > remaining_log_loss {
            continue;
        }
        let flight = slip::flight_seconds(distance, target.navigation_beacon.is_some());
        let exotic =
            state.exotic + slip::exotic_fuel_kg(performance.mass_kg, distance / slip::LY_M);
        if exotic > performance.exotic_available_kg * request.preferences.fuel_fraction {
            continue;
        }
        let seconds = state.seconds + burn.0 + charge + flight;
        let fuel = state.fuel + burn.1;
        let cost = seconds + weights.seconds_per_kg * fuel;
        if !cost.is_finite() || cost >= best_cost {
            continue;
        }
        forecasts += 1;
        match environment.slip(
            departure.position,
            target.pose.position,
            departure_after,
            departure_after + flight,
            target.navigation_beacon.is_some(),
        ) {
            Ok(estimate) if estimate.ready => {}
            _ => continue,
        }
        let outward = departure
            .position
            .relative_to(target.pose.position)
            .normalize_or_zero();
        let pose = Pose {
            position: target.pose.position.offset_by(outward * target.radius_m),
            velocity: departure.velocity,
            ..target.pose.clone()
        };
        let finish = match arrival_system {
            Some(system) if target.reference.system == system => (0.0, 0.0),
            Some(_) => (f64::INFINITY, 0.0),
            None => transfer(performance, weights, &pose, goal),
        };
        let total = cost + finish.0 + weights.seconds_per_kg * finish.1;
        let reaches_goal = arrival_system
            .map_or(Some(target.reference) == goal_reference, |system| {
                target.reference.system == system
            });
        let next = State {
            pose,
            parent: Some((edge.parent, Leg { target })),
            depth: state.depth + 1,
            seconds,
            fuel,
            exotic,
            loss: state.loss + loss,
        };
        // Only compare identical physical arrival states. A cheaper arrival at
        // another time can face different moving obstacles and is not dominant.
        if states.iter().any(|previous| {
            previous.pose == next.pose
                && previous.seconds == next.seconds
                && previous.fuel <= next.fuel
                && previous.exotic <= next.exotic
                && previous.loss <= next.loss
                && previous.depth <= next.depth
        }) {
            continue;
        }
        let index = states.len();
        states.push(next);
        if total < best_cost
            && start_s + seconds + finish.0 <= osg_model::travel::MAX_PREDICTION_SECONDS
        {
            best_cost = total;
            best = itinerary(&states, index);
            if reaches_goal || finish.0 < 3600. {
                first_solution.get_or_insert_with(std::time::Instant::now);
            }
        }
        expand = Some(index);
    }
    ensure!(
        best_cost.is_finite(),
        "no route found within the search budget and resource allowances"
    );
    Ok(best)
}
