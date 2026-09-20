use anyhow::{Result, ensure};
use glam::{DMat3, DVec3};
use osg_ships::{Catalogue, ShipBlueprint, Tank};
use serde::Deserialize;

#[derive(Deserialize)]
struct Assembly {
    assembly: Vec<Instance>,
}

#[derive(Deserialize)]
struct Instance {
    prototype: String,
    parent: u64,
    socket: String,
    plug: String,
    roll: u8,
    position_m: [f64; 3],
}

fn main() -> Result<()> {
    let assembly: Assembly = toml::from_str(include_str!("../data/common-sky.toml"))?;
    let catalogue = Catalogue::builtin();
    let mut ship = ShipBlueprint {
        name: "USE Common Sky — 350-seat orbital shuttle".into(),
        ..Default::default()
    };

    for instance in &assembly.assembly {
        ship.attach(
            &instance.prototype,
            instance.parent,
            &instance.socket,
            &instance.plug,
            instance.roll,
        );
    }

    let frame = ship
        .parts
        .iter_mut()
        .find(|part| part.prototype == "cs_propulsion_frame")
        .unwrap();
    for (resource, volume_m3, initial_fill) in [
        ("micropulse_charge", 30.0, 1.0),
        ("cs_rocket_propellant", 180.0, 1.0),
        ("propellant", 10.0, 1.0),
        ("fuel", 5.0, 1.0),
        ("life_support_supplies", 5.0, 1.0),
    ] {
        frame.tanks.push(Tank {
            resource: resource.into(),
            volume_m3,
            initial_fill,
        });
    }

    let layout = ship.layout(&catalogue)?;
    let origin = DVec3::from_array(assembly.assembly[0].position_m);
    for (part, instance) in layout.iter().zip(&assembly.assembly) {
        let expected = DVec3::from_array(instance.position_m) - origin;
        ensure!(
            (part.centre - expected).length() < 0.001,
            "incorrect centre: {}",
            instance.prototype
        );
        ensure!(
            (part.rotation - DMat3::IDENTITY)
                .to_cols_array()
                .iter()
                .all(|v| v.abs() < 1e-6),
            "incorrect orientation: {}",
            instance.prototype
        );
    }

    let design = ship.compile(&catalogue)?;
    let output = std::env::args_os()
        .nth(1)
        .unwrap_or_else(|| "assets/ships/common-sky.ship".into());
    ship.save(&output)?;
    println!(
        "Saved {} parts, {:.1} t dry, {:.0} m³ cargo, {:.1} MN m torque to {}",
        design.parts.len(),
        design.dry_mass / 1000.0,
        design.capacity_m3,
        design.max_torque / 1e6,
        std::path::Path::new(&output).display()
    );
    Ok(())
}
