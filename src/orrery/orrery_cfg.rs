
use serde::{Deserialize, Deserializer, Serialize};
use smol_str::SmolStr;

use bevy::asset::Asset;
use bevy::reflect::TypePath;

#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct OrreryCfg {
    pub name: SmolStr,
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
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "class")]
pub enum BodyClass {
    Star {
        lumens: f64,
    },
    #[default]
    Planet,
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
