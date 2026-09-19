use super::{SpatialIndex, sphere_fully_blocks};
use crate::sim::{
    hardware::{Device, PartDevices, ShipThermal},
    vessel::ShipDesign,
};
use bevy::prelude::*;
use toy_sim_ships::{Equipment, thermal};
use toy_sim_spatial::SpatialHash;

pub const LUMENS_PER_OPTICAL_WATT: f64 = 220.0;
const GEOMETRIC_ALBEDO: f64 = 0.3;
const MIN_ILLUMINANCE_W_M2: f64 = 1e-7;
const THERMAL_EXHAUST_OPTICAL_FRACTION: f64 = 0.001;
const MICROPULSE_EXHAUST_OPTICAL_FRACTION: f64 = 0.01;

pub type SourceCache = ahash::AHashMap<[i128; 3], Vec<u32>>;

pub fn nearby_sources<'a>(
    position: crate::sim::precision::GalacticPosition,
    sources: &SpatialHash,
    cache: &'a mut SourceCache,
) -> &'a [u32] {
    const CELL_UM: i128 = 1_i128 << 67;
    let key = [
        position.x.div_euclid(CELL_UM),
        position.y.div_euclid(CELL_UM),
        position.z.div_euclid(CELL_UM),
    ];
    cache.entry(key).or_insert_with(|| {
        let anchor = crate::sim::precision::GalacticPosition::new(
            key[0] * CELL_UM + CELL_UM / 2,
            key[1] * CELL_UM + CELL_UM / 2,
            key[2] * CELL_UM + CELL_UM / 2,
        );
        let radius = 3.0_f64.sqrt() * CELL_UM as f64 / 2e6;
        sources
            .visible_from_sphere(
                anchor,
                radius,
                4.0 * std::f64::consts::PI * MIN_ILLUMINANCE_W_M2,
            )
            .ids
    })
}

pub fn reflection_sources(
    index: &SpatialIndex,
    object_id: usize,
    sources: &SpatialHash,
    candidates: &[u32],
) -> Vec<(usize, f64)> {
    let object = index.objects[object_id];
    let mut reflected = Vec::new();
    for &source_id in candidates {
        let luminosity_w = sources.get(source_id).unwrap().luminosity;
        let source_id = source_id as usize;
        if source_id == object_id {
            continue;
        }
        let source = index.objects[source_id];
        let offset = source.position.relative_to(object.position);
        let distance2 = offset.length_squared().max(source.radius_m.powi(2));
        if distance2 <= 0.0 {
            continue;
        }
        let blocked = index.any_optical_blocker_on_segment(object.position, offset, |id| {
            let blocker = index.objects[id];
            id != object_id
                && id != source_id
                && sphere_fully_blocks(
                    offset,
                    source.radius_m,
                    blocker.position.relative_to(object.position),
                    blocker.radius_m,
                )
        });
        if !blocked {
            let incident = luminosity_w / (4.0 * std::f64::consts::PI * distance2);
            reflected.push((
                source_id,
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

fn engine_luminosity(equipment: &Equipment, device: &toy_sim_ships::DeviceState) -> f64 {
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
    use super::super::SpatialObject;
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
        let catalogue = toy_sim_ships::Catalogue::builtin();
        let mut checked = 0;
        for part in &catalogue.parts {
            let equipment = &part.equipment;
            if matches!(
                equipment,
                Equipment::Engine { .. }
                    | Equipment::ThermalEngine { .. }
                    | Equipment::MicropulseEngine { .. }
            ) {
                let mut device = toy_sim_ships::DeviceState::default();
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
        let mut device = toy_sim_ships::DeviceState::default();
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
        let mut sources = SpatialHash::default();
        sources.insert(
            1,
            toy_sim_spatial::Entry {
                position: index.objects[1].position,
                radius_m: 100.0,
                luminosity: 1e18,
            },
        );
        let lit = reflection_sources(&index, 0, &sources, &[1])
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
        assert!(reflection_sources(&index, 0, &sources, &[1]).is_empty());
        index.set_luminosity(0, 1e12);
        assert!(
            index
                .visible(origin.offset_by(DVec3::Y * 1e6), 1e-6)
                .contains(&0)
        );
    }
}
