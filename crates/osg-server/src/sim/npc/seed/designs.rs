use std::sync::Arc;

use anyhow::{Context, Result};
use osg_ships::{
    CHATTER_CONTROLLER, Catalogue, CompiledShipDesign, Firmware, ShipBlueprint, Tank,
    expedition_patrol, ntr_patrol,
};

const STATION_BLUEPRINT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/ships/neris-anchorage.ship"
));
const PATROL_BLUEPRINT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/ships/missile-patrol.ship"
));

const STATION_HANGAR: u64 = 2;
const STATION_BEACON: u64 = 4;
const STATION_REFINERY: u64 = 12;
const STATION_CARGO_HANDLER: u64 = 17;

pub(super) struct Designs {
    pub service_station: Arc<CompiledShipDesign>,
    pub industrial_station: Arc<CompiledShipDesign>,
    pub mining_station: Arc<CompiledShipDesign>,
    pub defense_station: Arc<CompiledShipDesign>,
    pub freighter: Arc<CompiledShipDesign>,
    pub patrol: Arc<CompiledShipDesign>,
    pub survey: Arc<CompiledShipDesign>,
}

impl Designs {
    pub(super) fn new(catalogue: &Catalogue) -> Result<Self> {
        let base = ShipBlueprint::from_bytes(STATION_BLUEPRINT)?;

        let mut service = base.clone();
        service.parts.retain(|part| part.id < STATION_REFINERY);
        add_warehouse(&mut service, STATION_BEACON);
        add_launcher(&mut service, "left", "right");

        let mut defense = service.clone();
        add_launcher(&mut defense, "bottom", "top");

        let mut mining = base.clone();
        mining.parts.retain(|part| part.id <= STATION_REFINERY);
        add_warehouse(&mut mining, STATION_REFINERY);
        add_launcher(&mut mining, "left", "right");

        let mut industrial = base;
        industrial.attach("workshop_4m", STATION_CARGO_HANDLER, "top", "bottom", 0);
        add_launcher(&mut industrial, "left", "right");
        add_launcher(&mut industrial, "bottom", "top");

        let mut freighter = ntr_patrol();
        let cargo = freighter
            .parts
            .iter_mut()
            .find(|part| part.prototype == "storage")
            .context("NTR freighter base has no storage mount")?;
        cargo.prototype = "cargo_hold_4m".into();

        let patrol = ShipBlueprint::from_bytes(PATROL_BLUEPRINT)?;
        let survey = expedition_patrol();

        Ok(Self {
            service_station: compile(service, "Public service station", catalogue)?,
            industrial_station: compile(industrial, "Industrial station", catalogue)?,
            mining_station: compile(mining, "Refinery station", catalogue)?,
            defense_station: compile(defense, "Defense station", catalogue)?,
            freighter: compile(freighter, "NTR cargo tender", catalogue)?,
            patrol: compile(patrol, "Missile patrol", catalogue)?,
            survey: compile(survey, "Expedition survey ship", catalogue)?,
        })
    }
}

fn add_warehouse(ship: &mut ShipBlueprint, parent: u64) {
    let warehouse = ship.attach("station_cargo_32m", parent, "aft", "fore", 0);
    let handler = ship.attach("cargo_handler_4m", warehouse, "top", "bottom", 0);
    let workshop = ship.attach("workshop_4m", handler, "top", "bottom", 0);
    ship.attach("power_coupler_2m", workshop, "top", "bottom", 0);
}

fn add_launcher(ship: &mut ShipBlueprint, socket: &str, plug: &str) {
    ship.attach("missile_launcher_2m", STATION_HANGAR, socket, plug, 0);
    ship.parts.last_mut().unwrap().tanks.push(Tank {
        resource: "interceptor_missile".into(),
        volume_m3: 6.0,
        initial_fill: 1.0,
    });
}

fn compile(
    mut blueprint: ShipBlueprint,
    name: &str,
    catalogue: &Catalogue,
) -> Result<Arc<CompiledShipDesign>> {
    blueprint.name = name.into();
    blueprint.firmware = Firmware::Custom(CHATTER_CONTROLLER.to_vec());

    blueprint
        .compile(catalogue)
        .map(Arc::new)
        .with_context(|| format!("compile NPC design {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::industry::IndustryCapability;
    use osg_ships::{Equipment, utilities::UtilityDef};

    #[test]
    fn population_designs_compile_with_physical_services_and_finite_supplies() {
        let designs = Designs::new(&Catalogue::builtin()).unwrap();
        let all = [
            &designs.service_station,
            &designs.industrial_station,
            &designs.mining_station,
            &designs.defense_station,
            &designs.freighter,
            &designs.patrol,
            &designs.survey,
        ];

        for design in all {
            assert_eq!(
                design.blueprint.firmware,
                Firmware::Custom(CHATTER_CONTROLLER.to_vec()),
            );
            assert!(design.blueprint.avionics.sensor_enabled);
            assert!(has_filled_tank(design, "reactor_fuel"));
            assert!(design.parts.iter().all(|part| {
                part.definition.model.is_some()
                    && (part.placed.id == 1 || part.placed.attachment.is_some())
            }));
        }

        for station in [
            &designs.service_station,
            &designs.industrial_station,
            &designs.mining_station,
            &designs.defense_station,
        ] {
            for predicate in [
                (|utility: &UtilityDef| matches!(utility, UtilityDef::Docking { .. }))
                    as fn(&UtilityDef) -> bool,
                |utility| matches!(utility, UtilityDef::Beacon { .. }),
                |utility| matches!(utility, UtilityDef::CargoHandler { .. }),
                |utility| matches!(utility, UtilityDef::PowerCoupler { .. }),
                |utility| matches!(utility, UtilityDef::Workshop { .. }),
                |utility| matches!(utility, UtilityDef::MissileLauncher { .. }),
            ] {
                assert!(station.parts.iter().any(|part| {
                    matches!(&part.definition.equipment, Equipment::Utility { utility } if predicate(utility))
                }));
            }

            assert!(station.capacity_m3 >= 40_000.0);
            assert!(
                station
                    .parts
                    .iter()
                    .any(|part| { matches!(part.definition.equipment, Equipment::Reactor { .. }) })
            );
            assert!(
                station.parts.iter().any(|part| {
                    matches!(part.definition.equipment, Equipment::Radiator { .. })
                })
            );

            for launcher in station.parts.iter().filter(|part| {
                matches!(
                    part.definition.equipment,
                    Equipment::Utility {
                        utility: UtilityDef::MissileLauncher { .. }
                    }
                )
            }) {
                assert_eq!(launcher.placed.tanks.len(), 1);
                let tank = &launcher.placed.tanks[0];
                assert_eq!(tank.resource, "interceptor_missile");
                assert_eq!(tank.volume_m3, 6.0);
                assert_eq!(tank.initial_fill, 1.0);
            }
        }

        for capability in [
            IndustryCapability::Refinery,
            IndustryCapability::FuelPlant,
            IndustryCapability::Fabricator,
        ] {
            assert!(has_factory(&designs.industrial_station, capability));
        }
        assert!(designs.industrial_station.parts.iter().any(|part| {
            matches!(
                part.definition.equipment,
                Equipment::Utility {
                    utility: UtilityDef::Shipyard { .. }
                }
            )
        }));
        assert!(has_factory(
            &designs.mining_station,
            IndustryCapability::Refinery
        ));
        assert!(!has_factory(
            &designs.service_station,
            IndustryCapability::Refinery
        ));

        assert!(designs.freighter.capacity_m3 >= 90.0);
        assert!(designs.freighter.max_torque > 0.0);
        assert!(has_filled_tank(&designs.freighter, "water"));
        assert!(designs.freighter.parts.iter().any(|part| {
            matches!(
                &part.definition.equipment,
                Equipment::ThermalEngine { propellant_resource, .. }
                    if propellant_resource == "water"
            )
        }));
        assert!(has_filled_tank(&designs.patrol, "interceptor_missile"));
        assert!(has_filled_tank(&designs.patrol, "micropulse_charge"));
        assert!(has_filled_tank(&designs.survey, "micropulse_charge"));
        assert!(designs.survey.capacity_m3 > 0.0);
    }

    fn has_factory(design: &CompiledShipDesign, expected: IndustryCapability) -> bool {
        design.parts.iter().any(|part| {
            matches!(
                part.definition.equipment,
                Equipment::Utility {
                    utility: UtilityDef::Factory { capability, .. }
                } if capability == expected
            )
        })
    }

    fn has_filled_tank(design: &CompiledShipDesign, resource: &str) -> bool {
        design.blueprint.parts.iter().any(|part| {
            part.tanks.iter().any(|tank| {
                tank.resource == resource && tank.volume_m3 > 0.0 && tank.initial_fill > 0.0
            })
        })
    }
}
