use glam::DVec3;
use toy_sim_model::{
    EntityId, GalacticPosition,
    transfer::TransferCost,
    travel::{Axes, Destination, Reference},
};

pub(super) struct Gate {
    pub entity: EntityId,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub exit: EntityId,
    pub staging: GalacticPosition,
    pub slip_allowed: bool,
}

const RELAXATIONS_PER_TICK: usize = 128;

pub(super) struct Graph {
    pub positions: Vec<GalacticPosition>,
    velocities: Vec<DVec3>,
    pub exits: Vec<Option<usize>>,
    pub distances: Vec<f64>,
    previous: Vec<Option<usize>>,
    visited: Vec<bool>,
    current: Option<usize>,
    edge: usize,
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
        gates: &[Gate],
        acceleration: f64,
        flow: f64,
        slip_base_s: f64,
        endpoints: [bool; 2],
        endpoint_velocities: [DVec3; 2],
    ) -> Self {
        let mut positions = vec![origin, target];
        positions.extend(gates.iter().map(|gate| gate.position));
        positions.extend(gates.iter().map(|gate| gate.staging));
        let mut velocities = endpoint_velocities.to_vec();
        velocities.extend(gates.iter().map(|gate| gate.velocity));
        velocities.extend(gates.iter().map(|gate| gate.velocity));
        let mut exits = vec![None; positions.len()];
        let indices: std::collections::BTreeMap<_, _> = gates
            .iter()
            .enumerate()
            .map(|(index, gate)| (gate.entity, index + 2))
            .collect();
        for (index, gate) in gates.iter().enumerate() {
            exits[index + 2] = indices.get(&gate.exit).copied();
        }
        let mut distances = vec![f64::INFINITY; positions.len()];
        distances[0] = 0.;
        let mut slip_allowed = endpoints.to_vec();
        slip_allowed.extend(gates.iter().map(|_| false));
        slip_allowed.extend(gates.iter().map(|g| g.slip_allowed));
        Self {
            previous: vec![None; positions.len()],
            visited: vec![false; positions.len()],
            positions,
            velocities,
            exits,
            distances,
            current: None,
            edge: 0,
            acceleration,
            flow,
            weights: TransferCost::default(),
            slip_base_s,
            slip_allowed,
        }
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

    pub fn destination(&self, node: usize, gates: &[Gate]) -> Destination {
        let gate = node
            .checked_sub(2 + gates.len())
            .and_then(|index| gates.get(index));
        match gate {
            Some(gate) => Destination::Relative {
                reference: Reference::Beacon(gate.entity),
                offset: GalacticPosition::from_meters(gate.staging.relative_to(gate.position)),
                axes: Axes::Galactic,
            },
            None => Destination::Galactic(self.positions[node]),
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
        let (_, transit) = self.costs(from, to);
        let (arrival_s, arrival_kg) = self.arrival_adjustment(from, to);
        transit + arrival_s + self.weights.seconds_per_kg * arrival_kg
    }

    pub fn costs(&self, from: usize, to: usize) -> (f64, f64) {
        let distance = self.positions[to]
            .relative_to(self.positions[from])
            .length();
        let (time, _) = self.transfer(from, to);
        let slip = if self.slip_allowed[from] && self.slip_allowed[to] && distance > 1. {
            self.slip_base_s + 8.64 * distance / 9.4607304725808e15
        } else {
            f64::INFINITY
        };
        (time, slip)
    }

    pub fn is_slip(&self, from: usize, to: usize) -> bool {
        self.exits[from] != Some(to) && self.slip_cost(from, to) < self.sublight_cost(from, to)
    }

    pub fn advance(&mut self) -> Option<Vec<usize>> {
        for _ in 0..RELAXATIONS_PER_TICK {
            let current = match self.current {
                Some(current) => current,
                None => {
                    let current = (0..self.positions.len())
                        .filter(|&i| !self.visited[i])
                        .min_by(|&a, &b| self.distances[a].total_cmp(&self.distances[b]))?;
                    if current == 1 {
                        let mut path = vec![current];
                        let mut cursor = current;
                        while let Some(previous) = self.previous[cursor] {
                            path.push(previous);
                            cursor = previous;
                        }
                        path.reverse();
                        return Some(path);
                    }
                    self.visited[current] = true;
                    self.current = Some(current);
                    self.edge = 0;
                    current
                }
            };
            let next = self.edge;
            self.edge += 1;
            if !self.visited[next] {
                let cost = if self.exits[current] == Some(next) {
                    0.1
                } else {
                    self.sublight_cost(current, next)
                        .min(self.slip_cost(current, next))
                };
                let cost = self.distances[current] + cost;
                if cost < self.distances[next] {
                    self.distances[next] = cost;
                    self.previous[next] = Some(current);
                }
            }
            if self.edge == self.positions.len() {
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
            &gates,
            5.,
            10.,
            90.,
            [true; 2],
            [DVec3::Y * 30_000.; 2],
        );
        let transfer = graph.transfer(2, 3);
        assert_eq!(transfer, graph.weights.estimate(1e7, 5., 10.));
        assert_eq!(
            graph.destination(3, &gates),
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
            &[],
            5.,
            10.,
            0.,
            [true; 2],
            [DVec3::ZERO; 2],
        );
        let (time, fuel) = graph.transfer(0, 1);
        graph.slip_base_s = time + graph.weights.seconds_per_kg * fuel * 0.5;
        assert!(graph.costs(0, 1).1 > time);
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
            &gates,
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
            &gates,
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
            &gates,
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
        let gates: Vec<_> = (0..200)
            .map(|index| gate(index + 1, index + 1, 1e6 * (f64::from(index) + 1.)))
            .collect();
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            &gates,
            1.,
            1.,
            f64::INFINITY,
            [false; 2],
            [DVec3::ZERO; 2],
        );
        assert!(graph.advance().is_none());
        assert_eq!(graph.edge, RELAXATIONS_PER_TICK);
        assert_eq!(graph.visited.iter().filter(|&&visited| visited).count(), 1);
        assert!(!finish(&mut graph).is_empty());
    }
    #[test]
    fn slip_edges_win_only_when_admissible_and_faster() {
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e12),
            &[],
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
            &[],
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
            &[],
            1.,
            1.,
            90.,
            [true; 2],
            [DVec3::ZERO; 2],
        );
        assert!(!nearby.is_slip(0, 1));
    }
}
