use super::*;
use crate::sim::{hardware, physics::MassProps, vessel};
use bevy::math::{DQuat, DVec3};
use osg_model::travel::{FuelBudget, FuelRequirement, Presence};
use osg_ships::{DeviceKind, Equipment};
use std::collections::BTreeMap;

pub(super) fn performance(world: &World, ship: Entity) -> Result<routing::ShipPerformance> {
    let design = &world
        .get::<vessel::ShipDesign>(ship)
        .context("ship design unavailable")?
        .0;
    let inventory = &world
        .get::<hardware::ShipInventory>(ship)
        .context("ship resources unavailable")?
        .0;
    let parts = world
        .get::<hardware::PartDevices>(ship)
        .context("ship hardware unavailable")?;
    ensure!(
        world.get::<hardware::PendingHardwareReset>(ship).is_none()
            && parts.0.len() == design.parts.len(),
        "ship hardware is still initializing"
    );
    let mass = world
        .get::<MassProps>(ship)
        .context("ship mass unavailable")?;
    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    let forward =
        osg_ships::orientation(design.blueprint.avionics.control_orientation) * DVec3::NEG_Z;
    let mut thrust = 0.0;
    let mut positive_torque = DVec3::ZERO;
    let mut negative_torque = DVec3::ZERO;
    let mut rates = BTreeMap::<String, f64>::new();

    for descriptor in &design.device_catalogue {
        if !descriptor.control_enabled {
            continue;
        }
        let Some(index) = design.part_for_device(descriptor.handle) else {
            continue;
        };
        let operational = parts
            .0
            .get(index)
            .and_then(|entity| world.get::<hardware::Device>(*entity))
            .is_some_and(|device| device.0.operational);
        if !operational {
            continue;
        }
        let rotation = DQuat::from_array(descriptor.rotation);
        let offset = DVec3::from_array(descriptor.position_m);
        let mut add =
            |force: DVec3, torque: DVec3, bidirectional: bool, fuel: Option<(&str, f64)>| {
                let projection = force.dot(forward);
                let usable = if bidirectional {
                    projection.abs()
                } else {
                    projection.max(0.0)
                };
                thrust += usable;
                positive_torque += if bidirectional {
                    torque.abs()
                } else {
                    torque.max(DVec3::ZERO)
                };
                negative_torque += if bidirectional {
                    torque.abs()
                } else {
                    (-torque).max(DVec3::ZERO)
                };
                if usable > 1e-8
                    && let Some((resource, rate)) = fuel
                {
                    *rates.entry(resource.to_owned()).or_default() += rate;
                }
            };
        match &descriptor.kind {
            DeviceKind::Engine {
                propellant_resource,
                thrust_n,
                propellant_kg_s,
                ..
            } => {
                let force = rotation * DVec3::NEG_Z * *thrust_n;
                add(
                    force,
                    offset.cross(force),
                    false,
                    Some((propellant_resource, *propellant_kg_s)),
                );
            }
            DeviceKind::Rcs {
                thrust_n,
                propellant_kg_s,
                ..
            } => {
                let Equipment::Rcs {
                    propellant_resource,
                    ..
                } = &design.parts[index].definition.equipment
                else {
                    continue;
                };
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    let force = rotation * axis * *thrust_n;
                    add(
                        force,
                        offset.cross(force),
                        true,
                        Some((propellant_resource, *propellant_kg_s)),
                    );
                }
            }
            DeviceKind::Torquer { torque_nm, .. } => {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    add(DVec3::ZERO, rotation * axis * *torque_nm, true, None);
                }
            }
            _ => {}
        }
    }

    let moment = mass
        .inertia
        .x_axis
        .abs()
        .element_sum()
        .max(mass.inertia.y_axis.abs().element_sum())
        .max(mass.inertia.z_axis.abs().element_sum());
    let torque = positive_torque.min(negative_torque).min_element();
    let alpha = (0.7 * torque / moment.max(1e-9)).max(1e-6);
    let max_rate = 2.5;
    let angle = std::f64::consts::PI;
    let turn_s = if angle < max_rate * max_rate / alpha {
        2.0 * (angle / alpha).sqrt() + 1.0
    } else {
        angle / max_rate + max_rate / alpha + 1.0
    };
    let fuels = rates
        .into_iter()
        .map(|(resource, kg_s)| {
            let index = catalogue
                .resources
                .iter()
                .position(|entry| entry.id == resource)
                .expect("validated propellant resource");
            routing::FuelRate {
                resource,
                kg_s,
                available_kg: inventory.quantities[index] as f64
                    * catalogue.resources[index].mass_kg,
            }
        })
        .collect::<Vec<_>>();

    Ok(routing::ShipPerformance {
        radius_m: travel::collision_radius(world, ship)?,
        mass_kg: mass.mass,
        acceleration_m_s2: thrust / mass.mass.max(1.0),
        propellant_kg_s: fuels.iter().map(|fuel| fuel.kg_s).sum(),
        turn_s,
        slip_power_w: world
            .get::<travel::SlipDrive>(ship)
            .map_or(0.0, |drive| drive.power_w),
        exotic_available_kg: catalogue
            .resources
            .iter()
            .enumerate()
            .find(|(_, resource)| resource.id == osg_model::travel::slip::EXOTIC_RESOURCE)
            .map_or(0.0, |(index, resource)| {
                inventory.quantities[index] as f64 * resource.mass_kg
            }),
        fuels,
    })
}

pub fn fuel_budget(world: &World, ship: Entity, total_kg: Option<f64>) -> FuelBudget {
    let Ok(performance) = performance(world, ship) else {
        return FuelBudget::default();
    };
    let total = total_kg.filter(|value| value.is_finite() && *value >= 0.0);
    FuelBudget {
        complete: total.is_some(),
        resources: performance
            .fuels
            .into_iter()
            .map(|fuel| FuelRequirement {
                resource: fuel.resource,
                required_kg: total.unwrap_or(0.0) * fuel.kg_s
                    / performance.propellant_kg_s.max(1e-30),
                available_kg: fuel.available_kg,
            })
            .collect(),
    }
}

pub(super) fn request(
    world: &mut World,
    ship: Entity,
    request: &Request,
) -> Result<routing::RouteRequest> {
    let performance = performance(world, ship)?;
    let mut origin =
        super::super::session::ship_pose(world, ship).context("ship pose unavailable")?;
    let presence = world
        .get::<travel::PresenceState>(ship)
        .context("ship presence unavailable")?
        .0
        .clone();
    ensure!(
        matches!(presence, Presence::Space | Presence::Docked { .. }),
        "ship cannot plan during transit or destruction"
    );
    if let Presence::Docked { host, .. } = presence {
        let host_entity = identity::lookup(world, host)?;
        origin = super::super::session::ship_pose(world, host_entity)
            .context("docking host pose unavailable")?;
    }
    Ok(routing::RouteRequest {
        origin,
        performance,
        preferences: request.preferences,
        tick: world.resource::<SimulationCounters>().ticks,
        directives: request.directives.clone(),
    })
}
