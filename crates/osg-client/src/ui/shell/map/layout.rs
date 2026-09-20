use super::*;
use osg_universe::civilization::LIGHT_YEAR_M;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Cache {
    revision: Option<u64>,
    pub systems: BTreeMap<Id, usize>,
    pub beacons: BTreeMap<Id, usize>,
    pub positions: Vec<glam::DVec3>,
    pub names: Vec<String>,
    pub bounds: glam::DVec3,
    pub tree: spatial::Tree,
    pub inhabited_tree: spatial::Tree,
    inhabited: Option<std::sync::Arc<osg_model::InhabitedDirectory>>,
    origin: GalacticPosition,
    center: glam::DVec3,
    catalogue_storage: Option<(usize, usize, Option<Id>, Option<Id>)>,
}

impl Cache {
    pub fn update(&mut self, catalogue: &NavigationCatalogue) -> bool {
        if self.revision == Some(catalogue.topology_revision) {
            return false;
        }
        self.revision = Some(catalogue.topology_revision);
        self.beacons = catalogue
            .beacons
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let storage = (
            catalogue.systems.as_ptr() as usize,
            catalogue.systems.len(),
            catalogue.systems.first().map(|s| s.id),
            catalogue.systems.last().map(|s| s.id),
        );
        if self.catalogue_storage == Some(storage) {
            return true;
        }
        self.catalogue_storage = Some(storage);
        self.inhabited = None;
        self.systems = catalogue
            .systems
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        self.names = catalogue
            .systems
            .iter()
            .map(|system| system.name.to_lowercase())
            .collect();
        let anchor = self
            .systems
            .first_key_value()
            .map(|(_, &index)| catalogue.systems[index].position)
            .unwrap_or_default();
        self.origin = anchor;
        self.positions = catalogue
            .systems
            .iter()
            .map(|system| system.position.relative_to(anchor) / LIGHT_YEAR_M)
            .collect();
        if self.positions.is_empty() {
            self.bounds = glam::DVec3::ONE;
            self.center = glam::DVec3::ZERO;
            self.tree = spatial::Tree::default();
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
        self.center = center;
        for position in &mut self.positions {
            *position -= center;
        }
        self.bounds = (hi - lo).max(glam::DVec3::ONE);
        self.tree = spatial::Tree::new(&self.positions);
        true
    }
}
impl Cache {
    pub fn update_inhabited(
        &mut self,
        directory: &std::sync::Arc<osg_model::InhabitedDirectory>,
    ) -> bool {
        if self
            .inhabited
            .as_ref()
            .is_some_and(|previous| std::sync::Arc::ptr_eq(previous, directory))
        {
            return false;
        }
        let indices = directory
            .systems
            .iter()
            .filter_map(|id| self.systems.get(id).copied())
            .collect();
        self.inhabited_tree = spatial::Tree::from_indices(&self.positions, indices);
        self.inhabited = Some(directory.clone());
        true
    }

    pub fn nearest(
        &self,
        catalogue: &NavigationCatalogue,
        position: GalacticPosition,
    ) -> Option<Id> {
        self.tree
            .nearest(
                &self.positions,
                position.relative_to(self.origin) / LIGHT_YEAR_M - self.center,
            )
            .map(|index| catalogue.systems[index].id)
    }

    pub fn search(
        &self,
        catalogue: &NavigationCatalogue,
        text: &str,
        sovereignty: Option<Id>,
        visible: &BTreeSet<usize>,
    ) -> Vec<usize> {
        let text = text.trim().to_lowercase();
        let mut matches: Vec<_> = visible
            .iter()
            .copied()
            .filter(|&index| {
                let system = &catalogue.systems[index];
                self.names[index].contains(&text)
                    && sovereignty.is_none_or(|id| system.sovereignty == Some(id))
            })
            .collect();
        matches.sort_unstable_by(|&a, &b| {
            self.names[a]
                .cmp(&self.names[b])
                .then(catalogue.systems[a].id.cmp(&catalogue.systems[b].id))
        });
        matches
    }

    fn order_system(&self, catalogue: &NavigationCatalogue, order: &travel::Order) -> Option<Id> {
        if let travel::Order::TravelToSystem(id) = order {
            return Some(*id);
        }
        use travel::{Destination, Order, Reference};
        match order {
            Order::Dock(id)
            | Order::TravelTo(Destination::Beacon(id))
            | Order::Sublight(Destination::Beacon(id))
            | Order::Slip {
                destination: Destination::Beacon(id),
                ..
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
                ..
            } => self
                .beacons
                .get(id)
                .and_then(|&index| catalogue.beacons[index].systems.first().copied()),
            Order::Slip {
                destination: Destination::Galactic(destination),
                ..
            }
            | Order::TravelTo(Destination::Galactic(destination))
            | Order::Sublight(Destination::Galactic(destination)) => {
                self.nearest(catalogue, *destination)
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
                ..
            } => Some(id.system),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct ActiveRoute {
    origin: Option<Id>,
    orders: Vec<travel::QueuedOrder>,
    pub systems: BTreeSet<usize>,
    pub slips: Vec<(usize, usize, f64, Option<f64>)>,
    pub stops: Vec<(usize, usize)>,
}

impl ActiveRoute {
    pub fn update(
        &mut self,
        cache: &Cache,
        catalogue: &NavigationCatalogue,
        origin: Option<Id>,
        orders: &[travel::QueuedOrder],
    ) -> bool {
        if self.origin == origin && self.orders == orders {
            return false;
        }
        self.origin = origin;
        self.orders = orders.to_vec();
        self.systems.clear();
        self.slips.clear();
        self.stops.clear();
        let mut cursor = origin;
        for (order_index, stage) in self.orders.iter().enumerate() {
            let action = &stage.action;
            let next = cache.order_system(catalogue, action);
            if let Some(&system_index) = next.and_then(|id| cache.systems.get(&id)) {
                if self
                    .stops
                    .last()
                    .is_none_or(|&(_, last)| last != system_index)
                {
                    self.stops.push((order_index + 1, system_index));
                }
            }
            if let Some((&a, &b)) = cursor
                .zip(next)
                .and_then(|(a, b)| cache.systems.get(&a).zip(cache.systems.get(&b)))
            {
                self.systems.extend([a, b]);
                if let travel::Order::Slip { speed_ly_s, .. } = action
                    && a != b
                {
                    self.slips
                        .push((a, b, *speed_ly_s, stage.estimated_loss_ppm));
                }
            }
            cursor = next.or(cursor);
        }
        true
    }
}
