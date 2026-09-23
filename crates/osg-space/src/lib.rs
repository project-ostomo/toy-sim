use std::ops::{Add, AddAssign, Sub, SubAssign};

use glam::{DVec3, Vec3};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod spatial;

/// Absolute coordinates (or relative displacements) in integer micrometres.
/// Subtract positions before converting to floating point to preserve local detail.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GalacticPosition {
    pub x: i128,
    pub y: i128,
    pub z: i128,
}

impl GalacticPosition {
    pub const fn to_array(self) -> [i128; 3] {
        [self.x, self.y, self.z]
    }

    pub const UNITS_PER_METRE: i128 = 1_000_000;

    #[inline]
    pub fn from_meters(metres: DVec3) -> Self {
        Self::new(
            (metres.x * 1_000_000.0).round() as i128,
            (metres.y * 1_000_000.0).round() as i128,
            (metres.z * 1_000_000.0).round() as i128,
        )
    }

    pub const ZERO: Self = Self::splat(0);
    pub const X: Self = Self::new(1, 0, 0);

    pub const fn new(x: i128, y: i128, z: i128) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(value: i128) -> Self {
        Self::new(value, value, value)
    }

    #[inline]
    pub fn relative_to(self, origin: Self) -> DVec3 {
        (self - origin).to_meters_64()
    }

    #[inline]
    pub fn offset_by(self, metres: DVec3) -> Self {
        self + Self::from_meters(metres)
    }

    pub fn saturating_add(self, rhs: Self) -> Self {
        Self::new(
            self.x.saturating_add(rhs.x),
            self.y.saturating_add(rhs.y),
            self.z.saturating_add(rhs.z),
        )
    }

    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self::new(
            self.x.saturating_sub(rhs.x),
            self.y.saturating_sub(rhs.y),
            self.z.saturating_sub(rhs.z),
        )
    }
}

impl Add for GalacticPosition {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for GalacticPosition {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl AddAssign for GalacticPosition {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for GalacticPosition {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

/// Most physical separations fit i64 even when the absolute position does not.
/// Keep the wide fallback out of line: otherwise LLVM can eagerly compute both
/// conversions and select afterward, defeating the fast path (see microbenchmark).
#[inline(always)]
fn micrometres_as_f64(value: i128) -> f64 {
    match i64::try_from(value) {
        Ok(local) => local as f64,
        Err(_) => wide_as_f64(value),
    }
}

#[cold]
#[inline(never)]
fn wide_as_f64(value: i128) -> f64 {
    value as f64
}

impl GalacticPosition {
    #[inline]
    pub fn to_meters(self) -> Vec3 {
        self.to_meters_64().as_vec3()
    }

    #[inline]
    pub fn to_meters_64(self) -> DVec3 {
        DVec3::new(
            micrometres_as_f64(self.x),
            micrometres_as_f64(self.y),
            micrometres_as_f64(self.z),
        ) / 1_000_000.0
    }
}

// Human-readable formats use decimal strings so neither TOML's i64 range nor
// JSON consumers' f64 numbers truncate persistent galactic coordinates. Accept
// legacy integer arrays too. Binary formats retain native signed i128 fields.
impl Serialize for GalacticPosition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let coordinates = [self.x, self.y, self.z];
        if serializer.is_human_readable() {
            coordinates.map(|v| v.to_string()).serialize(serializer)
        } else {
            coordinates.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for GalacticPosition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = if deserializer.is_human_readable() {
            #[derive(Deserialize)]
            #[serde(untagged)]
            enum Coordinate {
                Text(String),
                Integer(i64),
                Unsigned(u64),
            }
            let encoded = <[Coordinate; 3]>::deserialize(deserializer)?;
            let mut values = [0; 3];
            for (index, coordinate) in encoded.into_iter().enumerate() {
                values[index] = match coordinate {
                    Coordinate::Text(text) => text.parse().map_err(serde::de::Error::custom)?,
                    Coordinate::Integer(value) => value as i128,
                    Coordinate::Unsigned(value) => value as i128,
                };
            }
            values
        } else {
            <[i128; 3]>::deserialize(deserializer)?
        };
        Ok(Self::new(values[0], values[1], values[2]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_conversion_handles_signed_i64_boundaries_and_wide_distances() {
        for value in [
            0,
            1,
            -1,
            i64::MIN as i128,
            i64::MAX as i128,
            i64::MIN as i128 - 1,
            i64::MAX as i128 + 1,
            1_i128 << 90,
            -(1_i128 << 90),
        ] {
            assert_eq!(micrometres_as_f64(value), value as f64);
        }
    }

    #[test]
    fn nearby_subtraction_preserves_micrometres_anywhere_in_the_galaxy() {
        let origin = GalacticPosition::splat(1_i128 << 90);
        let delta = DVec3::new(0.000001, -0.000002, 0.123456);
        let target = origin.offset_by(delta);
        assert!(target.relative_to(origin).abs_diff_eq(delta, 1e-12));
        assert_eq!(target - origin, GalacticPosition::new(1, -2, 123456));
        assert_eq!(
            GalacticPosition::from_meters(DVec3::splat(1e12)),
            GalacticPosition::splat(1_000_000_000_000_000_000)
        );
    }

    #[test]
    fn human_readable_serialization_preserves_wide_positions_and_reads_legacy_arrays() {
        #[derive(Serialize, Deserialize)]
        struct Saved {
            position: GalacticPosition,
        }
        let original = Saved {
            position: GalacticPosition::new(1_i128 << 90, -(1_i128 << 90), 1),
        };
        let toml = toml::to_string(&original).unwrap();
        assert_eq!(
            toml::from_str::<Saved>(&toml).unwrap().position,
            original.position
        );
        let yaml = serde_yml::to_string(&original).unwrap();
        assert_eq!(
            serde_yml::from_str::<Saved>(&yaml).unwrap().position,
            original.position
        );
        assert_eq!(
            toml::from_str::<Saved>("position = [1, -2, 3]")
                .unwrap()
                .position,
            GalacticPosition::new(1, -2, 3)
        );
    }
}

/// A helper trait for conversion from meters to micrometers.
pub trait ToMicrometersExt {
    fn to_micrometers(self) -> GalacticPosition;
}

impl ToMicrometersExt for Vec3 {
    fn to_micrometers(self) -> GalacticPosition {
        GalacticPosition::from_meters(self.as_dvec3())
    }
}
impl ToMicrometersExt for DVec3 {
    fn to_micrometers(self) -> GalacticPosition {
        GalacticPosition::from_meters(self)
    }
}
