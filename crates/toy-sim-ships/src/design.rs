use crate::*;
use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
pub const SHIP_FORMAT_VERSION: u32 = 3;
pub const AVIONICS_MASS_KG: f64 = 71.;
pub const AVIONICS_POWER_W: f64 = 101.;
pub const SENSOR_POWER_W: f64 = 1000.;
pub const SENSOR_RANGE_M: f64 = 100_000_000.;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "program", rename_all = "snake_case")]
pub enum Firmware {
    #[default]
    Standard,
    Custom(#[serde(with = "serde_bytes")] Vec<u8>),
}
impl Firmware {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Standard => EXAMPLE_CONTROLLER,
            Self::Custom(bytes) => bytes,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Avionics {
    pub sensor_enabled: bool,
    /// Cube orientation: control axes to assembly axes. Zero points forward along -Z.
    pub control_orientation: u8,
    pub excluded_actuators: Vec<u64>,
}
impl Default for Avionics {
    fn default() -> Self {
        Self {
            sensor_enabled: true,
            control_orientation: 0,
            excluded_actuators: vec![],
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum DeviceSource {
    Part(usize),
    MicropulseGenerator(usize),
    Avionics,
}
pub const GRID: f64 = 0.1;
pub const MAX_FILE: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Tank {
    pub resource: String,
    pub volume_m3: f64,
    pub initial_fill: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlacedPart {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub tanks: Vec<Tank>,
    pub prototype: String,
    pub attachment: Option<crate::Attachment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShipBlueprint {
    #[serde(default)]
    pub format_version: u32,
    pub name: String,
    pub catalogue_revision: u32,
    pub parts: Vec<PlacedPart>,
    #[serde(default)]
    pub firmware: Firmware,
    #[serde(default)]
    pub avionics: Avionics,
}
impl Default for ShipBlueprint {
    fn default() -> Self {
        Self {
            name: "Untitled ship".into(),
            format_version: SHIP_FORMAT_VERSION,
            catalogue_revision: 3,
            parts: vec![],
            firmware: Firmware::Standard,
            avionics: Avionics::default(),
        }
    }
}
pub fn orientation(index: u8) -> DMat3 {
    let axes = [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ];
    let mut rotations = vec![];
    // Identity is orientation zero.
    for y in [
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Z,
        DVec3::NEG_Z,
    ] {
        for x in axes {
            if x.dot(y) == 0. {
                rotations.push(DMat3::from_cols(x, y, x.cross(y)));
            }
        }
    }
    rotations[index as usize % 24]
}
#[derive(Clone, Debug)]
pub struct PreparedPart {
    pub placed: PlacedPart,
    pub definition: PartDef,
    pub centre: DVec3,
    pub rotation: DMat3,
}
#[derive(Clone, Debug)]
pub struct CompiledShipDesign {
    pub blueprint: ShipBlueprint,
    pub parts: Vec<PreparedPart>,
    pub part_index: BTreeMap<u64, usize>,
    pub dry_mass: f64,
    pub centre: DVec3,
    pub inertia: DMat3,
    pub semi_axes: DVec3,
    pub radius: f64,
    pub hull: f64,
    pub hull_heat_capacity_j: f64,
    pub shield_deployed_kg: f64,
    pub shield_reserve_capacity_kg: f64,
    pub shield_feed_kg_s: f64,
    pub shield_radiator_area_m2: f64,
    pub exposed_area_m2: f64,
    pub capacity_m3: f64,
    pub battery_j: u64,
    pub avionics_handles: [crate::DeviceHandle; 3],
    /// Stable, precomputed priority order; passive parts never tick.
    pub active_parts: Vec<usize>,
    pub weapon_parts: Vec<usize>,
    pub part_weapons: Vec<Option<usize>>,
    pub weapon_specs: Vec<toy_sim_ship_api::abi::WeaponSpec>,
    pub max_torque: f64,
    pub device_catalogue: Vec<crate::DeviceDescriptor>,
    pub device_sources: Vec<DeviceSource>,
    pub part_devices: Vec<Option<usize>>,
    pub part_generators: Vec<Option<usize>>,
}
impl CompiledShipDesign {
    pub fn part_for_device(&self, handle: crate::DeviceHandle) -> Option<usize> {
        match self.device_sources.get(handle.0 as usize)? {
            DeviceSource::Part(i) | DeviceSource::MicropulseGenerator(i) => Some(*i),
            DeviceSource::Avionics => None,
        }
    }
}
impl ShipBlueprint {
    pub fn controller_bytes(&self) -> &[u8] {
        self.firmware.bytes()
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        ciborium::into_writer(self, &mut data)?;
        ensure!(data.len() <= MAX_FILE, "ship file too large");
        Ok(data)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_FILE,
            "invalid ship length"
        );
        let mut reader = std::io::Cursor::new(bytes);
        let s: Self = ciborium::from_reader(&mut reader)?;
        ensure!(
            s.format_version == SHIP_FORMAT_VERSION,
            "incompatible ship format {}; expected {} (rebuild the design with standard avionics)",
            s.format_version,
            SHIP_FORMAT_VERSION
        );
        ensure!(
            reader.position() as usize == bytes.len(),
            "trailing ship data"
        );
        ensure!(
            s.parts.len() <= 4096
                && s.controller_bytes().len() <= 1024 * 1024
                && s.name.len() <= 256,
            "ship design exceeds limits"
        );
        ensure!(
            s.parts.iter().all(|p| p.name.len() <= 256),
            "part name too long"
        );
        ensure!(
            s.parts.iter().all(PlacedPart::metadata_within_limits),
            "invalid device names/groups"
        );
        Ok(s)
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take((MAX_FILE + 1) as u64)
            .read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let tmp = path.with_extension("ship.tmp");
        let bytes = self.to_bytes()?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
    pub fn compile(&self, cat: &Catalogue) -> Result<CompiledShipDesign> {
        cat.validate()?;
        ensure!(
            self.format_version == SHIP_FORMAT_VERSION,
            "incompatible ship format"
        );
        ensure!(
            self.avionics.control_orientation < 24,
            "invalid control orientation"
        );
        ensure!(
            self.avionics.excluded_actuators.len() <= 4096,
            "too many actuator exclusions"
        );
        ensure!(self.name.len() <= 256, "ship name exceeds 256 bytes");
        ensure!(
            self.catalogue_revision == cat.revision,
            "catalogue revision mismatch"
        );
        ensure!(
            !self.parts.is_empty() && self.parts.len() <= 4096,
            "ship needs 1–4096 parts"
        );
        ensure!(
            self.controller_bytes().len() <= 1024 * 1024,
            "controller exceeds 1 MiB"
        );
        ensure!(
            !self.controller_bytes().is_empty(),
            "attach controller WASM to the computer"
        );
        let parts = self.layout(cat)?;
        let mut ids = BTreeMap::new();
        let mut boxes = vec![];
        let mut aliases = std::collections::BTreeSet::new();
        for (index, prepared) in parts.iter().enumerate() {
            let p = &prepared.placed;
            ensure!(p.valid_labels(), "invalid device names/groups");
            if !p.alias.is_empty() {
                ensure!(
                    !["computer", "accelerometer", "radar"].contains(&p.alias.as_str()),
                    "device alias {} is reserved for standard avionics",
                    p.alias
                );
                ensure!(
                    aliases.insert(&p.alias),
                    "duplicate device alias {}",
                    p.alias
                );
            }
            ensure!(
                ids.insert(p.id, index).is_none(),
                "duplicate part ID {}",
                p.id
            );
            let def = cat
                .part(&p.prototype)
                .with_context(|| format!("unknown part {}", p.prototype))?
                .clone();
            ensure!(
                def.equipment.device_kind().is_some()
                    || (p.alias.is_empty() && p.groups.is_empty()),
                "structural parts have no device"
            );
            let mut tank_volume = 0.;
            for tank in &p.tanks {
                ensure!(
                    cat.resources.iter().any(|r| r.id == tank.resource),
                    "unknown tank resource {}",
                    tank.resource
                );
                ensure!(
                    tank.volume_m3.is_finite() && tank.volume_m3 > 0.,
                    "invalid tank volume on part {}",
                    p.id
                );
                let resource = cat
                    .resources
                    .iter()
                    .find(|r| r.id == tank.resource)
                    .unwrap();
                crate::industry::tank_containment_mass_kg(
                    tank.volume_m3,
                    resource.storage.containment_kg_m3,
                )?;
                ensure!(
                    tank.initial_fill.is_finite() && (0.0..=1.0).contains(&tank.initial_fill),
                    "invalid tank fill on part {}",
                    p.id
                );
                tank_volume += tank.volume_m3;
            }
            ensure!(
                tank_volume <= def.tank_volume_m3,
                "tank volume exceeds capacity on part {}",
                p.id
            );
            boxes.push(prepared.bounds());
        }
        for a in 0..parts.len() {
            for b in a + 1..parts.len() {
                ensure!(
                    !crate::collision::parts_overlap(&parts[a], &parts[b]),
                    "parts {} and {} overlap",
                    parts[a].placed.id,
                    parts[b].placed.id
                );
            }
        }
        let voxel_work: f64 = parts
            .iter()
            .map(|p| {
                let (lo, hi) = p.bounds();
                (hi - lo + DVec3::splat(2.0)).ceil().element_product()
            })
            .sum();
        ensure!(
            voxel_work <= 16_000_000.0,
            "assembly exceeds collision voxel budget"
        );
        let part_mass = |p: &PreparedPart| {
            p.definition.mass_kg
                + p.placed
                    .tanks
                    .iter()
                    .map(|tank| {
                        let resource = cat
                            .resources
                            .iter()
                            .find(|r| r.id == tank.resource)
                            .unwrap();
                        crate::industry::tank_containment_mass_kg(
                            tank.volume_m3,
                            resource.storage.containment_kg_m3,
                        )
                        .expect("validated containment mass")
                    })
                    .sum::<f64>()
        };
        let dry_mass: f64 = parts.iter().map(&part_mass).sum();
        let centre = parts.iter().map(|p| p.centre * part_mass(p)).sum::<DVec3>() / dry_mass;
        let mut inertia = DMat3::ZERO;
        let mut lo = DVec3::splat(f64::INFINITY);
        let mut hi = -lo;
        let mut hull = 0.;
        let mut shield_deployed_kg = 0.;
        let mut shield_reserve_capacity_kg = 0.;
        let mut shield_feed_kg_s = 0.;
        let mut shield_radiator_area_m2 = 0.;
        let mut extra_heat_capacity_j = 0.;
        let mut capacity_m3 = 0.0f64;
        let mut battery_j = 0u64;

        for p in &parts {
            let m = part_mass(p);
            let d = DVec3::from_array(p.definition.dimensions.map(|v| v as f64 * GRID));
            let r = p.centre - centre;
            let local = DMat3::from_diagonal(
                DVec3::new(
                    d.y * d.y + d.z * d.z,
                    d.x * d.x + d.z * d.z,
                    d.x * d.x + d.y * d.y,
                ) * (m / 12.),
            );
            let outer = DMat3::from_cols(r * r.x, r * r.y, r * r.z);
            inertia += p.rotation * local * p.rotation.transpose()
                + m * (DMat3::IDENTITY * r.length_squared() - outer);
            let half = p.rotation.abs() * d / 2.;
            lo = lo.min(r - half);
            hi = hi.max(r + half);
            hull += p.definition.hull;
            match p.definition.equipment {
                Equipment::Storage { capacity_m3: c } => capacity_m3 = capacity_m3 + c,
                Equipment::Battery { capacity_j: c } => {
                    battery_j = battery_j
                        .checked_add(c)
                        .context("battery capacity overflow")?
                }
                Equipment::CoolantTank { capacity_kg } => shield_reserve_capacity_kg += capacity_kg,
                Equipment::HeatSink { capacity_j } => extra_heat_capacity_j += capacity_j,
                Equipment::Shield {
                    deployed_mass_kg,
                    radiator_area_m2,
                    feed_rate_kg_s,
                    ..
                } => {
                    shield_deployed_kg += deployed_mass_kg;
                    shield_radiator_area_m2 += radiator_area_m2;
                    shield_feed_kg_s += feed_rate_kg_s;
                }

                _ => {}
            }
        }
        // Distributed internal equipment follows the assembly's existing mass approximation.
        inertia *= (dry_mass + AVIONICS_MASS_KG) / dry_mass;
        let dry_mass = dry_mass + AVIONICS_MASS_KG;
        for id in &self.avionics.excluded_actuators {
            ensure!(
                parts.iter().any(|p| p.placed.id == *id
                    && matches!(
                        p.definition.equipment,
                        Equipment::Engine { .. }
                            | Equipment::MicropulseEngine { .. }
                            | Equipment::ThermalEngine { .. }
                            | Equipment::Torquer { .. }
                            | Equipment::Weapon { .. }
                            | Equipment::Rcs { .. }
                    )),
                "actuator exclusion refers to missing/non-actuator part {id}"
            );
        }
        let mut active_parts: Vec<_> = (0..parts.len())
            .filter(|&i| parts[i].definition.equipment.priority().is_some())
            .collect();
        active_parts
            .sort_by_key(|&i| (parts[i].definition.equipment.priority(), parts[i].placed.id));
        let max_torque = parts
            .iter()
            .filter_map(|p| {
                if let Equipment::Torquer { torque_nm, .. } = p.definition.equipment {
                    Some(torque_nm)
                } else {
                    None
                }
            })
            .sum();
        let mut device_catalogue = Vec::new();
        let mut device_sources = Vec::new();
        let mut part_devices = vec![None; parts.len()];
        let mut part_generators = vec![None; parts.len()];
        for (index, part) in parts.iter().enumerate() {
            if let Some(kind) = part.definition.equipment.device_kind() {
                let device = device_catalogue.len();
                part_devices[index] = Some(device);
                device_sources.push(DeviceSource::Part(index));
                device_catalogue.push(crate::DeviceDescriptor {
                    handle: crate::DeviceHandle(device as u16),
                    part_id: part.placed.id,
                    control_enabled: !self.avionics.excluded_actuators.contains(&part.placed.id),
                    alias: part.placed.alias.clone(),
                    groups: part.placed.groups.clone(),
                    kind,
                    position_m: (part.centre - centre).to_array(),
                    rotation: glam::DQuat::from_mat3(&part.rotation).to_array(),
                });
            }
            if let Equipment::MicropulseEngine {
                thrust_n,
                specific_impulse_s,
                ..
            } = part.definition.equipment
            {
                let device = device_catalogue.len();
                part_generators[index] = Some(device);
                device_sources.push(DeviceSource::MicropulseGenerator(index));
                device_catalogue.push(crate::DeviceDescriptor {
                    handle: crate::DeviceHandle(device as u16),
                    part_id: part.placed.id,
                    control_enabled: true,
                    alias: format!("{} generator", part.placed.alias),
                    groups: part.placed.groups.clone(),
                    kind: crate::DeviceKind::Generator {
                        power_w: 0.005
                            * thrust_n
                            * crate::STANDARD_GRAVITY_M_S2
                            * specific_impulse_s,
                    },
                    position_m: (part.centre - centre).to_array(),
                    rotation: glam::DQuat::from_mat3(&part.rotation).to_array(),
                });
            }
        }
        let fitted_sensor_range = parts
            .iter()
            .filter_map(|part| match part.definition.equipment {
                Equipment::Utility {
                    utility: crate::utilities::UtilityDef::Sensor { range_m, .. },
                } => Some(range_m),
                _ => None,
            })
            .reduce(f64::max);
        let avionics_handles = core::array::from_fn(|i| {
            use crate::*;
            let handle = DeviceHandle(device_catalogue.len() as u16);
            let (kind, alias) = match i {
                0 => (DeviceKind::Computer, "computer"),
                1 => (DeviceKind::Accelerometer, "accelerometer"),
                _ => (
                    DeviceKind::Sensor {
                        range_m: fitted_sensor_range.unwrap_or(SENSOR_RANGE_M),
                    },
                    "radar",
                ),
            };
            device_catalogue.push(DeviceDescriptor {
                handle,
                part_id: 0,
                control_enabled: true,
                alias: alias.into(),
                groups: vec![],
                kind,
                position_m: [0.; 3],
                rotation: if i == 0 {
                    glam::DQuat::from_mat3(&orientation(self.avionics.control_orientation))
                        .to_array()
                } else {
                    [0., 0., 0., 1.]
                },
            });
            device_sources.push(DeviceSource::Avionics);
            handle
        });
        ensure!(
            device_catalogue.len() <= crate::MAX_DEVICES,
            "ship exceeds logical device limit"
        );
        let exposed_area_m2 = exposed_area(&parts);
        let radius = parts
            .iter()
            .map(|part| {
                let half =
                    DVec3::from_array(part.definition.dimensions.map(|d| d as f64 * GRID)) * 0.5;
                let offset = part.centre - centre;
                // Catalogue orientations are axis permutations, so this is the
                // furthest actual part corner, rather than an empty assembly corner.
                (offset.abs() + part.rotation.abs() * half).length()
            })
            .fold(0.0_f64, f64::max);

        let mut weapon_parts = Vec::new();
        let mut part_weapons = vec![None; parts.len()];
        let mut weapon_specs = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            if let Equipment::Weapon { weapon } = &part.definition.equipment {
                part_weapons[i] = Some(weapon_parts.len());
                weapon_parts.push(i);
                weapon_specs.push(weapon.spec(cat).expect("validated ammunition"));
            }
        }

        Ok(CompiledShipDesign {
            weapon_parts,
            part_weapons,
            weapon_specs,
            blueprint: self.clone(),
            parts,
            part_index: ids,
            dry_mass,
            centre,
            inertia,
            semi_axes: lo.abs().max(hi.abs()) * 3f64.sqrt(),
            radius,
            hull,
            hull_heat_capacity_j: dry_mass * crate::thermal::HEAT_STORAGE_J_KG
                + extra_heat_capacity_j,
            shield_deployed_kg,
            shield_reserve_capacity_kg,
            shield_feed_kg_s,
            shield_radiator_area_m2,
            exposed_area_m2,
            capacity_m3,
            battery_j,
            avionics_handles,
            active_parts,
            max_torque,
            device_catalogue,
            device_sources,
            part_devices,
            part_generators,
        })
    }
}
/// A small connected test craft. Its WASM is supplied by the caller, never synthesized by hardware code.
pub fn starter(controller: Vec<u8>) -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: "Starter".into(),
        firmware: if controller == EXAMPLE_CONTROLLER {
            Firmware::Standard
        } else {
            Firmware::Custom(controller)
        },
        ..Default::default()
    };
    let mut parent = 0;
    for prototype in [
        "structure",
        "storage",
        "battery",
        "generator",
        "torquer",
        "shield",
        "engine",
    ] {
        parent = ship.attach(prototype, parent, "back", "front", 0);
        ship.parts.last_mut().unwrap().alias = match prototype {
            "structure" => "",
            "engine" => "main_engine",
            "torquer" => "attitude_control",
            other => other,
        }
        .into();
    }
    ship.attach("coolant_tank", 1, "front", "back", 0);
    let coolant = ship.parts.last_mut().unwrap();
    coolant.id = 14;
    ship.attach("command_2m", 14, "front", "back", 0);
    ship.parts[0].tanks = vec![
        Tank {
            resource: "propellant".into(),
            volume_m3: 0.76,
            initial_fill: 1.0,
        },
        Tank {
            resource: "fuel".into(),
            volume_m3: 0.01,
            initial_fill: 1.0,
        },
        Tank {
            resource: "bearing".into(),
            volume_m3: 0.02,
            initial_fill: 1.0,
        },
    ];
    ship
}

impl PlacedPart {
    pub fn metadata_within_limits(&self) -> bool {
        self.tanks.len() <= 32
            && self.tanks.iter().all(|t| t.resource.len() <= 128)
            && self.name.len() <= 256
            && self.alias.len() <= 64
            && self.groups.len() <= 16
            && self.groups.iter().all(|g| g.len() <= 64)
    }

    pub fn valid_labels(&self) -> bool {
        let valid_alias = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        };
        self.metadata_within_limits()
            && (self.alias.is_empty() || valid_alias(&self.alias))
            && self.groups.len() <= 16
            && self.groups.iter().all(|g| valid_alias(g))
            && self
                .groups
                .iter()
                .enumerate()
                .all(|(i, g)| !self.groups[..i].contains(g))
    }
}

/// Box surfaces minus shared face areas. Cavities/view factors are not modeled.
fn exposed_area(parts: &[PreparedPart]) -> f64 {
    let bounds: Vec<_> = parts
        .iter()
        .map(|p| {
            let half = p.rotation.abs()
                * DVec3::from_array(p.definition.dimensions.map(|v| v as f64 * GRID))
                * 0.5;
            (p.centre - half, p.centre + half)
        })
        .collect();
    let mut area = 0.0;
    for (i, &(lo, hi)) in bounds.iter().enumerate() {
        let d = hi - lo;
        area += 2.0 * (d.x * d.y + d.x * d.z + d.y * d.z);
        for &(other_lo, other_hi) in &bounds[..i] {
            for axis in 0..3 {
                if (hi[axis] - other_lo[axis]).abs() < 1e-8
                    || (other_hi[axis] - lo[axis]).abs() < 1e-8
                {
                    let a = (axis + 1) % 3;
                    let b = (axis + 2) % 3;
                    area -= 2.0
                        * (hi[a].min(other_hi[a]) - lo[a].max(other_lo[a])).max(0.0)
                        * (hi[b].min(other_hi[b]) - lo[b].max(other_lo[b])).max(0.0);
                }
            }
        }
    }
    area
}

/// Armed demonstration craft. The basic starter remains useful for flight and collision tests.
pub fn armed_starter() -> ShipBlueprint {
    let mut ship = starter(EXAMPLE_CONTROLLER.to_vec());
    ship.name = "Armed explorer".into();
    for (id, prototype, parent, socket, plug) in [
        (8, "railgun_turret", 15, "right", "left"),
        (9, "coilgun_turret", 15, "top", "bottom"),
        (10, "rcs", 1, "left", "right"),
        (11, "rcs", 1, "bottom", "top"),
        (12, "rcs", 6, "top", "bottom"),
        (13, "rcs", 6, "right", "left"),
    ] {
        ship.attach(prototype, parent, socket, plug, 0);
        let part = ship.parts.last_mut().unwrap();
        part.id = id;
        part.alias = format!("{prototype}_{id}");
        part.groups = vec![if prototype == "rcs" { "rcs" } else { "weapons" }.into()];
    }
    ship
}

pub fn micropulse_starter() -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: "Micropulse demonstrator".into(),
        ..Default::default()
    };
    ship.attach("fuselage_8m", 0, "", "", 0);
    ship.attach("micropulse_engine_8m", 1, "aft", "fore", 0);
    ship.parts[1].alias = "main_engine".into();
    ship.attach("fuselage_end_8m", 1, "fore", "aft", 0);
    let mut parent = 3;
    for prototype in ["battery", "shield", "coolant_tank", "torquer", "command_2m"] {
        parent = ship.attach(prototype, parent, "front", "back", 0);
    }
    ship.parts[0].tanks.push(Tank {
        resource: "micropulse_charge".into(),
        volume_m3: 500.0,
        initial_fill: 0.2,
    });
    ship
}

pub fn ntr_patrol() -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: "Kestrel NTR patrol".into(),
        ..Default::default()
    };
    ship.attach("fuselage_2m", 0, "", "", 0);
    ship.attach("fuselage_end_2m", 1, "fore", "aft", 0);
    ship.attach("ntr_water_2m", 1, "aft", "fore", 0);
    ship.attach("command_2m", 2, "front", "back", 0);
    ship.attach("reactor_compact_2m", 4, "front", "back", 0);
    ship.attach("battery", 1, "right", "left", 0);
    ship.attach("shield", 1, "left", "right", 0);
    ship.attach("coolant_tank", 7, "left", "right", 0);
    ship.attach("torquer_agile", 1, "top", "bottom", 0);
    ship.attach("autocannon_compact", 1, "bottom", "top", 0);
    ship.attach("storage", 6, "right", "left", 0);
    for (resource, volume_m3, initial_fill) in [
        ("water", 7.5625, 1.0),
        ("reactor_fuel", 0.05, 0.5),
        ("spent_fuel", 0.05, 0.0),
        ("autocannon_round", 0.15, 1.0),
    ] {
        ship.parts[0].tanks.push(Tank {
            resource: resource.into(),
            volume_m3,
            initial_fill,
        });
    }
    ship.parts[1].tanks.push(Tank {
        resource: "water".into(),
        volume_m3: 0.625,
        initial_fill: 1.0,
    });
    ship
}

pub fn expedition_patrol() -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: "Peregrine expedition patrol".into(),
        ..Default::default()
    };
    ship.attach("fuselage_4m", 0, "", "", 0);
    ship.attach("fuselage_end_4m", 1, "fore", "aft", 0);
    ship.attach("micropulse_engine_4m", 1, "aft", "fore", 0);
    ship.attach("command_2m", 2, "front", "back", 0);
    ship.attach("laser_pulse_2m", 1, "left", "right", 0);
    ship.attach("laser_pulse_2m", 1, "right", "left", 0);
    ship.attach("shield_emitter_2m", 1, "top", "bottom", 0);
    ship.attach("coolant_tank", 7, "top", "bottom", 0);
    ship.attach("battery_2m", 4, "left", "right", 0);
    ship.attach("storage", 4, "right", "left", 0);
    ship.attach("torquer_agile", 4, "top", "bottom", 0);
    ship.attach("torquer_agile", 4, "bottom", "top", 0);
    ship.attach("slipdrive_2m", 1, "bottom", "top", 0);
    ship.parts[0].tanks.push(Tank {
        resource: "micropulse_charge".into(),
        volume_m3: 60.,
        initial_fill: 0.5,
    });
    ship
}
