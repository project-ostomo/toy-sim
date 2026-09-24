use super::{SpatialIndex, sphere_fully_blocks};
use crate::sim::precision::GalacticPosition;
use crate::sim::{
    hardware::{Device, PartDevices, ShipThermal},
    vessel::ShipDesign,
};
use bevy::prelude::*;
use osg_ships::{Equipment, thermal};
use std::sync::{Arc, Mutex};

pub use osg_spatial::OPTICAL_LUMENS_PER_WATT as LUMENS_PER_OPTICAL_WATT;
const GEOMETRIC_ALBEDO: f64 = 0.3;
const MIN_ILLUMINANCE_W_M2: f64 = 1e-7;
const THERMAL_EXHAUST_OPTICAL_FRACTION: f64 = 0.001;
const MICROPULSE_EXHAUST_OPTICAL_FRACTION: f64 = 0.01;

#[derive(Clone, Copy)]
pub struct Light {
    pub position: GalacticPosition,
    pub radius: f64,
    pub power: f64,
}

#[derive(Default)]
struct Region {
    systems: Vec<usize>,
    irradiance_bound: f64,
}

#[derive(Clone, Default)]
pub struct Sky {
    pub universe: Option<Arc<osg_universe::universe::Universe>>,
    pub epoch: hifitime::Epoch,
    pub local: Vec<Light>,
    regions: Arc<Mutex<ahash::AHashMap<[i128; 3], Arc<Region>>>>,
}

impl Sky {
    pub fn set_universe(&mut self, universe: Option<Arc<osg_universe::universe::Universe>>) {
        if self.universe.as_ref().map(Arc::as_ptr) != universe.as_ref().map(Arc::as_ptr) {
            self.regions = Default::default();
        }
        self.universe = universe;
    }

    fn region(&self, position: GalacticPosition) -> Arc<Region> {
        const CELL_UM: i128 = 1_i128 << 67;
        let key = [
            position.x.div_euclid(CELL_UM),
            position.y.div_euclid(CELL_UM),
            position.z.div_euclid(CELL_UM),
        ];
        let mut regions = self.regions.lock().unwrap();
        if let Some(region) = regions.get(&key) {
            return region.clone();
        }
        let universe = self.universe.as_ref().unwrap();
        let anchor = crate::sim::precision::GalacticPosition::new(
            key[0] * CELL_UM + CELL_UM / 2,
            key[1] * CELL_UM + CELL_UM / 2,
            key[2] * CELL_UM + CELL_UM / 2,
        );
        let radius = 3.0_f64.sqrt() * CELL_UM as f64 / 2e6;
        let systems = universe.index().illumination_sources(
            anchor,
            radius,
            4.0 * std::f64::consts::PI * MIN_ILLUMINANCE_W_M2 * LUMENS_PER_OPTICAL_WATT,
        );
        // The discarded sources each contribute less than the selection threshold.
        let mut irradiance_bound = universe.systems().len() as f64 * MIN_ILLUMINANCE_W_M2;
        for &system in &systems {
            let summary = &universe.systems()[system];
            let minimum_distance =
                (summary.position.relative_to(anchor).length() - summary.influence_bound - radius)
                    .max(0.);
            let definition = universe
                .resolve_index(system)
                .expect("valid illumination system");
            for star in definition.solver.iter() {
                if let osg_universe::orrery_cfg::BodyClass::Star { lumens } = star.class_params {
                    irradiance_bound += lumens
                        / LUMENS_PER_OPTICAL_WATT
                        / (4.
                            * std::f64::consts::PI
                            * minimum_distance.max(star.radius).max(1e-9).powi(2));
                }
            }
        }
        if regions.len() >= 8192 {
            regions.clear();
        }
        let region = Arc::new(Region {
            systems,
            irradiance_bound,
        });
        regions.insert(key, region.clone());
        region
    }

    pub fn peak(&self, position: GalacticPosition, radius: f64) -> f64 {
        let incident = if self.universe.is_some() {
            self.region(position).irradiance_bound
        } else {
            self.local
                .iter()
                .map(|light| {
                    light.power
                        / (4.
                            * std::f64::consts::PI
                            * light
                                .position
                                .relative_to(position)
                                .length_squared()
                                .max(light.radius.powi(2))
                                .max(1e-18))
                })
                .sum()
        };
        4. * GEOMETRIC_ALBEDO * std::f64::consts::PI * radius.powi(2) * incident
    }

    fn sources(&self, position: GalacticPosition) -> Vec<Light> {
        let Some(universe) = &self.universe else {
            return self.local.clone();
        };
        let mut lights = Vec::new();
        for &system in &self.region(position).systems {
            let definition = universe
                .resolve_index(system)
                .expect("valid illumination system");
            for body in definition.solver.iter() {
                if let osg_universe::orrery_cfg::BodyClass::Star { lumens } = body.class_params {
                    let id = definition.body_id(&body.name).expect("star identity");
                    if let Some(position) = universe.solve_position(id, self.epoch) {
                        lights.push(Light {
                            position,
                            radius: body.radius,
                            power: lumens / LUMENS_PER_OPTICAL_WATT,
                        });
                    }
                }
            }
        }
        lights
    }

    pub fn uninstantiated_occlusion(
        &self,
        index: &SpatialIndex,
        origin: GalacticPosition,
        displacement: bevy::math::DVec3,
        target_radius: f64,
    ) -> bool {
        let Some(universe) = &self.universe else {
            return false;
        };
        let Ok(systems) = universe.capture_candidates(
            origin,
            displacement,
            target_radius,
            &mut osg_spatial::QueryBudget::new(1_000_000),
        ) else {
            return true;
        };
        for system in systems {
            if index.has_celestial_system(system) {
                continue;
            }
            let definition = universe
                .resolve_index(system)
                .expect("valid celestial geometry");
            for body in definition.solver.iter() {
                if body.radius <= 0.
                    || matches!(
                        body.class_params,
                        osg_universe::orrery_cfg::BodyClass::Barycenter
                    )
                {
                    continue;
                }
                if let Some(position) = definition.solver.solve_position(&body.name, self.epoch) {
                    if sphere_fully_blocks(
                        displacement,
                        target_radius,
                        position.relative_to(origin),
                        body.radius,
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

pub fn reflection_sources(
    index: &SpatialIndex,
    object_id: usize,
    sky: &Sky,
) -> Vec<(GalacticPosition, f64)> {
    let object = index.objects()[object_id];
    let mut reflected = Vec::new();
    for source in sky.sources(object.position) {
        let luminosity_w = source.power;
        if source.position == object.position {
            continue;
        }
        let offset = source.position.relative_to(object.position);
        let distance2 = offset.length_squared().max(source.radius.powi(2));
        if distance2 <= 0.0 {
            continue;
        }
        let blocked =
            index.any_optical_blocker_on_segment(object.position, offset, |id| {
                let blocker = index.objects()[id];
                id != object_id
                    && blocker.position != source.position
                    && sphere_fully_blocks(
                        offset,
                        source.radius,
                        blocker.position.relative_to(object.position),
                        blocker.radius_m,
                    )
            }) || sky.uninstantiated_occlusion(index, object.position, offset, source.radius);
        if !blocked {
            let incident = luminosity_w / (4.0 * std::f64::consts::PI * distance2);
            reflected.push((
                source.position,
                4.0 * GEOMETRIC_ALBEDO * std::f64::consts::PI * object.radius_m.powi(2) * incident,
            ));
        }
    }
    reflected
}

pub fn emitted_luminosity(
    design: Option<&ShipDesign>,
    thermal: Option<&ShipThermal>,
    parts: Option<&PartDevices>,
    devices: &Query<&Device>,
) -> f64 {
    let Some(design) = design else { return 0.0 };
    let mut luminosity = 0.0;
    if let Some(thermal) = thermal {
        let model = design.0.as_ref().into();
        let temperature = thermal.0.shield_temperature(model);
        luminosity += thermal::radiation(temperature, thermal.0.radiator_area(model))
            * visible_blackbody_fraction(temperature);
    }
    if let Some(parts) = parts {
        for (part, &entity) in design.0.parts.iter().zip(&parts.0) {
            let Ok(device) = devices.get(entity) else {
                continue;
            };
            luminosity += engine_luminosity(&part.definition.equipment, &device.0);
        }
    }
    luminosity
}

fn engine_luminosity(equipment: &Equipment, device: &osg_ships::DeviceState) -> f64 {
    if !device.operational || !device.powered {
        return 0.0;
    }
    let (force, exhaust, optical_fraction) = match *equipment {
        Equipment::Engine {
            thrust_n,
            propellant_kg_s,
            ..
        } => (
            device.actual,
            thrust_n / propellant_kg_s,
            THERMAL_EXHAUST_OPTICAL_FRACTION,
        ),
        Equipment::Rcs {
            thrust_n,
            propellant_kg_s,
            ..
        } => (
            bevy::math::DVec3::from_array(device.thrust_n)
                .abs()
                .element_sum(),
            thrust_n / propellant_kg_s,
            THERMAL_EXHAUST_OPTICAL_FRACTION,
        ),
        Equipment::ThermalEngine {
            specific_impulse_s, ..
        } => (
            device.actual,
            9.80665 * specific_impulse_s,
            THERMAL_EXHAUST_OPTICAL_FRACTION,
        ),
        Equipment::MicropulseEngine {
            specific_impulse_s, ..
        } => (
            device.actual,
            9.80665 * specific_impulse_s,
            MICROPULSE_EXHAUST_OPTICAL_FRACTION,
        ),
        _ => return 0.0,
    };
    0.5 * force.max(0.0) * exhaust * optical_fraction
}

fn visible_blackbody_fraction(temperature_k: f64) -> f64 {
    if temperature_k <= 0.0 {
        return 0.0;
    }
    let lower = 0.01438776877 / (780e-9 * temperature_k);
    let upper = 0.01438776877 / (380e-9 * temperature_k);
    let integrand = |x: f64| {
        if x > 700.0 {
            0.0
        } else {
            x.powi(3) / x.exp_m1()
        }
    };
    let step = (upper - lower) / 32.0;
    let mut integral = integrand(lower) + integrand(upper);
    for i in 1..32 {
        integral += integrand(lower + i as f64 * step) * if i % 2 == 0 { 2.0 } else { 4.0 };
    }
    (integral * step / 3.0 * 15.0 / std::f64::consts::PI.powi(4)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::super::{SpatialIndex, SpatialObject};
    use super::*;
    use crate::sim::precision::GalacticPosition;
    use bevy::math::DVec3;

    #[test]
    fn optical_thermal_emission_tracks_temperature() {
        assert!(visible_blackbody_fraction(300.0) < 1e-20);
        assert!(visible_blackbody_fraction(3500.0) > visible_blackbody_fraction(1000.0) * 1000.0);
        assert!((0.35..0.55).contains(&visible_blackbody_fraction(5778.0)));
    }

    #[test]
    fn engine_light_uses_delivered_scalar_thrust_and_all_firing_rcs_axes() {
        let catalogue = osg_ships::Catalogue::builtin();
        let mut checked = 0;
        for part in &catalogue.parts {
            let equipment = &part.equipment;
            if matches!(
                equipment,
                Equipment::Engine { .. }
                    | Equipment::ThermalEngine { .. }
                    | Equipment::MicropulseEngine { .. }
            ) {
                let mut device = osg_ships::DeviceState::default();
                assert_eq!(engine_luminosity(equipment, &device), 0.0);
                device.actual = 1000.0;
                let full = engine_luminosity(equipment, &device);
                assert!(full > 0.0);
                device.actual = 500.0;
                assert_eq!(engine_luminosity(equipment, &device), full * 0.5);
                device.powered = false;
                assert_eq!(engine_luminosity(equipment, &device), 0.0);
                checked += 1;
            }
        }
        assert!(checked >= 3);
        let rcs = Equipment::Rcs {
            propellant_resource: "water".into(),
            thrust_n: 1000.0,
            propellant_kg_s: 1.0,
            power_w: 1e6,
        };
        let mut device = osg_ships::DeviceState::default();
        device.thrust_n = [100.0, 0.0, 0.0];
        let one_axis = engine_luminosity(&rcs, &device);
        device.thrust_n = [100.0, -100.0, 100.0];
        assert_eq!(engine_luminosity(&rcs, &device), one_axis * 3.0);
    }

    #[test]
    fn planetary_shadow_removes_reflected_light_but_not_intrinsic_emission() {
        let mut world = World::new();
        let mut index = SpatialIndex::default();
        let origin = GalacticPosition::splat(1_i128 << 90);
        for (position, radius, occludes) in [(0.0, 10.0, false), (1e6, 100.0, true)] {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(DVec3::X * position),
                radius_m: radius,
                occludes,
                optical_occludes: occludes,
                optical_luminosity_w: 0.0,
            });
        }
        index.finish_geometry();
        let sources = Sky {
            local: vec![Light {
                position: index.objects()[1].position,
                radius: 100.0,
                power: 1e18,
            }],
            ..Default::default()
        };
        let lit = reflection_sources(&index, 0, &sources)
            .iter()
            .map(|(_, power)| power)
            .sum::<f64>();
        assert!(lit > 0.0);
        index.insert(SpatialObject {
            entity: world.spawn_empty().id(),
            position: origin.offset_by(DVec3::X * 5e5),
            radius_m: 1000.0,
            occludes: true,
            optical_occludes: true,
            optical_luminosity_w: 0.0,
        });
        index.finish_geometry();
        assert!(reflection_sources(&index, 0, &sources).is_empty());
        index.set_luminosity(0, 1e12);
        assert!(
            index
                .visible(origin.offset_by(DVec3::Y * 1e6), 1e-6)
                .contains(&0)
        );
    }
}
