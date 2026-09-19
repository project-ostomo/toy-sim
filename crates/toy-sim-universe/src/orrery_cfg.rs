use serde::{Deserialize, Deserializer, Serialize};
use smol_str::SmolStr;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrreryCfg {
    pub name: SmolStr,
    #[serde(default)]
    pub position_um: crate::precision::GalacticPosition,
    #[serde(default)]
    pub bodies: Vec<Body>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Body {
    pub name: SmolStr,
    #[serde(flatten)]
    pub class_params: BodyClass,
    #[serde(default)]
    pub parent: Option<SmolStr>,
    #[serde(flatten)]
    pub orbit: Orbit,
    #[serde(flatten)]
    pub rotation: Rotation,

    #[serde(deserialize_with = "de_mass", default)]
    pub mass: f64,
    #[serde(deserialize_with = "de_distance", default)]
    pub radius: f64,
    #[serde(default)]
    pub atmosphere: Option<super::atmosphere::AtmosphereCfg>,
    #[serde(default = "default_surface_color")]
    pub surface_color: [f32; 3],
    /// Optional broad stellar class; subtype/temperature inference belongs to import.
    #[serde(default)]
    pub spectral_class: Option<SpectralClass>,
    #[serde(default)]
    pub stellar: Option<StellarParameters>,
    #[serde(default)]
    pub planet: Option<PlanetParameters>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StellarKind {
    MainSequence,
    BrownDwarf,
    WhiteDwarf,
    Giant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StellarParameters {
    pub kind: StellarKind,
    pub effective_temperature_k: f64,
    pub age_years: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanetKind {
    Rocky,
    Ice,
    Ocean,
    IceGiant,
    GasGiant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanetParameters {
    pub seed: [u8; 32],
    pub kind: PlanetKind,
    pub equilibrium_temperature_k: f64,
    pub temperature_k: f64,
    pub bond_albedo: f64,
    pub ocean_fraction: f64,
}

/// Broad spectral classes. Colours are a display approximation, not spectra.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum SpectralClass {
    O,
    B,
    A,
    F,
    G,
    K,
    M,
}
impl SpectralClass {
    pub fn linear_rgb(self) -> [f32; 3] {
        let rgb = match self {
            Self::O => [0.60, 0.72, 1.0],
            Self::B => [0.70, 0.80, 1.0],
            Self::A => [0.86, 0.90, 1.0],
            Self::F => [1.0, 0.97, 0.92],
            Self::G => [1.0, 0.90, 0.76],
            Self::K => [1.0, 0.75, 0.51],
            Self::M => [1.0, 0.56, 0.30],
        };
        // Palette is specified in sRGB; all radiance calculations use linear RGB.
        rgb.map(|v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        })
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "class")]
pub enum BodyClass {
    Star {
        lumens: f64,
    },
    #[default]
    Planet,
    Barycenter,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default)]
pub struct Orbit {
    #[serde(deserialize_with = "de_distance", default)]
    pub semi_major: f64,
    #[serde(deserialize_with = "de_time", default)]
    pub period: f64,
    #[serde(default)]
    pub eccentricity: f64,
    #[serde(default)]
    pub inclination: f64,
    #[serde(default)]
    pub ascending_node: f64,
    #[serde(default)]
    pub arg_of_pericenter: f64,
    #[serde(default)]
    pub mean_anomaly: f64,
    #[serde(default)]
    pub epoch: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default)]
pub struct Rotation {
    #[serde(deserialize_with = "de_time", default)]
    pub rotation_period: f64,
    #[serde(default)]
    pub obliquity: f64,
    #[serde(default)]
    pub eq_ascend_node: f64,
    #[serde(default)]
    pub rotation_epoch: f64,
}

fn de_mass<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    struct MassVisitor;

    impl<'de> serde::de::Visitor<'de> for MassVisitor {
        type Value = f64; // kilograms

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a number or a string like \"200 massEarth\"")
        }

        fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v)
        }
        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v as f64)
        }

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let mut parts = s.split_whitespace();
            let value: f64 = parts
                .next()
                .ok_or_else(|| E::custom("missing value"))?
                .parse()
                .map_err(E::custom)?;

            let factor = match parts.next().unwrap_or("").to_ascii_lowercase().as_str() {
                "" | "kg" => 1.0,
                "massearth" | "mearth" => 5.9722e24, // M🜨
                "masssol" | "msol" | "masssun" => 1.9885e30, // M☉
                other => return Err(E::custom(format!("unknown mass unit: {other}"))),
            };

            Ok(value * factor)
        }
    }

    deserializer.deserialize_any(MassVisitor)
}

fn de_distance<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    struct DistanceVisitor;

    impl<'de> serde::de::Visitor<'de> for DistanceVisitor {
        type Value = f64; // meters

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str(
                "a number (m) or a string like \"0.5 AU\" / \"4.2 ly\" / \"1 pc\" / \"7 km\"",
            )
        }

        // ---------- numeric literals (interpreted as meters) ----------

        fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v) // already in meters
        }
        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v as f64)
        }

        // ---------- strings with optional unit ----------

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let mut parts = s.split_whitespace();
            let value: f64 = parts
                .next()
                .ok_or_else(|| E::custom("missing value"))?
                .parse()
                .map_err(E::custom)?;

            // default is meters if no unit supplied
            let unit = parts.next().unwrap_or("").to_ascii_lowercase();

            // conversion factors to meters
            let factor_m = match unit.as_str() {
                "" | "m" => 1.0,
                "km" => 1_000.0,
                "au" => 1.495_978_707e11, // meters per AU
                "ly" | "lightyear" | "lightyears" => 9.460_730_472_580_8e15, // meters per ly
                "pc" | "parsec" | "parsecs" => 3.085_677_581_491_37e16, // meters per pc
                other => return Err(E::custom(format!("unknown distance unit: {other}"))),
            };

            Ok(value * factor_m)
        }
    }

    deserializer.deserialize_any(DistanceVisitor)
}

fn de_time<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    struct TimeVisitor;

    impl<'de> serde::de::Visitor<'de> for TimeVisitor {
        type Value = f64; // seconds

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str(r#"a number (s) or a string like "2 h", "3 d", "1 yr""#)
        }

        // ---------- numeric literals ----------

        fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v) // already in seconds
        }
        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v as f64)
        }

        // ---------- strings with optional unit ----------

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let mut parts = s.split_whitespace();

            let value: f64 = parts
                .next()
                .ok_or_else(|| E::custom("missing value"))?
                .parse()
                .map_err(E::custom)?;

            // default to seconds if no unit supplied
            let unit = parts.next().unwrap_or("").to_ascii_lowercase();

            const SEC_PER_HOUR: f64 = 3_600.0;
            const SEC_PER_DAY: f64 = 86_400.0;
            const SEC_PER_YEAR: f64 = 31_557_600.0; // 365.25 d (Julian year)

            let factor = match unit.as_str() {
                "" | "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
                "h" | "hr" | "hrs" | "hour" | "hours" => SEC_PER_HOUR,
                "d" | "day" | "days" => SEC_PER_DAY,
                "yr" | "year" | "years" => SEC_PER_YEAR,
                other => return Err(E::custom(format!("unknown time unit: {other}"))),
            };

            Ok(value * factor)
        }
    }

    deserializer.deserialize_any(TimeVisitor)
}

fn default_surface_color() -> [f32; 3] {
    [0.4, 0.4, 0.4]
}

impl OrreryCfg {
    pub fn validate_system(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        use std::collections::BTreeMap;

        ensure!(
            !self.name.is_empty() && self.name.len() <= 128,
            "invalid system name"
        );
        ensure!(
            !self.bodies.is_empty() && self.bodies.len() <= 4096,
            "invalid system body count"
        );
        ensure!(
            [self.position_um.x, self.position_um.y, self.position_um.z]
                .iter()
                .all(|v| v.unsigned_abs() <= 1_u128 << 110),
            "invalid system anchor"
        );
        let mut names = BTreeMap::new();
        let mut stars = 0;
        let mut roots = 0;
        for body in &self.bodies {
            ensure!(
                !body.name.is_empty()
                    && body.name.len() <= 128
                    && names.insert(body.name.as_str(), body).is_none(),
                "invalid or duplicate body name"
            );
            ensure!(
                body.mass.is_finite()
                    && body.mass > 0.
                    && body.radius.is_finite()
                    && (body.radius > 0.
                        || matches!(body.class_params, BodyClass::Barycenter) && body.radius == 0.),
                "invalid body mass or radius"
            );
            if body.parent.is_none() {
                roots += 1;
                ensure!(body.orbit.semi_major == 0., "system root must be fixed");
            }
            let orbit = body.orbit;
            let rotation = body.rotation;
            ensure!(
                [
                    orbit.semi_major,
                    orbit.period,
                    orbit.eccentricity,
                    orbit.inclination,
                    orbit.ascending_node,
                    orbit.arg_of_pericenter,
                    orbit.mean_anomaly,
                    orbit.epoch,
                    rotation.rotation_period,
                    rotation.obliquity,
                    rotation.eq_ascend_node,
                    rotation.rotation_epoch
                ]
                .iter()
                .all(|v| v.is_finite())
                    && orbit.semi_major >= 0.
                    && orbit.period >= 0.
                    && (0. ..1.).contains(&orbit.eccentricity),
                "invalid body orbit or rotation"
            );
            ensure!(
                body.surface_color
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
                "invalid body surface color"
            );
            if let Some(atmosphere) = &body.atmosphere {
                atmosphere.validate()?;
            }
            if let Some(stellar) = &body.stellar {
                ensure!(
                    matches!(body.class_params, BodyClass::Star { .. })
                        && stellar.effective_temperature_k.is_finite()
                        && stellar.effective_temperature_k > 0.
                        && stellar.age_years.is_finite()
                        && stellar.age_years >= 0.,
                    "invalid stellar parameters"
                );
            }
            if let Some(planet) = &body.planet {
                ensure!(
                    matches!(body.class_params, BodyClass::Planet)
                        && planet.temperature_k.is_finite()
                        && planet.temperature_k > 0.
                        && planet.equilibrium_temperature_k.is_finite()
                        && planet.equilibrium_temperature_k > 0.
                        && (0. ..=1.).contains(&planet.bond_albedo)
                        && (0. ..=1.).contains(&planet.ocean_fraction),
                    "invalid planetary parameters"
                );
            }
            match body.class_params {
                BodyClass::Star { lumens } => {
                    stars += 1;
                    ensure!(
                        lumens.is_finite() && lumens > 0.,
                        "system star must have positive finite luminosity"
                    );
                }
                BodyClass::Planet => ensure!(body.parent.is_some(), "planet needs a parent"),
                BodyClass::Barycenter => ensure!(
                    body.radius == 0.
                        && body.atmosphere.is_none()
                        && body.stellar.is_none()
                        && body.planet.is_none(),
                    "barycenter must be virtual"
                ),
            }
        }
        ensure!(stars >= 1, "system requires a star");
        ensure!(roots == 1, "system requires exactly one root");
        for body in &self.bodies {
            if matches!(body.class_params, BodyClass::Barycenter) {
                let components: Vec<_> = self
                    .bodies
                    .iter()
                    .filter(|child| {
                        child.parent.as_ref() == Some(&body.name)
                            && !matches!(child.class_params, BodyClass::Planet)
                    })
                    .collect();
                ensure!(
                    components.len() == 2,
                    "barycenter requires two stellar components or groups"
                );
                let mass: f64 = components.iter().map(|child| child.mass).sum();
                ensure!(
                    (body.mass / mass - 1.0).abs() < 1e-10,
                    "barycenter mass must equal its components"
                );
                ensure!(
                    components
                        .iter()
                        .all(|child| child.orbit.semi_major > 0. && child.orbit.period > 0.),
                    "barycentric components require explicit orbital periods"
                );
            }
            let mut current = body;
            let mut depth = 0;
            while let Some(parent) = &current.parent {
                depth += 1;
                ensure!(depth <= 64, "body ancestry is cyclic or exceeds limit");
                current = names
                    .get(parent.as_str())
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("missing body parent"))?;
            }
        }
        Ok(())
    }
}
