use glam::DVec3;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BinaryHeap},
};
use toy_sim_model::{
    EntityId, GalacticPosition,
    transfer::TransferCost,
    travel::{Axes, Destination, Reference},
};

#[derive(Clone)]
pub(super) struct Gate {
    pub entity: EntityId,
    pub system: EntityId,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub exit: EntityId,
    pub staging: GalacticPosition,
    pub slip_allowed: bool,
}

const RELAXATIONS_PER_TICK: usize = 2048;
const NODES_PER_TICK: usize = 2048;
const SEARCH_CALLBACK_LIMIT: u32 = 60;

fn work_budget_available() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        toy_sim_ship_api::sdk::budget().is_ok_and(|budget| {
            budget.instruction_remaining > 300_000
                && budget.gas_remaining > budget.gas_refill_per_s / 10 + 300_000
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

fn with_endpoints<T>(capacity: usize, endpoints: [T; 2]) -> Vec<T> {
    let mut values = Vec::with_capacity(capacity);
    values.extend(endpoints);
    values
}

#[derive(Clone, Copy)]
struct QueueNode {
    node: usize,
    cost: f64,
}
impl PartialEq for QueueNode {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.cost == other.cost
    }
}
impl Eq for QueueNode {}
impl PartialOrd for QueueNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueueNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}

struct Expansion {
    node: usize,
    local: Vec<usize>,
    edge: usize,
    slip_edge: usize,
    allow_slip: bool,
}

pub(super) struct Graph {
    pub gates: Vec<Gate>,
    pub positions: Vec<GalacticPosition>,
    velocities: Vec<DVec3>,
    pub exits: Vec<Option<usize>>,
    pub distances: Vec<f64>,
    previous: Vec<Option<usize>>,
    visited: Vec<bool>,
    arrived_by_slip: Vec<bool>,
    groups: BTreeMap<EntityId, Vec<usize>>,
    endpoint_groups: [Option<(f64, EntityId)>; 2],
    staging_clearance: Vec<(f64, f64)>,
    initialized: usize,
    settled: u32,
    search_callbacks: u32,
    pub search_limited: bool,
    pub exhausted: bool,
    queue: BinaryHeap<QueueNode>,
    current: Option<Expansion>,
    solution: Option<Vec<usize>>,
    acceleration: f64,
    flow: f64,
    pub weights: TransferCost,
    slip_base_s: f64,
    slip_allowed: Vec<bool>,
}

impl Graph {
    pub fn new(
        origin: GalacticPosition,
        target: GalacticPosition,
        gates: Vec<Gate>,
        acceleration: f64,
        flow: f64,
        slip_base_s: f64,
        endpoints: [bool; 2],
        endpoint_velocities: [DVec3; 2],
    ) -> Self {
        let count = 2 + 2 * gates.len();
        let mut queue = BinaryHeap::with_capacity(count);
        queue.push(QueueNode { node: 0, cost: 0. });
        Self {
            gates,
            positions: with_endpoints(count, [origin, target]),
            velocities: with_endpoints(count, endpoint_velocities),
            exits: with_endpoints(count, [None; 2]),
            distances: with_endpoints(count, [0., f64::INFINITY]),
            previous: with_endpoints(count, [None; 2]),
            visited: with_endpoints(count, [false; 2]),
            arrived_by_slip: with_endpoints(count, [false; 2]),
            groups: BTreeMap::new(),
            endpoint_groups: [None; 2],
            staging_clearance: Vec::with_capacity(count / 2),
            initialized: 0,
            settled: 0,
            search_callbacks: 0,
            search_limited: false,
            exhausted: false,
            queue,
            current: None,
            solution: None,
            acceleration,
            flow,
            weights: TransferCost::default(),
            slip_base_s,
            slip_allowed: with_endpoints(count, endpoints),
        }
    }

    pub fn restart(
        &mut self,
        origin: GalacticPosition,
        target: GalacticPosition,
        acceleration: f64,
        flow: f64,
        slip_base_s: f64,
        endpoints: [bool; 2],
        endpoint_velocities: [DVec3; 2],
    ) {
        self.positions[..2].copy_from_slice(&[origin, target]);
        self.velocities[..2].copy_from_slice(&endpoint_velocities);
        self.slip_allowed[..2].copy_from_slice(&endpoints);
        self.distances[..2].copy_from_slice(&[0., f64::INFINITY]);
        self.previous[..2].fill(None);
        self.visited[..2].fill(false);
        self.arrived_by_slip[..2].fill(false);
        self.endpoint_groups = [None; 2];
        self.initialized = 0;
        self.settled = 0;
        self.search_callbacks = 0;
        self.search_limited = false;
        self.exhausted = false;
        self.queue.clear();
        self.queue.push(QueueNode { node: 0, cost: 0. });
        self.current = None;
        self.solution = None;
        self.acceleration = acceleration;
        self.flow = flow;
        self.slip_base_s = slip_base_s;
    }

    pub fn refresh_gate(&mut self, index: usize, gate: Gate) {
        let mouth = index + 2;
        let staging = mouth + self.gates.len();
        self.positions[mouth] = gate.position;
        self.positions[staging] = gate.staging;
        self.velocities[mouth] = gate.velocity;
        self.velocities[staging] = gate.velocity;
        self.slip_allowed[staging] = gate.slip_allowed;
        let system = gate.system;
        self.gates[index] = gate;
        if let Some(mouths) = self.groups.get(&system) {
            for &mouth in mouths {
                let gate = &self.gates[mouth - 2];
                let mut distance_squared = f64::INFINITY;
                let mut speed_squared: f64 = 0.;
                for &other in mouths {
                    distance_squared = distance_squared.min(
                        gate.staging
                            .relative_to(self.positions[other])
                            .length_squared(),
                    );
                    speed_squared = speed_squared
                        .max((gate.velocity - self.velocities[other]).length_squared());
                }
                self.staging_clearance[mouth - 2] = (distance_squared.sqrt(), speed_squared.sqrt());
            }
        }
    }

    pub fn refresh_motion(
        &mut self,
        origin: &toy_sim_model::Pose,
        target: &toy_sim_model::Pose,
        acceleration: f64,
        flow: f64,
        weights: TransferCost,
    ) {
        self.positions[..2].copy_from_slice(&[origin.position, target.position]);
        self.velocities[..2].copy_from_slice(&[
            DVec3::from_array(origin.velocity),
            DVec3::from_array(target.velocity),
        ]);
        self.acceleration = acceleration;
        self.flow = flow;
        self.weights = weights;
    }

    pub fn prepare(&mut self) {
        self.initialize();
    }

    pub fn progress(&self) -> toy_sim_model::travel::PlanningProgress {
        use toy_sim_model::travel::{PlanningProgress, PlanningStage};
        let nodes = 2 * self.gates.len();
        if self.initialized < nodes {
            PlanningProgress {
                stage: PlanningStage::BuildingGraph,
                completed: self.initialized as u32,
                total: Some(nodes as u32),
            }
        } else {
            PlanningProgress {
                stage: PlanningStage::SearchingRoutes,
                completed: self.settled,
                total: Some((nodes + 2) as u32),
            }
        }
    }

    pub fn release_groups(&mut self) -> bool {
        for _ in 0..128 {
            if !work_budget_available() || self.groups.pop_first().is_none() {
                break;
            }
        }
        self.groups.is_empty()
    }

    fn initialize(&mut self) -> bool {
        let count = self.gates.len();
        let end = (self.initialized + NODES_PER_TICK).min(2 * count);
        while self.initialized < end {
            if self.initialized % 16 == 0 && !work_budget_available() {
                return false;
            }
            let index = self.initialized % count;
            let staging = self.initialized >= count;
            let gate = &self.gates[index];
            let node = self.initialized + 2;
            let existing = node < self.positions.len();
            if existing {
                self.distances[node] = f64::INFINITY;
                self.previous[node] = None;
                self.visited[node] = false;
                self.arrived_by_slip[node] = false;
            } else {
                self.positions
                    .push(if staging { gate.staging } else { gate.position });
                self.velocities.push(gate.velocity);
                self.slip_allowed.push(staging && gate.slip_allowed);
                self.exits.push(if staging {
                    None
                } else {
                    self.gates
                        .binary_search_by_key(&gate.exit, |gate| gate.entity)
                        .ok()
                        .map(|index| index + 2)
                });
                self.distances.push(f64::INFINITY);
                self.previous.push(None);
                self.visited.push(false);
                self.arrived_by_slip.push(false);
                if staging {
                    let mut distance_squared = f64::INFINITY;
                    let mut speed_squared: f64 = 0.;
                    if let Some(mouths) = self.groups.get(&gate.system) {
                        for &mouth in mouths {
                            distance_squared = distance_squared.min(
                                gate.staging
                                    .relative_to(self.positions[mouth])
                                    .length_squared(),
                            );
                            speed_squared = speed_squared
                                .max((gate.velocity - self.velocities[mouth]).length_squared());
                        }
                    }
                    self.staging_clearance
                        .push((distance_squared.sqrt(), speed_squared.sqrt()));
                }
            }
            if !staging {
                if !existing {
                    self.groups.entry(gate.system).or_default().push(index + 2);
                }
                for endpoint in 0..2 {
                    let distance = gate
                        .position
                        .relative_to(self.positions[endpoint])
                        .length_squared();
                    if self.endpoint_groups[endpoint].is_none_or(|(best, _)| distance < best) {
                        self.endpoint_groups[endpoint] = Some((distance, gate.system));
                    }
                }
            }
            self.initialized += 1;
        }
        self.initialized == 2 * count
    }

    pub fn transfer(&self, from: usize, to: usize) -> (f64, f64) {
        let offset = self.positions[to].relative_to(self.positions[from]);
        let velocity = self.velocities[from] - self.velocities[to];
        let direction = offset.normalize_or_zero();
        let closing = velocity.dot(direction);
        let lateral = (velocity - direction * closing).length();
        let (time, fuel) =
            self.weights
                .remaining(offset.length(), closing, self.acceleration, self.flow);
        let correction_s = lateral / self.acceleration;
        (time + correction_s, fuel + correction_s * self.flow)
    }

    fn sublight_cost(&self, from: usize, to: usize) -> f64 {
        let (time, fuel) = self.transfer(from, to);
        time + self.weights.seconds_per_kg * fuel
    }

    pub fn destination(&self, node: usize) -> Destination {
        match node
            .checked_sub(2 + self.gates.len())
            .and_then(|index| self.gates.get(index))
        {
            Some(gate) => Destination::Relative {
                reference: Reference::Beacon(gate.entity),
                offset: GalacticPosition::from_meters(gate.staging.relative_to(gate.position)),
                axes: Axes::Galactic,
            },
            None => match node.checked_sub(2).and_then(|index| self.gates.get(index)) {
                Some(gate) => Destination::Beacon(gate.entity),
                None => Destination::Galactic(self.positions[node]),
            },
        }
    }

    pub fn arrival_adjustment(&self, from: usize, to: usize) -> (f64, f64) {
        self.weights.remaining(
            0.,
            (self.velocities[from] - self.velocities[to]).length(),
            self.acceleration,
            self.flow,
        )
    }

    fn slip_cost(&self, from: usize, to: usize) -> f64 {
        let distance = self.positions[to]
            .relative_to(self.positions[from])
            .length();
        if !self.slip_allowed[from] || !self.slip_allowed[to] || distance <= 1. {
            return f64::INFINITY;
        }
        let (arrival_s, arrival_kg) = self.arrival_adjustment(from, to);
        self.slip_base_s
            + 8.64 * distance / 9.4607304725808e15
            + arrival_s
            + self.weights.seconds_per_kg * arrival_kg
    }

    pub fn is_slip(&self, from: usize, to: usize) -> bool {
        if self.previous[to] == Some(from) {
            return self.arrived_by_slip[to];
        }
        self.exits[from] != Some(to)
            && !self.arrived_by_slip[from]
            && self.slip_cost(from, to) < self.sublight_cost(from, to)
    }

    fn expand(&self, node: usize) -> Expansion {
        let group = if node < 2 {
            self.endpoint_groups[node].map(|(_, group)| group)
        } else {
            Some(self.gates[(node - 2) % self.gates.len()].system)
        };
        let mut local = vec![1];
        if let Some(exit) = self.exits[node] {
            local.push(exit);
        }
        if let Some(mouths) = group.and_then(|group| self.groups.get(&group)) {
            for &mouth in mouths {
                local.push(mouth);
                if self.slip_base_s.is_finite() && node < 2 + self.gates.len() {
                    local.push(mouth + self.gates.len());
                }
            }
        }
        local.sort_unstable();
        local.dedup();
        let consecutive_slip = self.arrived_by_slip[node];
        Expansion {
            node,
            local,
            edge: 0,
            slip_edge: 0,
            allow_slip: self.slip_base_s.is_finite()
                && self.slip_allowed[node]
                && !consecutive_slip
                && self.distances[node] + self.slip_base_s < self.distances[1],
        }
    }

    fn approach_lower_bound(&self, distance: f64, closing: f64) -> f64 {
        if distance <= 0. {
            return 0.;
        }
        let fuel_weight = self.weights.seconds_per_kg * self.flow;
        let optimal_speed =
            ((0.5 * closing * closing + self.acceleration * distance) / (0.5 + fuel_weight)).sqrt();
        let burn = ((optimal_speed - closing) / self.acceleration).max(0.);
        let speed = closing + self.acceleration * burn;
        (distance + 0.5 * self.acceleration * burn * burn) / speed + fuel_weight * burn
    }

    fn staging_lower_bound(&self, node: usize) -> f64 {
        let (distance, speed) = self.staging_clearance[node - 2 - self.gates.len()];
        let gate_approach = self.approach_lower_bound(distance, speed);
        let destination_approach = self.approach_lower_bound(
            self.positions[1].relative_to(self.positions[node]).length(),
            (self.velocities[node] - self.velocities[1]).length(),
        );
        gate_approach.min(destination_approach)
    }

    fn relax(&mut self, from: usize, to: usize, slip_only: bool) {
        if from == to || self.visited[to] {
            return;
        }
        let (cost, by_slip) = if self.exits[from] == Some(to) {
            (0.1, false)
        } else if slip_only {
            (self.slip_cost(from, to), true)
        } else {
            let sublight = self.sublight_cost(from, to);
            let slip = if self.arrived_by_slip[from] {
                f64::INFINITY
            } else {
                self.slip_cost(from, to)
            };
            (sublight.min(slip), slip < sublight)
        };
        let cost = self.distances[from] + cost;
        if slip_only {
            let following = self.staging_lower_bound(to);
            if cost + following >= self.distances[1] {
                return;
            }
        }
        if cost < self.distances[to] {
            self.distances[to] = cost;
            self.previous[to] = Some(from);
            self.arrived_by_slip[to] = by_slip;
            self.queue.push(QueueNode { node: to, cost });
        }
    }

    fn incumbent_uses_slip(&self) -> bool {
        let mut node = 1;
        for _ in 0..256 {
            if self.arrived_by_slip[node] {
                return true;
            }
            let Some(previous) = self.previous[node] else {
                return false;
            };
            node = previous;
        }
        false
    }

    pub fn advance(&mut self) -> Option<Vec<usize>> {
        if !self.initialize() {
            return None;
        }
        self.search_callbacks += 1;
        if self.search_callbacks >= SEARCH_CALLBACK_LIMIT
            && self.solution.is_none()
            && self.distances[1].is_finite()
            && self.incumbent_uses_slip()
        {
            self.search_limited = true;
            self.solution = Some(vec![1]);
        }
        for iteration in 0..RELAXATIONS_PER_TICK {
            if iteration % 16 == 0 && !work_budget_available() {
                return None;
            }
            if let Some(path) = &mut self.solution {
                let cursor = *path.last().unwrap();
                if let Some(previous) = self.previous[cursor] {
                    path.push(previous);
                    continue;
                }
                let mut path = self.solution.take().unwrap();
                path.reverse();
                return Some(path);
            }
            if self.current.is_none() {
                let Some(next) = self.queue.pop() else {
                    self.exhausted = true;
                    return None;
                };
                if self.visited[next.node] || next.cost != self.distances[next.node] {
                    continue;
                }
                if next.node == 1 {
                    self.solution = Some(vec![1]);
                    continue;
                }
                self.visited[next.node] = true;
                self.settled += 1;
                self.current = Some(self.expand(next.node));
            }
            let expansion = self.current.as_mut().unwrap();
            let from = expansion.node;
            if let Some(&to) = expansion.local.get(expansion.edge) {
                expansion.edge += 1;
                self.relax(from, to, false);
            } else if expansion.allow_slip && expansion.slip_edge < self.gates.len() {
                let to = 2 + self.gates.len() + expansion.slip_edge;
                expansion.slip_edge += 1;
                self.relax(from, to, true);
            } else {
                self.current = None;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use toy_sim_model::Id;

    fn gate(id: u8, exit: u8, x: f64) -> Gate {
        Gate {
            entity: Id([id; 16]),
            system: Id([(x / 1e12).floor() as u8; 16]),
            position: GalacticPosition::from_meters(DVec3::X * x),
            velocity: DVec3::ZERO,
            staging: GalacticPosition::from_meters(DVec3::X * (x + 1e7)),
            slip_allowed: false,
            exit: Id([exit; 16]),
        }
    }

    fn finish(graph: &mut Graph) -> Vec<usize> {
        for _ in 0..4096 {
            if let Some(path) = graph.advance() {
                return path;
            }
        }
        panic!("bounded routing did not finish");
    }

    #[test]
    fn slip_staging_and_costs_are_independent_of_shared_galactic_velocity() {
        let mut mouth = gate(1, 2, 0.);
        mouth.velocity = DVec3::Y * 30_000.;
        let gates = [mouth];
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            gates.to_vec(),
            5.,
            10.,
            90.,
            [true; 2],
            [DVec3::Y * 30_000.; 2],
        );
        while !graph.initialize() {}
        let transfer = graph.transfer(2, 3);
        assert_eq!(transfer, graph.weights.estimate(1e7, 5., 10.));
        assert_eq!(
            graph.destination(3),
            Destination::Relative {
                reference: Reference::Beacon(gates[0].entity),
                offset: GalacticPosition::from_meters(DVec3::X * 1e7),
                axes: Axes::Galactic,
            }
        );
        assert_eq!(graph.arrival_adjustment(0, 1), (0., 0.));
        let moving_cost = graph.slip_cost(0, 1);
        for velocity in &mut graph.velocities {
            *velocity -= DVec3::Y * 30_000.;
        }
        assert_eq!(graph.slip_cost(0, 1), moving_cost);
        graph.velocities[1] = DVec3::Y * 1000.;
        assert!(graph.arrival_adjustment(0, 1).1 > 0.);
    }

    #[test]
    fn route_selection_compares_time_plus_fuel_instead_of_time_alone() {
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e6),
            Vec::new(),
            5.,
            10.,
            0.,
            [true; 2],
            [DVec3::ZERO; 2],
        );
        let (time, fuel) = graph.transfer(0, 1);
        graph.slip_base_s = time + graph.weights.seconds_per_kg * fuel * 0.5;
        assert!(graph.slip_cost(0, 1) > time);
        assert!(graph.is_slip(0, 1));
        assert_eq!(finish(&mut graph), vec![0, 1]);
    }

    #[test]
    fn route_uses_multiple_gate_pairs() {
        let gates = [
            gate(1, 2, 1e6),
            gate(2, 1, 1e12),
            gate(3, 4, 1e12 + 1e6),
            gate(4, 3, 2e12),
        ];
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * (2e12 + 1e6)),
            gates.to_vec(),
            1.,
            1.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        assert_eq!(finish(&mut graph), vec![0, 2, 3, 4, 5, 1]);
        assert!(graph.distances[1].is_finite());
    }

    #[test]
    fn distant_gate_does_not_detour_direct_route() {
        let gates = [gate(1, 2, 1e12), gate(2, 1, 2e12)];
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e6),
            gates.to_vec(),
            1.,
            1.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        assert_eq!(finish(&mut graph), vec![0, 1]);
    }

    #[test]
    fn missing_exit_is_not_a_teleport() {
        let gates = [gate(1, 2, 1e6)];
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            gates.to_vec(),
            1.,
            1.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        finish(&mut graph);
        assert!(graph.distances[1] > 1e6);
    }

    #[test]
    fn graph_search_yields_before_finishing_large_catalog() {
        let gates = tree_gates();
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            gates.to_vec(),
            1.,
            1.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        assert!(graph.advance().is_none());
        assert_eq!(graph.initialized, NODES_PER_TICK);
        assert_eq!(graph.visited.iter().filter(|&&visited| visited).count(), 0);
        assert!(!finish(&mut graph).is_empty());
    }
    #[test]
    fn slip_edges_win_only_when_admissible_and_faster() {
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            Vec::new(),
            1.,
            1.,
            90.,
            [true; 2],
            [DVec3::ZERO; 2],
        );
        assert_eq!(finish(&mut graph), vec![0, 1]);
        assert!(graph.is_slip(0, 1));
        let blocked = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            Vec::new(),
            1.,
            1.,
            90.,
            [false, true],
            [DVec3::ZERO; 2],
        );
        assert!(!blocked.is_slip(0, 1));
        let nearby = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X),
            Vec::new(),
            1.,
            1.,
            90.,
            [true; 2],
            [DVec3::ZERO; 2],
        );
        assert!(!nearby.is_slip(0, 1));
    }
    fn stable_id(index: u32) -> Id {
        let mut bytes = [0; 16];
        bytes[..4].copy_from_slice(&index.to_be_bytes());
        Id(bytes)
    }

    fn system_position(system: usize) -> GalacticPosition {
        GalacticPosition::from_meters(
            DVec3::new(
                (system % 20) as f64,
                ((system / 20) % 15) as f64,
                (system / 300) as f64,
            ) * (5. * 9.4607304725808e15),
        )
    }

    fn tree_gates() -> Vec<Gate> {
        let mut gates = Vec::new();
        for child in 1..3000 {
            let parent = (child - 1) / 2;
            for (side, system) in [(0, parent), (1, child)] {
                let id = child as u32 * 2 + side;
                let position =
                    system_position(system).offset_by(DVec3::X * (1e6 * f64::from(id % 3 + 1)));
                gates.push(Gate {
                    entity: stable_id(id),
                    system: stable_id(system as u32),
                    position,
                    velocity: DVec3::ZERO,
                    exit: stable_id(id ^ 1),
                    staging: position.offset_by(DVec3::Y * 1e7),
                    slip_allowed: false,
                });
            }
        }
        gates
    }

    #[test]
    fn unreachable_graph_reports_exhaustion() {
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X),
            Vec::new(),
            0.,
            0.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        assert!(graph.advance().is_none());
        assert!(graph.exhausted);
    }

    #[test]
    fn approach_bound_is_optimistic_for_motion_and_fuel_preferences() {
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::ZERO,
            Vec::new(),
            1.,
            1.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        for distance in [1., 1e4, 1e8, 1e12] {
            for acceleration in [0.01, 1., 100.] {
                graph.acceleration = acceleration;
                for closing in [-1000_f64, -10., 0., 10., 1000.] {
                    for preference in [0., 1., 1000.] {
                        graph.weights.seconds_per_kg = preference;
                        let (time, fuel) =
                            graph
                                .weights
                                .remaining(distance, closing, acceleration, graph.flow);
                        let bound = graph.approach_lower_bound(distance, closing.abs());
                        assert!(bound <= (time + preference * fuel) * (1. + 1e-12));
                    }
                }
            }
        }
    }

    #[test]
    fn slip_arrival_can_approach_a_different_nearer_mouth() {
        let mut gates = vec![
            gate(1, 2, 1e12),
            gate(2, 1, 5e15),
            gate(3, 4, 1001.),
            gate(4, 3, 1e15),
        ];
        gates[0].system = Id([0; 16]);
        gates[2].system = Id([0; 16]);
        gates[1].system = Id([1; 16]);
        gates[3].system = Id([2; 16]);
        for gate in &mut gates {
            gate.staging = gate.position.offset_by(DVec3::Y * 1e13);
            gate.slip_allowed = true;
        }
        gates[0].staging = GalacticPosition::from_meters(DVec3::X * 1000.);
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * (1e15 + 1000.)),
            gates,
            1.,
            1.,
            90.,
            [true, false],
            [DVec3::ZERO; 2],
        );
        let path = finish(&mut graph);
        assert!(path.windows(2).any(|pair| pair == [6, 4]), "{path:?}");
        assert!(path.windows(2).any(|pair| pair == [4, 5]), "{path:?}");
    }

    #[test]
    fn three_thousand_system_gate_route_finishes_with_sparse_bounded_work() {
        let mut graph = Graph::new(
            system_position(2047),
            system_position(2999),
            tree_gates(),
            5.,
            10.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        let path = finish(&mut graph);
        let jumps = path
            .windows(2)
            .filter(|edge| graph.exits[edge[0]] == Some(edge[1]))
            .count();
        assert!(jumps > 10 && jumps < 30);
        assert!(path.len() < 256);
        assert!(graph.distances[1].is_finite());
        assert_eq!(path.first(), Some(&0));
        assert_eq!(path.last(), Some(&1));
    }

    #[test]
    fn large_catalogue_still_combines_slip_with_a_useful_gate_shortcut() {
        let ly = 9.4607304725808e15;
        let mut gates = tree_gates();
        for gate in &mut gates {
            gate.position = gate.position.offset_by(DVec3::Y * (100. * ly));
            gate.staging = gate.position.offset_by(DVec3::Y * 1e6);
            gate.slip_allowed = true;
        }
        gates[0].position = GalacticPosition::from_meters(DVec3::X * (0.1 * ly));
        gates[1].position = GalacticPosition::from_meters(DVec3::X * (249.9 * ly));
        for gate in &mut gates[..2] {
            gate.staging = gate.position.offset_by(DVec3::Y * 1e6);
        }
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * (250. * ly)),
            gates,
            100.,
            1.,
            90.,
            [true; 2],
            [DVec3::ZERO; 2],
        );
        let path = finish(&mut graph);
        assert!(
            path.windows(2)
                .any(|edge| graph.exits[edge[0]] == Some(edge[1]))
        );
        assert!(path.windows(2).any(|edge| graph.is_slip(edge[0], edge[1])));
        assert!(graph.distances[1] < 90. + 250. * 8.64);
    }
}
