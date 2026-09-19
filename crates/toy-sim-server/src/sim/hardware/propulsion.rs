use super::*;
use toy_sim_model::PropulsionTelemetry;

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
    ] {
        resources.sort();
        resources.dedup();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ratings_respect_rotated_opposed_engines_and_lever_arms() {
        let design = crate::sim::vessel::ShipCatalogue(toy_sim_ships::Catalogue::builtin());
        let mut ship = toy_sim_ships::starter(toy_sim_ships::EXAMPLE_CONTROLLER.to_vec())
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
