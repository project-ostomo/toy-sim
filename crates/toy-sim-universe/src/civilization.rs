use crate::generation::{CatalogueStar, seed};
use glam::DVec3;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::OnceLock};
use toy_sim_space::GalacticPosition;
use toy_sim_spatial::{Entry, SpatialHash};

pub const LIGHT_YEAR_M: f64 = 9.460_730_472_580_8e15;
pub const INHABITED_SYSTEMS: usize = 3000;
pub const GENERATION_VERSION: u32 = 1;
pub const MAX_SYSTEM_CONNECTIONS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Alignment {
    Use,
    Lfs,
    Independent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettledSystem {
    pub name: String,
    pub position: GalacticPosition,
    pub sovereignty: String,
    pub alignment: Alignment,
    pub population: u64,
    pub catalogue_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateLinkKind {
    Trunk,
    Regional,
    Mesh,
    Concord,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GateLink {
    pub a: usize,
    pub b: usize,
    pub kind: GateLinkKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CivilizationMap {
    pub generation_version: u32,
    pub systems: Vec<SettledSystem>,
    pub links: Vec<GateLink>,
}

pub fn stars() -> &'static [CatalogueStar] {
    static STARS: OnceLock<Vec<CatalogueStar>> = OnceLock::new();
    STARS.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Names {
            names: std::collections::BTreeMap<String, String>,
        }

        let aliases: Names = serde_json::from_str(include_str!("../data/star-names.json"))
            .expect("bundled star names");
        let mut stars: Vec<CatalogueStar> =
            serde_json::from_str(include_str!("../data/inhabited-stars.json"))
                .expect("bundled nearby star catalogue");
        for star in &mut stars {
            if star.name.starts_with("Gaia ") {
                if let Some(name) = aliases.names.get(&star.id) {
                    star.name.clone_from(name);
                }
            }
        }
        stars
    })
}

pub fn map() -> &'static CivilizationMap {
    static MAP: OnceLock<CivilizationMap> = OnceLock::new();
    MAP.get_or_init(generate)
}

struct Region {
    name: &'static str,
    center_ly: [f64; 3],
    alignment: Alignment,
    weight: f64,
}

const REGIONS: [Region; 9] = [
    Region {
        name: "Helion Commonwealth",
        center_ly: [90.0, 10.0, 5.0],
        alignment: Alignment::Lfs,
        weight: 1.0,
    },
    Region {
        name: "Aurora Compact",
        center_ly: [40.0, 83.0, 40.0],
        alignment: Alignment::Lfs,
        weight: 1.0,
    },
    Region {
        name: "Meridian League",
        center_ly: [-55.0, 65.0, 45.0],
        alignment: Alignment::Lfs,
        weight: 1.0,
    },
    Region {
        name: "St Raphael Commonwealth",
        center_ly: [30.0, -68.0, 60.0],
        alignment: Alignment::Lfs,
        weight: 1.0,
    },
    Region {
        name: "Vesper Freeports",
        center_ly: [-30.0, -85.0, -15.0],
        alignment: Alignment::Lfs,
        weight: 1.0,
    },
    Region {
        name: "Lyra Research Compact",
        center_ly: [20.0, 20.0, -100.0],
        alignment: Alignment::Lfs,
        weight: 1.0,
    },
    Region {
        name: "Nova Partenia",
        center_ly: [-75.0, 25.0, -20.0],
        alignment: Alignment::Independent,
        weight: 2.3,
    },
    Region {
        name: "Concord Free State",
        center_ly: [-75.0, -40.0, 60.0],
        alignment: Alignment::Independent,
        weight: 1.0,
    },
    Region {
        name: "Terminus Protectorate",
        center_ly: [-70.0, -20.0, -70.0],
        alignment: Alignment::Independent,
        weight: 1.0,
    },
];

fn authored_position(name: &str) -> [f64; 3] {
    match name {
        "Sol" => [0.0, 0.0, 0.0],
        "Helion system" => REGIONS[0].center_ly,
        "Vesper system" => REGIONS[4].center_ly,
        "Aurora system" => REGIONS[1].center_ly,
        "Lyra system" => REGIONS[5].center_ly,
        "Cinder system" => REGIONS[3].center_ly,
        "Meridian system" => REGIONS[2].center_ly,
        "Havoc system" => REGIONS[7].center_ly,
        "Elysium system" => REGIONS[6].center_ly,
        "Terminus system" => REGIONS[8].center_ly,
        _ => panic!("authored system has no political location: {name}"),
    }
}

fn generate() -> CivilizationMap {
    let mut systems = Vec::with_capacity(INHABITED_SYSTEMS);
    let mut names = BTreeSet::new();
    for config in crate::handcrafted_configs() {
        let name = config.name.to_string();
        let position = GalacticPosition::from_meters(
            DVec3::from_array(authored_position(&name)) * LIGHT_YEAR_M,
        );
        names.insert(name.clone());
        systems.push(settlement(
            name.clone(),
            format!("authored:{name}"),
            position,
        ));
    }
    let authored_count = systems.len();
    assert_eq!(authored_count, 10);
    for star in stars() {
        let position =
            GalacticPosition::from_meters(DVec3::from_array(star.position_ly) * LIGHT_YEAR_M);
        let mut name = star.name.clone();
        if !names.insert(name.clone()) {
            name = format!("{} ({})", name, star.id);
            assert!(names.insert(name.clone()), "duplicate catalogue identity");
        }
        systems.push(settlement(name, star.id.clone(), position));
    }
    assert_eq!(systems.len(), INHABITED_SYSTEMS);

    let mut network = Network::new(&systems);
    for a in 0..authored_count - 1 {
        network.connect(a, a + 1, GateLinkKind::Concord);
    }
    let mut connected = SpatialHash::default();
    for (index, system) in systems.iter().take(authored_count).enumerate() {
        connected.insert(index as u32, entry(system));
    }
    let mut expansion: Vec<_> = (authored_count..systems.len()).collect();
    expansion.sort_by(|&a, &b| {
        distance_from_sol(&systems[a])
            .total_cmp(&distance_from_sol(&systems[b]))
            .then(a.cmp(&b))
    });
    for child in expansion {
        let parents =
            connected.nearest_filtered(systems[child].position, f64::INFINITY, 1, |candidate| {
                let parent = candidate as usize;
                (systems[parent].sovereignty == systems[child].sovereignty
                    && network.degree[parent] < MAX_SYSTEM_CONNECTIONS)
                    .then_some(candidate as u64)
            });
        let parent = *parents
            .first()
            .expect("sovereignty has an expansion anchor") as usize;
        let kind = if systems[child].alignment == Alignment::Use {
            GateLinkKind::Trunk
        } else {
            GateLinkKind::Regional
        };
        assert!(network.connect(child, parent, kind));
        connected.insert(child as u32, entry(&systems[child]));
    }

    for node in 0..systems.len() {
        let target_degree = match systems[node].alignment {
            Alignment::Use => continue,
            Alignment::Lfs => 4,
            Alignment::Independent => 3,
        };
        while network.degree[node] < target_degree {
            let neighbors =
                connected.nearest_filtered(systems[node].position, f64::INFINITY, 1, |candidate| {
                    let other = candidate as usize;
                    let compatible = match systems[node].alignment {
                        Alignment::Lfs => systems[other].alignment == Alignment::Lfs,
                        _ => systems[other].sovereignty == systems[node].sovereignty,
                    };
                    (compatible && network.available(node, other)).then_some(candidate as u64)
                });
            let Some(&other) = neighbors.first() else {
                break;
            };
            let kind = if systems[node].alignment == Alignment::Lfs {
                GateLinkKind::Mesh
            } else {
                GateLinkKind::Regional
            };
            network.connect(node, other as usize, kind);
        }
    }

    for node in 0..systems.len() {
        let selection = seed("gate-crosslink", systems[node].catalogue_id.as_bytes());
        let mature_core = systems[node].alignment == Alignment::Use
            && distance_from_sol(&systems[node]) < 35.0 * LIGHT_YEAR_M
            && selection[0] % 8 == 0;
        let border_route = selection[1] % 32 == 0;
        if !mature_core && !border_route {
            continue;
        }
        let neighbors = connected.nearest_filtered(
            systems[node].position,
            32.0 * LIGHT_YEAR_M,
            1,
            |candidate| {
                let other = candidate as usize;
                let eligible = if mature_core {
                    systems[other].alignment == Alignment::Use
                } else {
                    systems[other].alignment != systems[node].alignment
                };
                (eligible && network.available(node, other)).then_some(candidate as u64)
            },
        );
        if let Some(&other) = neighbors.first() {
            network.connect(
                node,
                other as usize,
                if mature_core {
                    GateLinkKind::Regional
                } else {
                    GateLinkKind::Concord
                },
            );
        }
    }

    network
        .links
        .retain(|link| systems[link.a].name != "Sol" && systems[link.b].name != "Sol");
    network.links.sort_by_key(|link| (link.a, link.b));
    CivilizationMap {
        generation_version: GENERATION_VERSION,
        systems,
        links: network.links,
    }
}

fn entry(system: &SettledSystem) -> Entry {
    Entry {
        position: system.position,
        radius_m: 0.0,
        luminosity: 0.0,
    }
}

fn distance_from_sol(system: &SettledSystem) -> f64 {
    system.position.relative_to(GalacticPosition::ZERO).length()
}

fn settlement(name: String, catalogue_id: String, position: GalacticPosition) -> SettledSystem {
    let location = position.relative_to(GalacticPosition::ZERO) / LIGHT_YEAR_M;
    let (sovereignty, alignment) = jurisdiction(location);
    let mut rng = ChaCha20Rng::from_seed(seed("settlement", catalogue_id.as_bytes()));
    let (minimum, maximum) = match alignment {
        Alignment::Use => (7.0, 10.2),
        Alignment::Lfs => (5.2, 9.3),
        Alignment::Independent => (5.0, 8.9),
    };
    let maturity = (1.0 - location.length() / 170.0).max(0.2);
    let population = if name == "Sol" {
        36_000_000_000
    } else {
        (10f64.powf(rng.random_range(minimum..maximum)) * maturity) as u64
    };
    SettledSystem {
        name,
        position,
        sovereignty: sovereignty.into(),
        alignment,
        population,
        catalogue_id,
    }
}

fn jurisdiction(position: DVec3) -> (&'static str, Alignment) {
    let radius = position.length();
    if radius < 52.0 {
        return ("USE", Alignment::Use);
    }
    if position.distance(DVec3::from_array(REGIONS[6].center_ly)) < 15.0 {
        return ("Nova Partenia", Alignment::Independent);
    }
    if position.x < -0.82 * radius && radius < 103.0 {
        return ("USE", Alignment::Use);
    }
    let region = REGIONS
        .iter()
        .min_by(|a, b| {
            let score = |region: &Region| {
                position.distance_squared(DVec3::from_array(region.center_ly)) * region.weight
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap();
    (region.name, region.alignment)
}

struct Network {
    degree: Vec<usize>,
    linked: BTreeSet<(usize, usize)>,
    links: Vec<GateLink>,
}

impl Network {
    fn new(systems: &[SettledSystem]) -> Self {
        Self {
            degree: vec![0; systems.len()],
            linked: BTreeSet::new(),
            links: Vec::new(),
        }
    }

    fn available(&self, a: usize, b: usize) -> bool {
        a != b
            && self.degree[a] < MAX_SYSTEM_CONNECTIONS
            && self.degree[b] < MAX_SYSTEM_CONNECTIONS
            && !self.linked.contains(&(a.min(b), a.max(b)))
    }

    fn connect(&mut self, a: usize, b: usize, kind: GateLinkKind) -> bool {
        if !self.available(a, b) {
            return false;
        }
        let (a, b) = (a.min(b), a.max(b));
        self.linked.insert((a, b));
        self.degree[a] += 1;
        self.degree[b] += 1;
        self.links.push(GateLink { a, b, kind });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn map_isolates_sol_and_has_distinct_political_networks() {
        let map = map();
        assert_eq!(map.systems.len(), INHABITED_SYSTEMS);
        let mut adjacency = vec![Vec::new(); map.systems.len()];
        let mut links = BTreeSet::new();
        for link in &map.links {
            assert_ne!(link.a, link.b);
            assert!(links.insert((link.a, link.b)));
            adjacency[link.a].push(link.b);
            adjacency[link.b].push(link.a);
        }
        let mut reached = BTreeSet::new();
        let mut pending = vec![1];
        while let Some(node) = pending.pop() {
            if reached.insert(node) {
                pending.extend(&adjacency[node]);
            }
        }
        assert_eq!(reached, BTreeSet::from([1]));
        assert!(adjacency[1].is_empty());
        assert!(
            adjacency
                .iter()
                .all(|neighbors| neighbors.len() <= MAX_SYSTEM_CONNECTIONS)
        );
        for a in 0..9 {
            assert_eq!(links.contains(&(a, a + 1)), a > 1);
        }
        let mut names = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut sovereigns = BTreeMap::<&str, usize>::new();
        let mut union = 0;
        let mut league = 0;
        let mut independent = 0;
        let mut union_degree = 0;
        let mut league_degree = 0;
        for (index, system) in map.systems.iter().enumerate() {
            assert!(names.insert(&system.name));
            assert!(identities.insert(&system.catalogue_id));
            assert!(distance_from_sol(system) <= 125.000_001 * LIGHT_YEAR_M);
            assert!(system.population > 0);
            *sovereigns.entry(&system.sovereignty).or_default() += 1;
            match system.alignment {
                Alignment::Use => {
                    union += 1;
                    union_degree += adjacency[index].len();
                }
                Alignment::Lfs => {
                    league += 1;
                    league_degree += adjacency[index].len();
                    assert!(distance_from_sol(system) >= 52.0 * LIGHT_YEAR_M);
                }
                Alignment::Independent => independent += 1,
            }
        }
        assert!(union > 250 && league > 1000 && independent > 250);
        assert_eq!(sovereigns.len(), 10);
        assert!(sovereigns.values().all(|&count| count >= 20));
        assert!(union_degree as f64 / (union as f64) < 2.5);
        assert!(league_degree as f64 / (league as f64) > 3.8);
        assert!(
            map.links
                .iter()
                .filter(|link| link.kind == GateLinkKind::Trunk)
                .count()
                > 250
        );
        assert!(
            map.links
                .iter()
                .filter(|link| link.kind == GateLinkKind::Mesh)
                .count()
                > 1000
        );
        assert!(
            map.links
                .iter()
                .filter(|link| link.kind == GateLinkKind::Concord)
                .count()
                > 15
        );
        assert_eq!(map.systems[0].sovereignty, "Helion Commonwealth");
        assert_eq!(map.systems[1].name, "Sol");
        assert_eq!(map.systems[1].position, GalacticPosition::ZERO);
        assert_eq!(map.systems[1].sovereignty, "USE");
        assert_eq!(map.systems[8].sovereignty, "Nova Partenia");
        assert_eq!(map.systems[8].alignment, Alignment::Independent);
    }

    #[test]
    fn map_generation_is_reproducible_and_catalogue_axes_match_the_background() {
        assert_eq!(
            serde_json::to_vec(map()).unwrap(),
            serde_json::to_vec(&generate()).unwrap()
        );
        let sirius = stars().iter().find(|star| star.name == "Sirius").unwrap();
        let position = DVec3::from_array(sirius.position_ly).normalize();
        let ra = 101.2872_f64.to_radians();
        let dec = -16.7161_f64.to_radians();
        let direction = DVec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin());
        assert!(position.dot(direction) > 0.999);
    }

    #[test]
    fn all_gate_routes_have_short_paths_that_fit_the_navigation_queue() {
        let map = map();
        let mut adjacency = vec![Vec::new(); map.systems.len()];
        for link in &map.links {
            adjacency[link.a].push(link.b);
            adjacency[link.b].push(link.a);
        }
        let mut diameter = 0;
        for origin in 0..map.systems.len() {
            let mut distances = vec![u16::MAX; map.systems.len()];
            distances[origin] = 0;
            let mut pending = std::collections::VecDeque::from([origin]);
            while let Some(node) = pending.pop_front() {
                for &neighbor in &adjacency[node] {
                    if distances[neighbor] == u16::MAX {
                        distances[neighbor] = distances[node] + 1;
                        pending.push_back(neighbor);
                    }
                }
            }
            diameter = diameter.max(
                distances
                    .into_iter()
                    .filter(|&distance| distance != u16::MAX)
                    .max()
                    .unwrap(),
            );
        }
        let mut sovereigns = BTreeMap::<&str, usize>::new();
        for system in &map.systems {
            *sovereigns.entry(&system.sovereignty).or_default() += 1;
        }
        eprintln!(
            "{} systems, {} links, exact gate-hop diameter {}, sovereignties {:?}",
            map.systems.len(),
            map.links.len(),
            diameter,
            sovereigns
        );
        assert!(
            diameter <= 100,
            "generated routes exceed the standard flight computer queue"
        );
    }
}
