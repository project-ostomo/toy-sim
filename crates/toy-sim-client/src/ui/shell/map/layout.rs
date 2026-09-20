use super::*;
use std::collections::BTreeMap;
use toy_sim_model::navigation::GateNetwork;
use toy_sim_universe::civilization::LIGHT_YEAR_M;

#[derive(Default)]
pub(super) struct Cache {
    revision: Option<u64>,
    pub network: GateNetwork,
    pub systems: BTreeMap<Id, usize>,
    pub beacons: BTreeMap<Id, usize>,
    pub positions: Vec<glam::DVec3>,
    pub names: Vec<String>,
    pub links: Vec<(usize, usize, Id, Id)>,
    pub bounds: glam::DVec3,
}

impl Cache {
    pub fn update(&mut self, catalogue: &NavigationCatalogue) -> bool {
        if self.revision == Some(catalogue.topology_revision) {
            return false;
        }
        self.revision = Some(catalogue.topology_revision);
        self.network = GateNetwork::from_catalogue(catalogue);
        self.systems = catalogue
            .systems
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        self.beacons = catalogue
            .beacons
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        self.names = catalogue
            .systems
            .iter()
            .map(|system| system.name.to_lowercase())
            .collect();
        self.links.clear();
        for beacon in &catalogue.beacons {
            let Some(exit) = beacon
                .gate_exit
                .and_then(|id| self.beacons.get(&id))
                .map(|&index| &catalogue.beacons[index])
            else {
                continue;
            };
            if beacon.id > exit.id || exit.gate_exit != Some(beacon.id) {
                continue;
            }
            if let Some((&a, &b)) = self
                .systems
                .get(&beacon.system)
                .zip(self.systems.get(&exit.system))
            {
                self.links.push((a, b, beacon.id, exit.id));
            }
        }
        let anchor = self
            .systems
            .first_key_value()
            .map(|(_, &index)| catalogue.systems[index].position)
            .unwrap_or_default();
        self.positions = catalogue
            .systems
            .iter()
            .map(|system| system.position.relative_to(anchor) / LIGHT_YEAR_M)
            .collect();
        if self.positions.is_empty() {
            self.bounds = glam::DVec3::ONE;
            return true;
        }
        let (lo, hi) = self.positions.iter().fold(
            (
                glam::DVec3::splat(f64::INFINITY),
                glam::DVec3::splat(f64::NEG_INFINITY),
            ),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let center = (lo + hi) * 0.5;
        for position in &mut self.positions {
            *position -= center;
        }
        self.bounds = (hi - lo).max(glam::DVec3::ONE);
        true
    }
}
impl Cache {
    pub fn search(
        &self,
        catalogue: &NavigationCatalogue,
        text: &str,
        sovereignty: Option<Id>,
    ) -> Vec<usize> {
        let text = text.trim().to_lowercase();
        let mut matches: Vec<_> = catalogue
            .systems
            .iter()
            .enumerate()
            .filter(|(index, system)| {
                self.names[*index].contains(&text)
                    && sovereignty.is_none_or(|id| system.sovereignty == Some(id))
            })
            .map(|(index, _)| index)
            .collect();
        matches.sort_unstable_by(|&a, &b| {
            self.names[a]
                .cmp(&self.names[b])
                .then(catalogue.systems[a].id.cmp(&catalogue.systems[b].id))
        });
        matches
    }

    fn order_system(
        &self,
        catalogue: &NavigationCatalogue,
        celestial_systems: &BTreeMap<Id, Id>,
        order: &travel::Order,
    ) -> Option<Id> {
        use travel::{Destination, Order, Reference};
        match order {
            Order::Jump(id) => self
                .beacons
                .get(id)
                .and_then(|&index| catalogue.beacons[index].gate_exit)
                .and_then(|id| self.beacons.get(&id))
                .map(|&index| catalogue.beacons[index].system),
            Order::Dock(id)
            | Order::TravelTo(Destination::Beacon(id))
            | Order::Sublight(Destination::Beacon(id))
            | Order::Slip {
                destination: Destination::Beacon(id),
            }
            | Order::TravelTo(Destination::Relative {
                reference: Reference::Beacon(id),
                ..
            })
            | Order::Sublight(Destination::Relative {
                reference: Reference::Beacon(id),
                ..
            })
            | Order::Slip {
                destination:
                    Destination::Relative {
                        reference: Reference::Beacon(id),
                        ..
                    },
            } => self
                .beacons
                .get(id)
                .map(|&index| catalogue.beacons[index].system),
            Order::Slip {
                destination: Destination::Galactic(destination),
            }
            | Order::TravelTo(Destination::Galactic(destination))
            | Order::Sublight(Destination::Galactic(destination)) => {
                self.network.nearest(*destination)
            }
            Order::TravelTo(Destination::Relative {
                reference: Reference::Celestial(id),
                ..
            })
            | Order::Sublight(Destination::Relative {
                reference: Reference::Celestial(id),
                ..
            })
            | Order::Slip {
                destination:
                    Destination::Relative {
                        reference: Reference::Celestial(id),
                        ..
                    },
            } => celestial_systems.get(id).copied(),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct ActiveRoute {
    origin: Option<Id>,
    actions: Vec<travel::Order>,
    celestial_systems: BTreeMap<Id, Id>,
    pub gates: BTreeSet<Id>,
    pub systems: BTreeSet<usize>,
    pub slips: Vec<(usize, usize)>,
    pub stops: Vec<(usize, usize)>,
}

impl ActiveRoute {
    pub fn update(
        &mut self,
        cache: &Cache,
        catalogue: &NavigationCatalogue,
        celestial_systems: &BTreeMap<Id, Id>,
        origin: Option<Id>,
        orders: &[travel::QueuedOrder],
    ) {
        if self.origin == origin
            && self.celestial_systems == *celestial_systems
            && self
                .actions
                .iter()
                .eq(orders.iter().map(|stage| &stage.action))
        {
            return;
        }
        self.origin = origin;
        self.celestial_systems.clone_from(celestial_systems);
        self.actions = orders.iter().map(|stage| stage.action.clone()).collect();
        self.gates.clear();
        self.systems.clear();
        self.slips.clear();
        self.stops.clear();
        let mut cursor = origin;
        for (order_index, action) in self.actions.iter().enumerate() {
            let next = cache.order_system(catalogue, celestial_systems, action);
            if let Some(&system_index) = next.and_then(|id| cache.systems.get(&id)) {
                if self
                    .stops
                    .last()
                    .is_none_or(|&(_, last)| last != system_index)
                {
                    self.stops.push((order_index + 1, system_index));
                }
            }
            if let travel::Order::Jump(id) = action {
                self.gates.insert(*id);
            }
            if let Some((&a, &b)) = cursor
                .zip(next)
                .and_then(|(a, b)| cache.systems.get(&a).zip(cache.systems.get(&b)))
            {
                self.systems.extend([a, b]);
                if matches!(action, travel::Order::Slip { .. }) && a != b {
                    self.slips.push((a, b));
                }
            }
            cursor = next.or(cursor);
        }
    }
}
