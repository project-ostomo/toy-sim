use crate::{EntityId, GalacticPosition, NavigationCatalogue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
}

impl GateNetwork {
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
        Self { regions }
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
}
