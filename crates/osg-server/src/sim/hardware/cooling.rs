use super::*;

#[derive(Component)]
pub struct Radiator {
    pub area_m2: f64,
    pub emissivity: f64,
}

#[derive(Component)]
pub struct EmergencyCooling {
    pub max_flow_kg_s: f64,
    pub heat_removed_j_kg: f64,
    pub activation_fraction: f64,
}

pub fn install(part: &mut EntityCommands, equipment: &Equipment) {
    match *equipment {
        Equipment::Radiator {
            area_m2,
            emissivity,
        } => {
            part.insert(Radiator {
                area_m2,
                emissivity,
            });
        }
        Equipment::EmergencyCooling {
            max_flow_kg_s,
            heat_removed_j_kg,
            activation_fraction,
        } => {
            part.insert(EmergencyCooling {
                max_flow_kg_s,
                heat_removed_j_kg,
                activation_fraction,
            });
        }
        _ => {}
    }
}

fn remove_heat(state: &mut thermal::ThermalState, requested: f64, shield: bool) -> f64 {
    let reservoir = if shield {
        &mut state.shield_energy_j
    } else {
        &mut state.hull_energy_j
    };
    let removed = requested.min(*reservoir).max(0.0);
    *reservoir -= removed;
    removed
}

pub fn run(
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut ships: Query<
        (
            &ShipDesign,
            &PartDevices,
            &Hull,
            &mut ShipThermal,
            &mut ShipInventory,
            Option<&super::super::travel::Transit>,
        ),
        Without<super::super::travel::SystemsSuspended>,
    >,
    mut parts: Query<(&mut Device, Option<&Radiator>, Option<&EmergencyCooling>)>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("hardware.cooling.run");
    let dt = time.delta_secs_f64();
    let water = cat.0.resources.iter().position(|r| r.id == "water");
    for (design, installed, hull, mut thermal, mut inventory, transit) in &mut ships {
        for &entity in &installed.0 {
            let Ok((mut device, radiator, emergency)) = parts.get_mut(entity) else {
                continue;
            };
            if radiator.is_none() && emergency.is_none() {
                continue;
            }
            device.0.actual = 0.0;
            device.0.powered = device.0.operational && hull.0 > 0.0;
            if !device.0.powered {
                continue;
            }
            let state = &mut thermal.0;
            let shield = state.shield_operating() && state.shield_deployed_kg > 0.0;
            let temperature = if shield {
                state.shield_temperature(design.0.as_ref().into())
            } else {
                300.0 + 1200.0 * state.hull_energy_j / design.0.hull_heat_capacity_j.max(1.0)
            };
            if let Some(radiator) = radiator {
                let capacity = if shield {
                    state.shield_deployed_kg * thermal::SPECIFIC_HEAT
                } else {
                    design.0.hull_heat_capacity_j.max(1.0) / 1200.0
                };
                let cooled = thermal::exchanged(
                    temperature,
                    capacity,
                    radiator.area_m2 * radiator.emissivity / thermal::EMISSIVITY,
                    dt,
                    if transit.is_some() {
                        thermal::SLIPSPACE_K
                    } else {
                        thermal::BACKGROUND_K
                    },
                )
                .max(300.0);
                let exchanged = (temperature - cooled) * capacity;
                if exchanged >= 0.0 {
                    device.0.actual = remove_heat(state, exchanged, shield) / dt;
                } else {
                    if shield {
                        state.shield_energy_j -= exchanged;
                    } else {
                        state.hull_energy_j -= exchanged;
                    }
                    device.0.actual = exchanged / dt;
                }
            }
            if let (Some(emergency), Some(water)) = (emergency, water) {
                let fraction = state.hull_energy_j / design.0.hull_heat_capacity_j.max(1.0);
                if fraction < emergency.activation_fraction {
                    continue;
                }
                let resource = &cat.0.resources[water];
                let mass = (emergency.max_flow_kg_s * dt)
                    .min(inventory.0.available(water) * resource.mass_kg);
                let removed = remove_heat(state, mass * emergency.heat_removed_j_kg, false);
                inventory.0.consume(
                    water,
                    removed / emergency.heat_removed_j_kg / resource.mass_kg,
                );
                device.0.actual = removed / dt;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auxiliary_cooling_conserves_energy_and_cannot_overdraw_reservoirs() {
        let mut state = thermal::ThermalState {
            hull_energy_j: 30.0,
            shield_energy_j: 70.0,
            ..Default::default()
        };
        assert_eq!(remove_heat(&mut state, 50.0, false), 30.0);
        assert_eq!(state.hull_energy_j, 0.0);
        assert_eq!(state.shield_energy_j, 70.0);
        assert_eq!(remove_heat(&mut state, 90.0, true), 70.0);
        assert_eq!(state.shield_energy_j, 0.0);
        assert_eq!(remove_heat(&mut state, 90.0, true), 0.0);
    }

    #[test]
    fn emergency_cooling_spends_only_water_needed_and_respects_damage() {
        use bevy::ecs::system::RunSystemOnce;
        let mut fixture = super::super::fixtures::HardwareFixture::standard();
        let water = fixture
            .app
            .world()
            .resource::<ShipCatalogue>()
            .0
            .resources
            .iter()
            .position(|r| r.id == "water")
            .unwrap();
        let part = fixture
            .app
            .world()
            .get::<PartDevices>(fixture.ship)
            .unwrap()
            .0[0];
        fixture
            .app
            .world_mut()
            .entity_mut(part)
            .insert(EmergencyCooling {
                max_flow_kg_s: 10.0,
                heat_removed_j_kg: 3e6,
                activation_fraction: 0.8,
            });
        fixture
            .app
            .world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(osg_model::TICK_DURATION);
        let heat = fixture.design.hull_heat_capacity_j;
        fixture
            .app
            .world_mut()
            .get_mut::<ShipThermal>(fixture.ship)
            .unwrap()
            .0
            .hull_energy_j = heat;
        fixture.set_inventory(|inventory| inventory.quantities[water] = 1);
        fixture.app.world_mut().run_system_once(run).unwrap();
        let state = fixture.state();
        assert_eq!(state.inventory.quantities[water], 0);
        assert!((state.thermal.hull_energy_j - (heat - 3e6)).abs() < 1e-6);

        fixture.set_inventory(|inventory| inventory.quantities[water] = 1);
        fixture
            .app
            .world_mut()
            .get_mut::<Device>(part)
            .unwrap()
            .0
            .operational = false;
        fixture.app.world_mut().run_system_once(run).unwrap();
        assert_eq!(fixture.state().inventory.quantities[water], 1);
        assert_eq!(
            fixture.state().thermal.hull_energy_j,
            state.thermal.hull_energy_j
        );
    }

    #[test]
    fn radiator_rejects_stored_hull_heat_without_consuming_resources() {
        use bevy::ecs::system::RunSystemOnce;
        let mut fixture = super::super::fixtures::HardwareFixture::standard();
        let part = fixture
            .app
            .world()
            .get::<PartDevices>(fixture.ship)
            .unwrap()
            .0[0];
        fixture.app.world_mut().entity_mut(part).insert(Radiator {
            area_m2: 128.0,
            emissivity: 0.9,
        });
        fixture
            .app
            .world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(osg_model::TICK_DURATION);
        let heat = fixture.design.hull_heat_capacity_j * 0.5;
        fixture
            .app
            .world_mut()
            .get_mut::<ShipThermal>(fixture.ship)
            .unwrap()
            .0
            .hull_energy_j = heat;
        let before = fixture.state().inventory.quantities;
        fixture.app.world_mut().run_system_once(run).unwrap();
        let after = fixture.state();
        assert!(after.thermal.hull_energy_j < heat);
        assert!(after.thermal.hull_energy_j >= 0.0);
        assert_eq!(after.inventory.quantities, before);
        let rejected =
            fixture.app.world().get::<Device>(part).unwrap().0.actual * osg_model::TICK_SECONDS;
        assert!((heat - after.thermal.hull_energy_j - rejected).abs() < 1e-6);
    }
}
