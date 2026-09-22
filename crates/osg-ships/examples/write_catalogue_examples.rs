use anyhow::Result;
use osg_ships::*;

fn tank(ship: &mut ShipBlueprint, resource: &str, volume_m3: f64, initial_fill: f64) {
    ship.parts[0].tanks.push(Tank {
        resource: resource.into(),
        volume_m3,
        initial_fill,
    });
}

fn common(name: &str) -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: name.into(),
        ..Default::default()
    };
    ship.attach("fuselage_4m", 0, "", "", 0);
    ship.attach("fuselage_end_4m", 1, "fore", "aft", 0);
    ship.attach("command_2m", 2, "front", "back", 0);
    ship.attach("battery_2m", 1, "right", "left", 0);
    ship.attach("shield_emitter_2m", 1, "left", "right", 0);
    ship.attach("shield_reservoir_2m", 5, "left", "right", 0);
    ship.attach("heat_sink_2m", 4, "right", "left", 0);
    ship.attach("torquer", 1, "top", "bottom", 0);
    ship.attach("sensor_passive_2m", 3, "front", "back", 0);
    tank(&mut ship, "reactor_fuel", 0.2, 0.5);
    tank(&mut ship, "spent_fuel", 0.2, 0.0);
    ship
}

fn main() -> Result<()> {
    let output = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .unwrap_or_else(|| "assets/ships".into()),
    );
    std::fs::create_dir_all(&output)?;
    let catalogue = Catalogue::builtin();
    let mut combat = common("Micropulse patrol ship");
    combat.attach("micropulse_engine_4m", 1, "aft", "fore", 0);
    combat.attach("reactor_compact_2m", 9, "front", "back", 0);
    combat.attach("railgun_compact", 1, "bottom", "top", 0);
    tank(&mut combat, "micropulse_charge", 50., 0.5);
    tank(&mut combat, "railgun_dart", 0.01, 0.5);
    tank(&mut combat, "propellant", 1., 0.5);

    let mut utility = common("Water NTR utility ship");
    utility.attach("ntr_water_4m", 1, "aft", "fore", 0);
    utility.attach("reactor_compact_2m", 9, "front", "back", 0);
    utility.attach("cargo_handler_4m", 1, "bottom", "top", 0);
    utility.attach("emergency_cooler_2m", 7, "right", "left", 0);
    tank(&mut utility, "water", 50., 0.8);

    let mut freighter = common("Breeder electric freighter");
    let reactor = freighter.attach("reactor_breeder_4m", 1, "back", "front", 0);
    freighter.attach("electric_engine_1m", reactor, "back", "front", 0);
    freighter.attach("cargo_hold_4m", reactor, "top", "bottom", 0);
    freighter.attach("fuel_processor_4m", reactor, "bottom", "top", 0);
    freighter.attach("radiator_4m", reactor, "right", "left", 0);
    tank(&mut freighter, "propellant", 40., 0.5);
    tank(&mut freighter, "fertile_feedstock", 1., 0.5);
    tank(&mut freighter, "bred_fuel", 1., 0.0);

    let mut station = ShipBlueprint {
        name: "Neris Anchorage".into(),
        ..Default::default()
    };
    station.attach("station_core_32m", 0, "", "", 0);
    station.attach("station_hangar_64m", 1, "fore", "aft", 0);
    station.attach("station_habitat_200m", 1, "aft", "fore", 0);
    station.attach("directory_transmitter_48m", 3, "aft", "fore", 0);
    station.attach("reactor_hot_4m", 1, "left", "right", 0);
    station.attach("radiator_32m", 5, "left", "right", 0);
    station.attach("station_life_support_8m", 1, "right", "left", 0);
    station.attach("battery", 1, "top", "bottom", 0);
    station.attach("command_2m", 1, "bottom", "top", 0);
    station.attach("cargo_hold_8m", 2, "top", "bottom", 0);
    station.attach("autocannon_compact", 2, "right", "left", 0);
    station.parts[0].tanks = vec![
        Tank {
            resource: "reactor_fuel".into(),
            volume_m3: 2.,
            initial_fill: 0.5,
        },
        Tank {
            resource: "spent_fuel".into(),
            volume_m3: 2.,
            initial_fill: 0.,
        },
        Tank {
            resource: "life_support_supplies".into(),
            volume_m3: 100.,
            initial_fill: 1.,
        },
        Tank {
            resource: "autocannon_round".into(),
            volume_m3: 1.,
            initial_fill: 1.,
        },
    ];

    for (filename, ship) in [
        ("neris-anchorage.ship", station),
        ("micropulse-patrol.ship", combat),
        ("ntr-utility.ship", utility),
        ("breeder-freighter.ship", freighter),
        ("starter.ship", armed_starter()),
        ("micropulse-demo.ship", micropulse_starter()),
        ("ntr-patrol.ship", ntr_patrol()),
        ("expedition-patrol.ship", expedition_patrol()),
    ] {
        ship.compile(&catalogue)?;
        ship.save(output.join(filename))?;
    }
    Ok(())
}
