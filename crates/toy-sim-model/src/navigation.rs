use crate::{EntityId, GalacticPosition, NavigationCatalogue};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationConnection {
    pub entry: EntityId,
    pub exit: EntityId,
    pub destination: EntityId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationRegion {
    pub id: EntityId,
    pub position: GalacticPosition,
    pub gates: Vec<NavigationConnection>,
}

#[derive(Default)]
pub struct GateNetwork {
    pub regions: Vec<NavigationRegion>,
    indices: BTreeMap<EntityId, usize>,
    edges: Vec<Vec<(usize, EntityId)>>,
}

impl GateNetwork {
    pub fn new(regions: Vec<NavigationRegion>) -> Self {
        let indices: BTreeMap<_, _> = regions
            .iter()
            .enumerate()
            .map(|(index, system)| (system.id, index))
            .collect();
        let edges = regions
            .iter()
            .map(|region| {
                region
                    .gates
                    .iter()
                    .filter_map(|gate| {
                        indices
                            .get(&gate.destination)
                            .map(|&next| (next, gate.entry))
                    })
                    .collect()
            })
            .collect();
        Self {
            regions,
            indices,
            edges,
        }
    }

    pub fn from_catalogue(catalogue: &NavigationCatalogue) -> Self {
        let beacons: BTreeMap<_, _> = catalogue.beacons.iter().map(|b| (b.id, b)).collect();
        let mut regions: Vec<_> = catalogue
            .systems
            .iter()
            .map(|system| NavigationRegion {
                id: system.id,
                position: system.position,
                gates: Vec::new(),
            })
            .collect();
        let indices: BTreeMap<_, _> = regions.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
        for entry in &catalogue.beacons {
            let Some(exit) = entry.gate_exit.and_then(|id| beacons.get(&id)) else {
                continue;
            };
            if exit.gate_exit != Some(entry.id) {
                continue;
            }
            if let Some(&index) = indices.get(&entry.system) {
                regions[index].gates.push(NavigationConnection {
                    entry: entry.id,
                    exit: exit.id,
                    destination: exit.system,
                });
            }
        }
        Self::new(regions)
    }

    pub fn nearest(&self, position: GalacticPosition) -> Option<EntityId> {
        self.regions
            .iter()
            .min_by(|a, b| {
                a.position
                    .relative_to(position)
                    .length_squared()
                    .total_cmp(&b.position.relative_to(position).length_squared())
            })
            .map(|system| system.id)
    }

    pub fn route(&self, origin: EntityId, destination: EntityId) -> Option<Vec<EntityId>> {
        let &origin = self.indices.get(&origin)?;
        let &destination = self.indices.get(&destination)?;
        let mut previous = vec![None; self.regions.len()];
        let mut seen = vec![false; self.regions.len()];
        seen[origin] = true;
        let mut queue = VecDeque::from([origin]);
        while let Some(current) = queue.pop_front() {
            if current == destination {
                let mut route = Vec::new();
                let mut cursor = current;
                while let Some((parent, gate)) = previous[cursor] {
                    route.push(gate);
                    cursor = parent;
                }
                route.reverse();
                return Some(route);
            }
            for &(next, gate) in &self.edges[current] {
                if !seen[next] {
                    seen[next] = true;
                    previous[next] = Some((current, gate));
                    queue.push_back(next);
                }
            }
        }
        None
    }
}

pub fn gate_route(
    catalogue: &NavigationCatalogue,
    origin: EntityId,
    destination: EntityId,
) -> Option<Vec<EntityId>> {
    GateNetwork::from_catalogue(catalogue).route(origin, destination)
}
