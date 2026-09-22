use crate::generation::CatalogueStar;
use glam::DVec3;
use osg_space::GalacticPosition;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::OnceLock};

pub const LIGHT_YEAR_M: f64 = 9.460_730_472_580_8e15;
pub const INITIAL_SETTLEMENTS: usize = 3000;

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
    pub catalogue_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CivilizationMap {
    pub systems: Vec<SettledSystem>,
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
    let mut systems = Vec::with_capacity(INITIAL_SETTLEMENTS);
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
    assert_eq!(systems.len(), INITIAL_SETTLEMENTS);
    CivilizationMap { systems }
}

fn settlement(name: String, catalogue_id: String, position: GalacticPosition) -> SettledSystem {
    let location = position.relative_to(GalacticPosition::ZERO) / LIGHT_YEAR_M;
    let (sovereignty, alignment) = jurisdiction(location);
    SettledSystem {
        name,
        position,
        sovereignty: sovereignty.into(),
        alignment,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn initial_settlements_have_unique_identities_and_authored_jurisdictions() {
        let map = map();
        assert_eq!(map.systems.len(), INITIAL_SETTLEMENTS);
        let mut names = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut sovereigns = BTreeMap::<&str, usize>::new();
        for system in &map.systems {
            assert!(names.insert(&system.name));
            assert!(identities.insert(&system.catalogue_id));
            *sovereigns.entry(&system.sovereignty).or_default() += 1;
        }
        assert_eq!(sovereigns.len(), 10);
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
}
