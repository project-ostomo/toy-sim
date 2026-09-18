use crate::{EntityId, NavigationCatalogue};
use std::collections::{BTreeMap, VecDeque};

pub fn gate_route(
    catalogue: &NavigationCatalogue,
    origin: EntityId,
    destination: EntityId,
) -> Option<Vec<EntityId>> {
    let mut queue = VecDeque::from([origin]);
    let mut previous = BTreeMap::from([(origin, None)]);
    while let Some(system) = queue.pop_front() {
        if system == destination {
            let mut route = Vec::new();
            let mut current = destination;
            while let Some(&(parent, entry)) = previous.get(&current)?.as_ref() {
                route.push(entry);
                current = parent;
            }
            route.reverse();
            return Some(route);
        }
        for gate in catalogue
            .beacons
            .iter()
            .filter(|gate| gate.system == system)
        {
            let Some(exit) = gate
                .gate_exit
                .and_then(|id| catalogue.beacons.iter().find(|b| b.id == id))
            else {
                continue;
            };
            if exit.gate_exit != Some(gate.id) || previous.contains_key(&exit.system) {
                continue;
            }
            previous.insert(exit.system, Some((system, gate.id)));
            queue.push_back(exit.system);
        }
    }
    None
}
