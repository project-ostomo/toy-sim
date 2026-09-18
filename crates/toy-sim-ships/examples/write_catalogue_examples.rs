use anyhow::Result;
use toy_sim_ships::{Catalogue, Firmware, PlacedPart, ShipBlueprint, Tank};

fn add(ship: &mut ShipBlueprint, prototype: &str, position: [i32; 3]) -> usize {
    let index = ship.parts.len();
    ship.parts.push(PlacedPart {
        id: index as u64 + 1,
        name: String::new(),
        alias: String::new(),
        groups: vec![],
        tanks: vec![],
        prototype: prototype.into(),
        position,
        orientation: 0,
    });
    index
}

fn tank(ship: &mut ShipBlueprint, part: usize, resource: &str, volume_m3: f64, initial_fill: f64) {
    ship.parts[part].tanks.push(Tank {
        resource: resource.into(),
        volume_m3,
        initial_fill,
    });
}

fn common(name: &str) -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: name.into(),
        firmware: Firmware::Standard,
        ..Default::default()
    };
    add(&mut ship, "fuselage_4m", [0, 0, 0]);
    add(&mut ship, "fuselage_end_4m", [0, 0, -10]);
    add(&mut ship, "command_2m", [10, 10, -30]);
    add(&mut ship, "battery_2m", [40, 10, 0]);
    add(&mut ship, "shield_emitter_2m", [-20, 10, 0]);
    add(&mut ship, "shield_reservoir_2m", [-20, 10, 20]);
    add(&mut ship, "heat_sink_2m", [40, 10, 20]);
    add(&mut ship, "torquer", [40, 15, 50]);
    add(&mut ship, "sensor_passive_2m", [10, 10, -50]);
    tank(&mut ship, 0, "reactor_fuel", 0.2, 0.5);
    tank(&mut ship, 0, "spent_fuel", 0.2, 0.0);
    ship
}

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .unwrap_or_else(|| "assets/ships".into());
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output)?;
    let catalogue = Catalogue::builtin();

    let mut combat = common("Micropulse patrol ship");
    add(&mut combat, "micropulse_engine_4m", [0, 0, 80]);
    add(&mut combat, "reactor_compact_2m", [10, 10, -80]);
    add(&mut combat, "railgun_compact", [17, -5, 10]);
    tank(&mut combat, 0, "micropulse_charge", 50., 0.5);
    tank(&mut combat, 0, "railgun_dart", 0.01, 0.5);
    tank(&mut combat, 0, "propellant", 1., 0.5);

    let mut utility = common("Water NTR utility ship");
    add(&mut utility, "ntr_water_4m", [0, 0, 80]);
    add(&mut utility, "reactor_compact_2m", [10, 10, -80]);
    add(&mut utility, "docking_port_2m", [40, 10, 60]);
    add(&mut utility, "cargo_handler_4m", [0, 40, 20]);
    add(&mut utility, "emergency_cooler_2m", [40, 10, 70]);
    tank(&mut utility, 0, "water", 50., 0.8);

    let mut freighter = common("Breeder electric freighter");
    add(&mut freighter, "reactor_breeder_4m", [0, 0, 80]);
    add(&mut freighter, "electric_engine_1m", [15, 15, 160]);
    add(&mut freighter, "cargo_hold_4m", [0, 40, 80]);
    add(&mut freighter, "fuel_processor_4m", [0, -40, 80]);
    for z in [0, 40, 80, 120] {
        add(&mut freighter, "radiator_4m", [60, 18, z]);
    }
    tank(&mut freighter, 0, "propellant", 40., 0.5);
    tank(&mut freighter, 0, "fertile_feedstock", 1., 0.5);
    tank(&mut freighter, 0, "bred_fuel", 1., 0.0);

    for (filename, ship) in [
        ("micropulse-patrol.ship", combat),
        ("ntr-utility.ship", utility),
        ("breeder-freighter.ship", freighter),
    ] {
        ship.compile(&catalogue)?;
        ship.save(output.join(filename))?;
    }
    toy_sim_ships::armed_starter().save(output.join("starter.ship"))?;
    toy_sim_ships::micropulse_starter().save(output.join("micropulse-demo.ship"))?;
    Ok(())
}
