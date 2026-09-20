use super::*;
use osg_model::travel::{Axes, Destination, Reference};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BinaryHeap},
};

#[derive(Clone, Copy, Debug)]
pub(super) enum EdgeKind {
    Sublight,
    Slip,
    Jump(usize),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub time_s: f64,
    pub fuel_kg: f64,
}

pub(super) struct Node {
    pub pose: Pose,
    pub destination: Destination,
    pub gate: Option<usize>,
    system: Option<Id>,
    slip: Option<bool>,
}

pub(super) struct Graph<'a> {
    pub nodes: Vec<Node>,
    gates: &'a [RouteGate],
    groups: BTreeMap<Id, Vec<usize>>,
    exits: Vec<Option<usize>>,
}

#[derive(Clone, Copy)]
struct QueueEntry {
    node: usize,
    cost: f64,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.cost == other.cost
    }
}
impl Eq for QueueEntry {}
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl<'a> Graph<'a> {
    pub fn new(
        origin: Pose,
        target: Pose,
        destination: Destination,
        gates: &'a [RouteGate],
        performance: &ShipPerformance,
    ) -> Result<Self> {
        ensure!(
            gates.len() <= 16_384,
            "navigation catalogue exceeds route limit"
        );
        let indices: BTreeMap<_, _> = gates
            .iter()
            .enumerate()
            .map(|(index, gate)| (gate.navigation.entity, index))
            .collect();
        ensure!(indices.len() == gates.len(), "duplicate navigation mouth");
        let nearest = |pose: &Pose| {
            gates
                .iter()
                .min_by(|a, b| {
                    a.navigation
                        .pose
                        .position
                        .relative_to(pose.position)
                        .length_squared()
                        .total_cmp(
                            &b.navigation
                                .pose
                                .position
                                .relative_to(pose.position)
                                .length_squared(),
                        )
                })
                .map(|gate| gate.navigation.system)
        };
        let mut nodes = Vec::with_capacity(2 + 2 * gates.len());
        nodes.push(Node {
            system: nearest(&origin),
            destination: Destination::Galactic(origin.position),
            pose: origin,
            gate: None,
            slip: (performance.slip_power_w <= 0.0).then_some(false),
        });
        nodes.push(Node {
            system: nearest(&target),
            destination,
            pose: target,
            gate: None,
            slip: (performance.slip_power_w <= 0.0).then_some(false),
        });

        let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut exits = Vec::with_capacity(gates.len());
        for (index, gate) in gates.iter().enumerate() {
            let navigation = &gate.navigation;
            groups.entry(navigation.system).or_default().push(index);
            let exit = indices.get(&navigation.exit).copied().filter(|&exit| {
                gates[exit].navigation.exit == navigation.entity
                    && gate.aperture_radius_m > performance.radius_m + 2.0
                    && gates[exit].aperture_radius_m > performance.radius_m + 2.0
            });
            exits.push(exit);
            nodes.push(Node {
                pose: navigation.pose.clone(),
                destination: Destination::Beacon(navigation.entity),
                gate: Some(index),
                system: Some(navigation.system),
                slip: Some(false),
            });
        }
        for (index, gate) in gates.iter().enumerate() {
            let navigation = &gate.navigation;
            let mut pose = navigation.pose.clone();
            pose.position = navigation.staging;
            nodes.push(Node {
                pose,
                destination: Destination::Relative {
                    reference: Reference::Beacon(navigation.entity),
                    offset: GalacticPosition::from_meters(
                        navigation.staging.relative_to(navigation.pose.position),
                    ),
                    axes: Axes::Galactic,
                },
                gate: Some(index),
                system: Some(navigation.system),
                slip: (!navigation.slip_ready || performance.slip_power_w <= 0.0).then_some(false),
            });
        }
        Ok(Self {
            nodes,
            gates,
            groups,
            exits,
        })
    }

    fn slip_allowed(
        &mut self,
        node: usize,
        environment: &impl RouteEnvironment,
        work: &mut Work,
    ) -> Result<bool> {
        if let Some(allowed) = self.nodes[node].slip {
            return Ok(allowed);
        }
        work.charge(ENVIRONMENT_WORK, environment)?;
        let position = self.nodes[node].pose.position;
        let allowed = environment.slip(position, position, 0.0, 0.0)?.ready;
        self.nodes[node].slip = Some(allowed);
        Ok(allowed)
    }

    pub fn direct(
        &mut self,
        request: &RouteRequest,
        environment: &impl RouteEnvironment,
        work: &mut Work,
        weights: osg_model::transfer::TransferCost,
    ) -> Result<Vec<Edge>> {
        let local = transfer(
            &request.performance,
            weights,
            &self.nodes[0].pose,
            &self.nodes[1].pose,
        );
        let mut best_cost = local.0 + weights.seconds_per_kg * local.1;
        let mut best = vec![Edge {
            from: 0,
            to: 1,
            kind: EdgeKind::Sublight,
            time_s: local.0,
            fuel_kg: local.1,
        }];
        if !request.preferences.allow_slipdrive || request.performance.slip_power_w <= 0.0 {
            return Ok(best);
        }

        let candidates = |endpoint: usize| {
            let mut nodes = vec![endpoint];
            if let Some(gates) = self.nodes[endpoint]
                .system
                .and_then(|system| self.groups.get(&system))
            {
                nodes.extend(gates.iter().map(|gate| 2 + self.gates.len() + gate));
            }
            nodes
        };
        let departures = candidates(0);
        let arrivals = candidates(1);
        for from in departures {
            work.charge(1, environment)?;
            if !self.slip_allowed(from, environment, work)? {
                continue;
            }
            let departure = transfer(
                &request.performance,
                weights,
                &self.nodes[0].pose,
                &self.nodes[from].pose,
            );
            for &to in &arrivals {
                work.charge(1, environment)?;
                let (preparation, transit) = crate::sim::travel::slip_times(
                    self.nodes[from].pose.position,
                    self.nodes[to].pose.position,
                    request.performance.mass_kg,
                    request.performance.slip_power_w,
                    None,
                    0,
                );
                let rendezvous = slip_rendezvous(
                    &request.performance,
                    weights,
                    &self.nodes[from].pose,
                    &self.nodes[to].pose,
                );
                let arrival = transfer(
                    &request.performance,
                    weights,
                    &self.nodes[to].pose,
                    &self.nodes[1].pose,
                );
                let slip = (preparation + transit + rendezvous.0, rendezvous.1);
                let cost = departure.0
                    + slip.0
                    + arrival.0
                    + weights.seconds_per_kg * (departure.1 + slip.1 + arrival.1);
                if cost >= best_cost {
                    continue;
                }
                best_cost = cost;
                best.clear();
                if from != 0 {
                    best.push(Edge {
                        from: 0,
                        to: from,
                        kind: EdgeKind::Sublight,
                        time_s: departure.0,
                        fuel_kg: departure.1,
                    });
                }
                best.push(Edge {
                    from,
                    to,
                    kind: EdgeKind::Slip,
                    time_s: slip.0,
                    fuel_kg: slip.1,
                });
                if to != 1 {
                    best.push(Edge {
                        from: to,
                        to: 1,
                        kind: EdgeKind::Sublight,
                        time_s: arrival.0,
                        fuel_kg: arrival.1,
                    });
                }
            }
        }
        ensure!(best_cost.is_finite(), "no reachable direct transfer");
        Ok(best)
    }

    pub fn search(
        &mut self,
        request: &RouteRequest,
        environment: &impl RouteEnvironment,
        work: &mut Work,
        weights: osg_model::transfer::TransferCost,
    ) -> Result<Vec<Edge>> {
        work.charge(self.nodes.len() as u64, environment)?;
        let mut costs = vec![f64::INFINITY; self.nodes.len()];
        let mut previous = vec![None::<Edge>; self.nodes.len()];
        let mut queue = BinaryHeap::new();
        let mut settled = vec![false; self.nodes.len()];
        costs[0] = 0.0;
        queue.push(QueueEntry { node: 0, cost: 0.0 });

        while let Some(current) = queue.pop() {
            work.charge(1, environment)?;
            if settled[current.node] || current.cost != costs[current.node] {
                continue;
            }
            if current.node == 1 {
                let mut path = Vec::new();
                let mut node = 1;
                while node != 0 {
                    let edge = previous[node]
                        .ok_or_else(|| anyhow::anyhow!("route predecessor unavailable"))?;
                    path.push(edge);
                    node = edge.from;
                }
                path.reverse();
                return Ok(path);
            }
            settled[current.node] = true;
            let from = current.node;
            let local = self.nodes[from]
                .system
                .and_then(|system| self.groups.get(&system))
                .cloned()
                .unwrap_or_default();
            let mut edges = Vec::with_capacity(2 + 2 * local.len());
            edges.push((1, EdgeKind::Sublight));
            for gate in local {
                if request.preferences.allow_wormholes
                    && let Some(exit) = self.exits[gate]
                {
                    edges.push((2 + exit, EdgeKind::Jump(gate)));
                }
                if request.preferences.allow_slipdrive && request.performance.slip_power_w > 0.0 {
                    edges.push((2 + self.gates.len() + gate, EdgeKind::Sublight));
                }
            }

            for (to, kind) in edges {
                work.charge(1, environment)?;
                if from == to || settled[to] {
                    continue;
                }
                let (time, fuel) = match kind {
                    EdgeKind::Jump(entry) => {
                        let gate = &self.gates[entry];
                        let mouth = &gate.navigation.pose;
                        let outward = self.nodes[from]
                            .pose
                            .position
                            .relative_to(mouth.position)
                            .try_normalize()
                            .unwrap_or(DVec3::Z);
                        let mut checkpoint = mouth.clone();
                        checkpoint.position = mouth.position.offset_by(
                            outward
                                * (gate.aperture_radius_m + request.performance.radius_m + 100.0),
                        );
                        let approach = transfer(
                            &request.performance,
                            weights,
                            &self.nodes[from].pose,
                            &checkpoint,
                        );
                        let crossing = transfer(&request.performance, weights, &checkpoint, mouth);
                        let mut clearance = self.nodes[to].pose.clone();
                        clearance.position = clearance.position.offset_by(DVec3::Z * 98.0);
                        let departure = transfer(
                            &request.performance,
                            weights,
                            &self.nodes[to].pose,
                            &clearance,
                        );
                        (
                            approach.0 + crossing.0 + departure.0 + 0.1,
                            approach.1 + crossing.1 + departure.1,
                        )
                    }
                    _ => transfer(
                        &request.performance,
                        weights,
                        &self.nodes[from].pose,
                        &self.nodes[to].pose,
                    ),
                };
                let cost = current.cost + time + weights.seconds_per_kg * fuel;
                if cost < costs[to] {
                    costs[to] = cost;
                    previous[to] = Some(Edge {
                        from,
                        to,
                        kind,
                        time_s: time,
                        fuel_kg: fuel,
                    });
                    queue.push(QueueEntry { node: to, cost });
                }
            }

            if !request.preferences.allow_slipdrive
                || !self.slip_allowed(from, environment, work)?
            {
                continue;
            }
            for to in std::iter::once(1).chain(2 + self.gates.len()..self.nodes.len()) {
                work.charge(1, environment)?;
                if from == to || settled[to] {
                    continue;
                }
                let distance = self.nodes[to]
                    .pose
                    .position
                    .relative_to(self.nodes[from].pose.position)
                    .length();
                if distance <= 1.0 {
                    continue;
                }
                let (preparation, transit) = crate::sim::travel::slip_times_for_distance(
                    distance,
                    request.performance.mass_kg,
                    request.performance.slip_power_w,
                    None,
                    0,
                );
                if current.cost + preparation + transit >= costs[1].min(costs[to]) {
                    continue;
                }
                let rendezvous = slip_rendezvous(
                    &request.performance,
                    weights,
                    &self.nodes[from].pose,
                    &self.nodes[to].pose,
                );
                let cost = current.cost
                    + preparation
                    + transit
                    + rendezvous.0
                    + weights.seconds_per_kg * rendezvous.1;
                if cost < costs[to] {
                    costs[to] = cost;
                    previous[to] = Some(Edge {
                        from,
                        to,
                        kind: EdgeKind::Slip,
                        time_s: preparation + transit + rendezvous.0,
                        fuel_kg: rendezvous.1,
                    });
                    queue.push(QueueEntry { node: to, cost });
                }
            }
        }
        anyhow::bail!("no reachable route with the available propulsion and public gates")
    }
}
