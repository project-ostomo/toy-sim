use osg_ships::{Catalogue, Equipment, GRID, PartDef, weapons};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Category {
    #[default]
    All,
    Structure,
    Storage,
    Propulsion,
    Power,
    Thermal,
    Weapons,
    Utilities,
}

impl Category {
    pub const ALL: [Self; 8] = [
        Self::All,
        Self::Structure,
        Self::Storage,
        Self::Propulsion,
        Self::Power,
        Self::Thermal,
        Self::Weapons,
        Self::Utilities,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All parts",
            Self::Structure => "Structure",
            Self::Storage => "Storage",
            Self::Propulsion => "Propulsion",
            Self::Power => "Power",
            Self::Thermal => "Thermal & shields",
            Self::Weapons => "Weapons",
            Self::Utilities => "Utilities",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Unit {
    Mass,
    MassFlow,
    Force,
    Torque,
    Power,
    Energy,
    Speed,
    Area,
    Volume,
    Percent,
    Degrees,
    DegreesPerSecond,
    Milliradians,
    Hertz,
    Seconds,
    Metres,
    Hull,
}

#[derive(Clone, Debug)]
pub enum SpecValue {
    Quantity(f64, Unit),
    Dimensions([f64; 3]),
    Text(String),
}

impl std::fmt::Display for SpecValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Quantity(value, unit) => f.write_str(&unit.format(*value)),
            Self::Dimensions(size) => write!(
                f,
                "{} × {} × {} m",
                number(size[0]),
                number(size[1]),
                number(size[2])
            ),
            Self::Text(text) => f.write_str(text),
        }
    }
}

impl Unit {
    pub fn format(self, value: f64) -> String {
        match self {
            Self::Mass if value.abs() >= 1000. => format!("{} t", number(value / 1000.)),
            Self::Mass if value.abs() >= 1. => format!("{} kg", number(value)),
            Self::Mass => scaled(value * 1000., "g"),
            Self::MassFlow if value.abs() >= 1. => format!("{} kg/s", number(value)),
            Self::MassFlow => scaled(value * 1000., "g/s"),
            Self::Force => scaled(value, "N"),
            Self::Torque => scaled(value, "N·m"),
            Self::Power => scaled(value, "W"),
            Self::Energy => scaled(value, "J"),
            Self::Speed => scaled(value, "m/s"),
            Self::Metres => scaled(value, "m"),
            Self::Area => format!("{} m²", number(value)),
            Self::Volume => format!("{} m³", number(value)),
            Self::Percent => format!("{}%", number(value * 100.)),
            Self::Degrees => format!("{}°", number(value.to_degrees())),
            Self::DegreesPerSecond => format!("{}°/s", number(value.to_degrees())),
            Self::Milliradians => format!("{} mrad", number(value * 1000.)),
            Self::Hertz => format!("{} /s", number(value)),
            Self::Seconds => format!("{} s", number(value)),
            Self::Hull => number(value),
        }
    }
}

fn number(value: f64) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    let decimals = if value.abs() >= 100. {
        0
    } else if value.abs() >= 10. {
        1
    } else {
        2
    };
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

fn scaled(value: f64, suffix: &str) -> String {
    let magnitude = value.abs();
    let (scale, prefix) = if magnitude >= 1e12 {
        (1e12, "T")
    } else if magnitude >= 1e9 {
        (1e9, "G")
    } else if magnitude >= 1e6 {
        (1e6, "M")
    } else if magnitude >= 1e3 {
        (1e3, "k")
    } else if magnitude >= 1. || magnitude == 0. {
        (1., "")
    } else if magnitude >= 1e-3 {
        (1e-3, "m")
    } else {
        (1e-6, "µ")
    };
    format!("{} {prefix}{suffix}", number(value / scale))
}

#[derive(Clone, Debug)]
pub struct SpecRow {
    pub label: &'static str,
    pub value: SpecValue,
}

#[derive(Clone, Debug)]
pub struct SpecSection {
    pub title: &'static str,
    pub rows: Vec<SpecRow>,
    pub note: Option<&'static str>,
}

impl SpecSection {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            rows: Vec::new(),
            note: None,
        }
    }

    fn quantity(&mut self, label: &'static str, value: f64, unit: Unit) {
        self.rows.push(SpecRow {
            label,
            value: SpecValue::Quantity(value, unit),
        });
    }

    fn text(&mut self, label: &'static str, value: impl Into<String>) {
        self.rows.push(SpecRow {
            label,
            value: SpecValue::Text(value.into()),
        });
    }
}

#[derive(Clone, Debug)]
pub struct PartDescription {
    pub kind: &'static str,
    pub category: Category,
    pub summary: &'static str,
    pub sections: Vec<SpecSection>,
}

impl PartDescription {
    pub fn new(part: &PartDef, catalogue: &Catalogue) -> Self {
        use Unit::*;
        let mut physical = SpecSection::new("Physical");
        if part.tank_volume_m3 > 0. {
            physical.quantity("Tank space", part.tank_volume_m3, Unit::Volume);
        }
        physical.quantity("Dry mass", part.mass_kg, Mass);
        physical.rows.push(SpecRow {
            label: "Dimensions",
            value: SpecValue::Dimensions(part.dimensions.map(|v| v as f64 * GRID)),
        });
        physical.quantity("Hull contribution", part.hull, Hull);

        let mut performance = SpecSection::new("Performance");
        let mut requirements = SpecSection::new("Requirements");
        let (kind, category, summary) = match &part.equipment {
            Equipment::Structure => (
                "Structure",
                Category::Structure,
                "Structural reinforcement for the ship's hull.",
            ),
            Equipment::Storage { capacity_m3 } => {
                performance.quantity("Cargo capacity", *capacity_m3, Volume);
                (
                    "Cargo storage",
                    Category::Storage,
                    "Stores cargo, ammunition, fuel and propellant.",
                )
            }
            Equipment::Battery { capacity_j } => {
                performance.quantity("Stored energy", *capacity_j as f64, Energy);
                (
                    "Battery",
                    Category::Power,
                    "Stores electrical energy for ship systems.",
                )
            }
            Equipment::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
                propellant_energy_j_kg,
                ..
            } => {
                performance.quantity("Maximum thrust", *thrust_n, Force);
                performance.quantity("Exhaust velocity", thrust_n / propellant_kg_s, Speed);
                requirements.quantity("Electrical demand", *power_w, Power);
                requirements.quantity("Propellant", *propellant_kg_s, MassFlow);
                requirements.note =
                    Some("Ratings at full thrust. Thrust acts along the part's forward axis (−Z).");
                (
                    "Main engine",
                    Category::Propulsion,
                    if *propellant_energy_j_kg > 0. {
                        "Burns stored propellant to produce thrust; electrical power runs the controls."
                    } else {
                        "Converts electrical power and propellant into forward thrust."
                    },
                )
            }
            Equipment::ThermalEngine {
                thrust_n,
                specific_impulse_s,
                thermal_efficiency,
                fuel_energy_j_kg,
                ..
            } => {
                let exhaust_velocity = specific_impulse_s * osg_ships::STANDARD_GRAVITY_M_S2;
                let thermal_power = 0.5 * thrust_n * exhaust_velocity / thermal_efficiency;
                performance.quantity("Maximum thrust", *thrust_n, Force);
                performance.quantity("Specific impulse", *specific_impulse_s, Seconds);
                performance.quantity("Exhaust velocity", exhaust_velocity, Speed);
                requirements.quantity("Propellant", thrust_n / exhaust_velocity, MassFlow);
                requirements.quantity("Reactor fuel", thermal_power / fuel_energy_j_kg, MassFlow);
                requirements.quantity(
                    "Waste heat",
                    thermal_power * (1.0 - thermal_efficiency),
                    Power,
                );
                (
                    "Nuclear thermal engine",
                    Category::Propulsion,
                    "A reusable fission core heats stored propellant to produce thrust.",
                )
            }
            Equipment::MicropulseEngine {
                thrust_n,
                specific_impulse_s,
                charge_energy_j_kg,
                electric_efficiency,
                absorbed_heat_fraction,
                ..
            } => {
                let exhaust_velocity = specific_impulse_s * osg_ships::STANDARD_GRAVITY_M_S2;
                let charge_flow = thrust_n / exhaust_velocity;
                let reaction_power = charge_flow * charge_energy_j_kg;
                let electric_power = 0.01 * 0.5 * thrust_n * exhaust_velocity;

                performance.quantity("Maximum thrust", *thrust_n, Force);
                performance.quantity("Specific impulse", *specific_impulse_s, Seconds);
                performance.quantity("Exhaust velocity", exhaust_velocity, Speed);
                performance.quantity("Electrical output", electric_power, Power);
                requirements.quantity("Micropulse charges", charge_flow, MassFlow);
                requirements.quantity(
                    "Absorbed heat",
                    reaction_power * absorbed_heat_fraction,
                    Power,
                );
                requirements.quantity(
                    "Generator conversion heat",
                    electric_power * (1.0 / electric_efficiency - 1.0),
                    Power,
                );
                requirements.note = Some(
                    "Maximum-output ratings. Charges include reaction mass. Thrust and electrical output are independently controlled and share pulse fuel. Generation works at zero thrust; thrust acts along −Z.",
                );
                (
                    "Micropulse engine",
                    Category::Propulsion,
                    "Consumes complete micropulse charges to produce thrust and recover electrical power.",
                )
            }
            Equipment::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => {
                performance.quantity("Thrust per axis", *thrust_n, Force);
                requirements.quantity("Power per axis", *power_w, Power);
                requirements.quantity("Propellant per axis", *propellant_kg_s, MassFlow);
                requirements.note = Some(
                    "Full-output ratings for one axis. Simultaneous axes add their power and propellant consumption.",
                );
                (
                    "Reaction control",
                    Category::Propulsion,
                    "Provides translation and steering through opposing thrusters.",
                )
            }
            Equipment::Torquer { torque_nm, power_w } => {
                performance.quantity("Torque per axis", *torque_nm, Torque);
                requirements.quantity("Rated power", *power_w, Power);
                (
                    "Torque actuator",
                    Category::Propulsion,
                    "Turns the ship without consuming propellant.",
                )
            }
            Equipment::Reactor { spec } => {
                performance.quantity("Thermal output", spec.thermal_power_w, Power);
                performance.quantity(
                    "Electrical output at 300 K",
                    spec.thermal_power_w * spec.efficiency(300.0),
                    Power,
                );
                performance.quantity("Breeding ratio", spec.breeding_ratio, Percent);
                requirements.quantity(
                    "Fuel at full thermal output",
                    spec.thermal_power_w / spec.fuel_energy_j_kg,
                    MassFlow,
                );
                (
                    "Fission reactor",
                    Category::Power,
                    "Produces electricity with temperature-dependent conversion and retained decay heat.",
                )
            }
            Equipment::FuelProcessor { spec } => {
                performance.quantity("Processing rate", spec.throughput_kg_s, MassFlow);
                performance.quantity("Recovery", spec.recovery_fraction, Percent);
                if spec.produces_charges {
                    requirements.text("Input", "Reactor fuel");
                    performance.text("Output", "Micropulse charges");
                } else {
                    requirements.text("Input", "Bred fuel");
                    performance.text("Outputs", "Reactor fuel and spent processing residue");
                }
                requirements.quantity("Electrical demand", spec.power_w, Power);
                (
                    "Fuel processing",
                    Category::Power,
                    "Automatically processes stored nuclear material when power and output storage are available.",
                )
            }
            Equipment::Generator {
                power_w,
                fuel_kg_s,
                efficiency,
            } => {
                performance.quantity("Electrical output", *power_w, Power);
                performance.quantity("Efficiency", *efficiency, Percent);
                requirements.quantity("Fuel consumption", *fuel_kg_s, MassFlow);
                requirements.quantity("Waste heat", power_w * (1. / efficiency - 1.), Power);
                requirements.note =
                    Some("Fuel consumption and waste heat are rated at full electrical output.");
                (
                    "Generator",
                    Category::Power,
                    "Generates electrical power from onboard fuel.",
                )
            }
            Equipment::Shield {
                deployed_mass_kg,
                radiator_area_m2,
                feed_rate_kg_s,
                power_w,
            } => {
                performance.quantity("Deployed coolant", *deployed_mass_kg, Mass);
                performance.quantity("Radiator area", *radiator_area_m2, Area);
                performance.quantity("Replenishment rate", *feed_rate_kg_s, MassFlow);
                requirements.quantity("Electrical demand", *power_w, Power);
                requirements.note = Some("Replenishment draws from the ship's coolant reserve.");
                (
                    "Shield generator",
                    Category::Thermal,
                    "Deploys a protective shield that also radiates waste heat.",
                )
            }
            Equipment::CoolantTank { capacity_kg } => {
                performance.quantity("Coolant reserve", *capacity_kg, Mass);
                (
                    "Coolant tank",
                    Category::Thermal,
                    "Supplies replacement coolant as shield material ablates.",
                )
            }
            Equipment::HeatSink { capacity_j } => {
                performance.quantity("Additional heat storage", *capacity_j, Energy);
                (
                    "Heat sink",
                    Category::Thermal,
                    "Adds heat storage before accumulated heat damages the hull.",
                )
            }
            Equipment::Utility { utility } => {
                use osg_ships::utilities::UtilityDef;
                let (kind, summary) = match *utility {
                    UtilityDef::Factory {
                        capability,
                        power_per_lane_w,
                        lanes,
                    } => {
                        use osg_model::industry::IndustryCapability;
                        performance.text("Production lanes", lanes.to_string());
                        requirements.quantity("Power per lane", power_per_lane_w as f64, Power);
                        let kind = match capability {
                            IndustryCapability::Refinery => "Refinery",
                            IndustryCapability::FuelPlant => "Fuel plant",
                            IndustryCapability::Fabricator => "Fabricator",
                            IndustryCapability::Shipyard => "Industry module",
                        };
                        (
                            kind,
                            "Processes reserved cargo into manufactured goods using station electricity.",
                        )
                    }
                    UtilityDef::Shipyard {
                        power_per_lane_w,
                        lanes,
                        max_radius_m,
                    } => {
                        performance.text("Assembly lanes", lanes.to_string());
                        performance.quantity("Maximum ship radius", max_radius_m, Metres);
                        requirements.quantity("Power per lane", power_per_lane_w as f64, Power);
                        (
                            "Shipyard",
                            "Assembles empty ships from part kits and materials into station inventory.",
                        )
                    }
                    UtilityDef::SlipDrive { power_w } => {
                        requirements.quantity("Preparation power", power_w, Power);
                        (
                            "Slipdrive",
                            "Prepares slip travel outside natural exclusion regions.",
                        )
                    }
                    UtilityDef::Command { power_w } => {
                        requirements.quantity("Electrical input", power_w, Power);
                        ("Command module", "Provides powered backup avionics.")
                    }
                    UtilityDef::Sensor {
                        range_m,
                        power_w,
                        active,
                    } => {
                        performance.quantity("Sensor range", range_m, Metres);
                        requirements.quantity("Electrical input", power_w, Power);
                        (
                            if active {
                                "Active sensor"
                            } else {
                                "Passive sensor"
                            },
                            "Contributes observations to the ship's current sensor detections.",
                        )
                    }
                    UtilityDef::DirectoryTransmitter { power_w } => {
                        requirements.quantity("Electrical input", power_w, Power);
                        (
                            "Directory transmitter",
                            "Publishes this object and its inhabited system while its transponder is lit.",
                        )
                    }
                    UtilityDef::NavigationBeacon { power_w } => {
                        requirements.quantity("Electrical input", power_w, Power);
                        (
                            "Navigation beacon",
                            "Provides an authenticated reference for precise slip travel.",
                        )
                    }
                    UtilityDef::Docking {
                        radius_m,
                        mass_capacity_kg,
                    } => {
                        performance.quantity("Capture radius", radius_m, Metres);
                        performance.quantity("Docked mass capacity", mass_capacity_kg, Mass);
                        (
                            "Docking facility",
                            "Stores docked ships outside active flight physics.",
                        )
                    }
                    UtilityDef::Crew { capacity } => {
                        performance.text("Crew capacity", capacity.to_string());
                        (
                            "Crew compartment",
                            "Provides accommodation supported by life support equipment.",
                        )
                    }
                    UtilityDef::LifeSupport {
                        capacity,
                        power_w,
                        supplies_kg_per_person_s,
                    } => {
                        performance.text("Supported crew", capacity.to_string());
                        requirements.quantity("Electrical input", power_w, Power);
                        requirements.quantity(
                            "Supplies per person",
                            supplies_kg_per_person_s,
                            MassFlow,
                        );
                        (
                            "Life support",
                            "Consumes supplies and electricity to sustain the crew.",
                        )
                    }
                    UtilityDef::CargoHandler {
                        transfer_kg_s,
                        power_w,
                    } => {
                        performance.quantity("Transfer rate", transfer_kg_s, MassFlow);
                        requirements.quantity("Electrical input", power_w, Power);
                        (
                            "Cargo handler",
                            "Resupplies docked ships under the same owner's control.",
                        )
                    }
                    UtilityDef::PowerCoupler { transfer_w } => {
                        performance.quantity("Transfer limit", transfer_w, Power);
                        ("Power coupler", "Charges the batteries of docked ships.")
                    }
                };
                (kind, Category::Utilities, summary)
            }
            Equipment::Radiator {
                area_m2,
                emissivity,
            } => {
                performance.quantity("Emitting area", *area_m2, Area);
                performance.quantity("Emissivity", *emissivity, Percent);
                (
                    "Auxiliary radiator",
                    Category::Thermal,
                    "Radiates stored waste heat into space.",
                )
            }
            Equipment::EmergencyCooling {
                max_flow_kg_s,
                heat_removed_j_kg,
                activation_fraction,
            } => {
                performance.quantity("Maximum cooling", max_flow_kg_s * heat_removed_j_kg, Power);
                performance.quantity("Activation heat fraction", *activation_fraction, Percent);
                requirements.quantity("Maximum water flow", *max_flow_kg_s, MassFlow);
                (
                    "Emergency cooler",
                    Category::Thermal,
                    "Expels water to remove excess stored heat.",
                )
            }
            Equipment::Weapon { weapon } => {
                if let Some(spec) = weapon.spec(catalogue) {
                    if spec.beam_power_w > 0.0 {
                        performance.quantity("Optical power", spec.beam_power_w, Power);
                        performance.quantity("Maximum range", spec.beam_range_m, Metres);
                        performance.quantity("Efficiency", spec.efficiency, Percent);
                        requirements.quantity(
                            "Electrical power",
                            spec.beam_power_w / spec.efficiency,
                            Power,
                        );
                        requirements.quantity(
                            "Waste heat",
                            spec.beam_power_w * (1.0 / spec.efficiency - 1.0),
                            Power,
                        );
                    } else {
                        let ammunition =
                            &catalogue.resources[spec.ammunition_resource as usize - 1];
                        performance.quantity("Muzzle velocity", spec.muzzle_speed_m_s, Speed);
                        performance.quantity(
                            "Maximum firing rate",
                            1. / spec.cycle_interval_s,
                            Hertz,
                        );
                        performance.quantity("Projectile mass", spec.projectile_mass_kg, Mass);
                        performance.quantity(
                            "Muzzle energy",
                            0.5 * spec.projectile_mass_kg * spec.muzzle_speed_m_s.powi(2),
                            Energy,
                        );
                        performance.text(
                            "Launch mechanism",
                            if spec.chemical != 0 {
                                "Chemical cartridge"
                            } else {
                                "Electromagnetic"
                            },
                        );
                        performance.quantity(
                            "Projectile diameter",
                            spec.projectile_radius_m * 2.,
                            Metres,
                        );
                        performance.quantity(
                            "Dispersion half-angle",
                            spec.dispersion_half_angle_rad,
                            Milliradians,
                        );
                        performance.quantity("Efficiency", spec.efficiency, Percent);
                        if weapon.slew_rate_rad_s > 0. {
                            performance.quantity(
                                "Traverse rate",
                                weapon.slew_rate_rad_s,
                                DegreesPerSecond,
                            );
                            performance.quantity(
                                "Yaw travel",
                                spec.yaw_max_rad - spec.yaw_min_rad,
                                Degrees,
                            );
                            performance.text(
                                "Elevation",
                                format!(
                                    "{} to {}",
                                    Degrees.format(spec.pitch_min_rad),
                                    Degrees.format(spec.pitch_max_rad)
                                ),
                            );
                        }
                        requirements.text("Ammunition", ammunition.title.clone());
                        requirements.quantity(
                            "Electrical energy per shot",
                            weapons::shot_energy(&spec),
                            Energy,
                        );
                        requirements.quantity(
                            "Sustained power",
                            weapons::shot_energy(&spec) / spec.cycle_interval_s,
                            Power,
                        );
                        requirements.quantity(
                            "Propellant per shot",
                            weapons::shot_propellant_kg(&spec),
                            Mass,
                        );
                        requirements.note = Some(
                            "Sustained demand assumes uninterrupted firing at the maximum rate.",
                        );
                    }
                } else {
                    requirements.text(
                        "Ammunition",
                        format!("Unknown resource: {}", weapon.ammunition),
                    );
                }
                if weapon.laser.is_some() {
                    ("Laser", Category::Weapons, "")
                } else if weapon.slew_rate_rad_s > 0. {
                    (
                        "Weapon turret",
                        Category::Weapons,
                        "An articulated projectile weapon with independent aiming.",
                    )
                } else {
                    (
                        "Fixed weapon",
                        Category::Weapons,
                        "A projectile weapon aimed by turning the ship.",
                    )
                }
            }
        };

        let sections = [physical, performance, requirements]
            .into_iter()
            .filter(|section| !section.rows.is_empty())
            .collect();
        Self {
            kind,
            category,
            summary,
            sections,
        }
    }

    pub fn matches(&self, part: &PartDef, category: Category, search: &str) -> bool {
        (category == Category::All || self.category == category)
            && search.split_whitespace().all(|word| {
                let word = word.to_lowercase();
                part.title.to_lowercase().contains(&word)
                    || self.kind.to_lowercase().contains(&word)
                    || self.category.label().to_lowercase().contains(&word)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantity(description: &PartDescription, label: &str) -> f64 {
        let row = description
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .find(|row| row.label == label)
            .unwrap();
        let SpecValue::Quantity(value, _) = row.value else {
            panic!("expected a quantity")
        };
        value
    }

    #[test]
    fn specifications_follow_equipment_values_not_part_identity() {
        let catalogue = Catalogue::builtin();
        let mut part = catalogue.part("engine").unwrap().clone();
        part.id = "different-engine".into();
        part.equipment = Equipment::Engine {
            propellant_resource: "propellant".into(),
            thrust_n: 900.,
            propellant_kg_s: 3.,
            power_w: 12000.,
            propellant_energy_j_kg: 0.,
            plume: None,
        };
        let info = PartDescription::new(&part, &catalogue);
        assert_eq!(quantity(&info, "Maximum thrust"), 900.);
        assert_eq!(quantity(&info, "Exhaust velocity"), 300.);
        assert_eq!(quantity(&info, "Electrical demand"), 12000.);
    }

    #[test]
    fn weapon_and_generator_ratings_use_actual_resources_and_efficiency() {
        let mut catalogue = Catalogue::builtin();
        let mass = &mut catalogue
            .resources
            .iter_mut()
            .find(|r| r.id == "bearing")
            .unwrap()
            .mass_kg;
        *mass = 0.02;
        let weapon = PartDescription::new(catalogue.part("railgun_turret").unwrap(), &catalogue);
        assert!((quantity(&weapon, "Electrical energy per shot") - 250000. / 0.3).abs() < 1e-6);
        assert!((quantity(&weapon, "Sustained power") - 250000. / 0.3 / 0.05).abs() < 1e-6);
        assert_eq!(quantity(&weapon, "Propellant per shot"), 0.002);
        let generator = PartDescription::new(catalogue.part("generator").unwrap(), &catalogue);
        assert_eq!(quantity(&generator, "Waste heat"), 300000.);
        let rcs = PartDescription::new(catalogue.part("rcs").unwrap(), &catalogue);
        assert_eq!(quantity(&rcs, "Power per axis"), 5000000.);
        assert_eq!(quantity(&rcs, "Thrust per axis"), 2000.);
    }

    #[test]
    fn quantities_keep_small_consumption_rates_readable() {
        assert_eq!(Unit::MassFlow.format(0.0001), "100 mg/s");
        assert_eq!(Unit::Energy.format(300000000.), "300 MJ");
        assert_eq!(Unit::Percent.format(0.4), "40%");
        assert_eq!(Unit::Mass.format(1500.), "1.5 t");
        assert_eq!(Unit::Metres.format(0.01), "10 mm");
        assert_eq!(Unit::Power.format(0.), "0 W");
    }
}
