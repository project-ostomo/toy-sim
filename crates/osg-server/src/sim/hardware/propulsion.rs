use super::*;
use osg_model::PropulsionTelemetry;

#[derive(Component)]
pub struct InstalledRatings(pub PropulsionTelemetry);

#[derive(Component, Default)]
pub struct ActuatorOutput {
    pub force: DVec3,
    pub torque: DVec3,
}

pub fn telemetry(
    design: &CompiledShipDesign,
    output: Option<&ActuatorOutput>,
) -> PropulsionTelemetry {
    let mut result = PropulsionTelemetry::default();
    if let Some(output) = output {
        result.force_n = output.force.to_array();
        result.torque_nm = output.torque.to_array();
    }
    let control =
        bevy::math::DQuat::from_mat3(&orientation(design.blueprint.avionics.control_orientation));
    let mut add = |force: DVec3, torque: DVec3, bidirectional: bool| {
        let force = control.inverse() * force;
        let torque = control.inverse() * torque;
        result.rated_forward_n += if bidirectional {
            force.z.abs()
        } else {
            (-force.z).max(0.)
        };
        for axis in 0..3 {
            result.positive_torque_nm[axis] += if bidirectional {
                torque[axis].abs()
            } else {
                torque[axis].max(0.)
            };
            result.negative_torque_nm[axis] += if bidirectional {
                torque[axis].abs()
            } else {
                (-torque[axis]).max(0.)
            };
        }
    };
    for device in &design.device_catalogue {
        if !device.control_enabled {
            continue;
        }
        let rotation = bevy::math::DQuat::from_array(device.rotation);
        let offset = DVec3::from_array(device.position_m);
        match device.kind {
            DeviceKind::Engine { thrust_n, .. } => {
                let force = rotation * DVec3::NEG_Z * thrust_n;
                add(force, offset.cross(force), false);
            }
            DeviceKind::Rcs { thrust_n, .. } => {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    let force = rotation * axis * thrust_n;
                    add(force, offset.cross(force), true);
                }
            }
            DeviceKind::Torquer { torque_nm, .. } => {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    add(DVec3::ZERO, rotation * axis * torque_nm, true);
                }
            }
            _ => {}
        }
    }
    for part in &design.parts {
        match &part.definition.equipment {
            Equipment::Engine {
                propellant_resource,
                ..
            }
            | Equipment::ThermalEngine {
                propellant_resource,
                ..
            }
            | Equipment::Rcs {
                propellant_resource,
                ..
            } => {
                result.propellants.push(propellant_resource.clone());
            }
            Equipment::MicropulseEngine { .. } => result.charges.push("micropulse_charge".into()),
            Equipment::Reactor { .. } => result.fuels.push("reactor_fuel".into()),
            Equipment::Generator { .. } => result.fuels.push("fuel".into()),
            Equipment::Weapon { weapon } if weapon.laser.is_none() => {
                result.ammunition.push(weapon.ammunition.clone())
            }
            _ => {}
        }
        if matches!(part.definition.equipment, Equipment::ThermalEngine { .. }) {
            result.fuels.push("reactor_fuel".into());
        }
    }
    for resources in [
        &mut result.propellants,
        &mut result.fuels,
        &mut result.charges,
        &mut result.ammunition,
    ] {
        resources.sort();
        resources.dedup();
    }
    result
}

pub fn reserves(
    design: &CompiledShipDesign,
    mass: f64,
    inventory: &[osg_model::ResourceAmount],
) -> Vec<osg_model::DriveReserve> {
    let mut families: std::collections::BTreeMap<(&str, &str), (f64, f64)> = Default::default();
    for part in &design.parts {
        let (name, resource, thrust, flow) = match &part.definition.equipment {
            Equipment::Engine {
                propellant_resource,
                thrust_n,
                propellant_kg_s,
                ..
            } => (
                "Electric",
                propellant_resource.as_str(),
                *thrust_n,
                *propellant_kg_s,
            ),
            Equipment::ThermalEngine {
                propellant_resource,
                thrust_n,
                specific_impulse_s,
                ..
            } => (
                "Nuclear thermal",
                propellant_resource.as_str(),
                *thrust_n,
                thrust_n / (9.80665 * specific_impulse_s),
            ),
            Equipment::MicropulseEngine {
                thrust_n,
                specific_impulse_s,
                ..
            } => (
                "Micropulse",
                "micropulse_charge",
                *thrust_n,
                thrust_n / (9.80665 * specific_impulse_s),
            ),
            _ => continue,
        };
        let entry = families.entry((name, resource)).or_default();
        entry.0 += thrust;
        entry.1 += flow;
    }
    families
        .into_iter()
        .filter_map(|((name, resource), (thrust, flow))| {
            let reserve = inventory.iter().find(|r| r.resource == resource)?;
            let dry = (mass - reserve.amount_kg).max(1.);
            let exhaust = if flow > 0. { thrust / flow } else { 0. };
            Some(osg_model::DriveReserve {
                name: name.into(),
                resource: resource.into(),
                delta_v_m_s: exhaust * (mass.max(dry) / dry).ln(),
                full_delta_v_m_s: exhaust * ((dry + reserve.capacity_kg) / dry).ln(),
                flow_kg_s: flow,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ratings_respect_rotated_opposed_engines_and_lever_arms() {
        let design = crate::sim::vessel::ShipCatalogue(osg_ships::Catalogue::builtin());
        let mut ship = osg_ships::starter(osg_ships::EXAMPLE_CONTROLLER.to_vec())
            .compile(&design.0)
            .unwrap();
        let template = ship
            .device_catalogue
            .iter()
            .find(|d| matches!(d.kind, DeviceKind::Engine { .. }))
            .unwrap()
            .clone();
        ship.device_catalogue.clear();
        let mut engine = template.clone();
        engine.kind = DeviceKind::Engine {
            propellant_resource: "propellant".into(),
            thrust_n: 100.,
            propellant_kg_s: 1.,
            power_w: 0.,
        };
        engine.rotation = bevy::math::DQuat::IDENTITY.to_array();
        engine.position_m = [2., 0., 0.];
        ship.device_catalogue.push(engine.clone());
        engine.rotation = bevy::math::DQuat::from_rotation_y(std::f64::consts::PI).to_array();
        ship.device_catalogue.push(engine);
        let result = telemetry(&ship, None);
        assert!((result.rated_forward_n - 100.).abs() < 1e-8);
        assert!((result.positive_torque_nm[1] - 200.).abs() < 1e-8);
        assert!((result.negative_torque_nm[1] - 200.).abs() < 1e-8);
        let output = ActuatorOutput {
            force: DVec3::NEG_Z * 20.,
            torque: DVec3::Y * 40.,
        };
        let limited = telemetry(&ship, Some(&output));
        assert_eq!(limited.rated_forward_n, result.rated_forward_n);
        assert_eq!(limited.force_n, output.force.to_array());
        ship.device_catalogue.clear();
        assert_eq!(telemetry(&ship, None).rated_forward_n, 0.);
    }
}
