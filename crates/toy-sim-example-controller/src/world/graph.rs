use toy_sim_model::{EntityId, GalacticPosition};

pub(super) struct Gate {
    pub entity: EntityId,
    pub position: GalacticPosition,
    pub exit: EntityId,
}

const RELAXATIONS_PER_TICK: usize = 128;

pub(super) struct Graph {
    positions: Vec<GalacticPosition>,
    pub exits: Vec<Option<usize>>,
    pub distances: Vec<f64>,
    previous: Vec<Option<usize>>,
    visited: Vec<bool>,
    current: Option<usize>,
    edge: usize,
}

impl Graph {
    pub fn new(origin: GalacticPosition, target: GalacticPosition, gates: &[Gate]) -> Self {
        let mut positions = vec![origin, target];
        positions.extend(gates.iter().map(|gate| gate.position));
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
        Self {
            previous: vec![None; positions.len()],
            visited: vec![false; positions.len()],
            positions,
            exits,
            distances,
            current: None,
            edge: 0,
        }
    }

    pub fn advance(&mut self) -> Option<Vec<usize>> {
        for _ in 0..RELAXATIONS_PER_TICK {
            let current = match self.current {
                Some(current) => current,
                None => {
                    let current = (0..self.positions.len())
                        .filter(|&index| !self.visited[index])
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
                let distance = self.positions[next]
                    .relative_to(self.positions[current])
                    .length();
                let cost = if self.exits[current] == Some(next) {
                    distance.min(1e6)
                } else {
                    distance
                };
                let distance = self.distances[current] + cost;
                if distance < self.distances[next] {
                    self.distances[next] = distance;
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
            exit: Id([exit; 16]),
        }
    }

    fn finish(graph: &mut Graph) -> Vec<usize> {
        for _ in 0..1024 {
            if let Some(path) = graph.advance() {
                return path;
            }
        }
        panic!("bounded routing did not finish");
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
        );
        assert_eq!(finish(&mut graph), vec![0, 2, 3, 4, 5, 1]);
        assert_eq!(graph.distances[1], 5e6);
    }

    #[test]
    fn distant_gate_does_not_detour_direct_route() {
        let gates = [gate(1, 2, 1e12), gate(2, 1, 2e12)];
        let mut graph = Graph::new(
            GalacticPosition::ZERO,
            GalacticPosition::from_meters(DVec3::X * 1e6),
            &gates,
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
        );
        finish(&mut graph);
        assert_eq!(graph.distances[1], 1e12);
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
        );
        assert!(graph.advance().is_none());
        assert_eq!(graph.edge, RELAXATIONS_PER_TICK);
        assert_eq!(graph.visited.iter().filter(|&&visited| visited).count(), 1);
        assert!(!finish(&mut graph).is_empty());
    }
}
