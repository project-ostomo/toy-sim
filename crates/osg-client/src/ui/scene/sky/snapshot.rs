//! A periodically queried set of stars around a snapshot origin.
//!
//! The query is slow, so it runs rarely; the set it returns is chosen so the
//! GPU can evaluate every star's direction and brightness per frame (true
//! parallax) anywhere within `VALIDITY_RADIUS_M` of the origin without stars
//! missing from, or popping into, the sky.
use bevy::math::DVec3;
use osg_model::Id;
use osg_space::GalacticPosition;
use osg_stars::{Star, StarId};

/// Angular radius (about one 4K pixel at a 70° field of view) at which a star
/// is drawn as an emissive sphere instead of a point sprite.
pub const HANDOVER: f64 = 1.0 / 2048.0;
/// Spheres only render inside the camera far plane; resolvable stars beyond
/// this distance stay point sprites.
pub const MESH_RANGE_M: f64 = 0.9e15;
/// How far the camera may move from the origin before the set is re-queried.
pub const VALIDITY_RADIUS_M: f64 = 0.5 * 9.4607e15;
/// The query keeps stars down to `cutoff / BRIGHTNESS_MARGIN`. A star at least
/// `2 * VALIDITY_RADIUS_M` away brightens by at most `(d / (d - R))² <= 4`
/// within the validity radius, so nothing outside the set can reach the
/// cutoff; nearer stars are included regardless of brightness.
pub const BRIGHTNESS_MARGIN: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeometryKey {
    Catalogue(StarId),
    Celestial(Id),
}

/// A cached candidate for an emissive sphere. Its angular size is evaluated
/// at the current camera position every frame.
#[derive(Clone)]
pub struct GeometryStar {
    pub key: GeometryKey,
    pub position: GalacticPosition,
    pub radius_m: f64,
    pub luminosity: f64,
    pub colour: [f32; 3],
}

/// A sprite candidate, positioned relative to the snapshot origin. The shader
/// suppresses its quad whenever its current angular size calls for a sphere.
#[derive(Clone, Debug)]
pub struct Sprite {
    pub offset: DVec3,
    pub radius_m: f64,
    pub luminosity: f64,
    pub colour: [f32; 3],
}

/// A candidate sky entry: a catalogue or celestial star together with the
/// physical radius used to decide between sprites and geometry.
pub struct Source {
    pub star: Star,
    pub radius_m: f64,
    pub key: GeometryKey,
}

pub struct Snapshot {
    pub stars: Vec<Sprite>,
    pub geometry: Vec<GeometryStar>,
    pub origin: GalacticPosition,
    /// Brightness (`L / d²`) at which sprites have faded out completely.
    pub cutoff: f64,
    magnitude: f64,
    revision: u64,
    relocation: u64,
}
impl Snapshot {
    pub fn new(
        sources: Vec<Source>,
        origin: GalacticPosition,
        magnitude: f64,
        revision: u64,
        relocation: u64,
        cutoff: f64,
    ) -> Self {
        let mut stars = Vec::new();
        let mut geometry = Vec::new();
        for Source {
            star,
            radius_m,
            key,
        } in sources
        {
            let offset = star.position.relative_to(origin);
            // Keep candidates that can become resolvable anywhere in the
            // snapshot's validity region. Handover is evaluated each frame.
            if radius_m > 0.0 && offset.length() < VALIDITY_RADIUS_M + MESH_RANGE_M {
                geometry.push(GeometryStar {
                    key,
                    position: star.position,
                    radius_m,
                    luminosity: star.luminosity,
                    colour: star.colour,
                });
            }
            stars.push(Sprite {
                offset,
                radius_m,
                luminosity: star.luminosity,
                colour: star.colour,
            });
        }
        Self {
            stars,
            geometry,
            origin,
            cutoff,
            magnitude,
            revision,
            relocation,
        }
    }
    pub fn valid(
        &self,
        position: GalacticPosition,
        magnitude: f64,
        revision: u64,
        relocation: u64,
    ) -> bool {
        self.magnitude == magnitude
            && self.revision == revision
            && self.relocation == relocation
            && position.relative_to(self.origin).length() <= VALIDITY_RADIUS_M
    }
}

/// The sprite fade-out brightness for a query that asked for `requested`.
/// When the query hit its star cap, the faintest kept star (`faintest`) may
/// sit above `requested / BRIGHTNESS_MARGIN`; raise the cutoff so the margin
/// still holds and truncated stars cannot pop in.
pub fn effective_cutoff(requested: f64, faintest: Option<f64>) -> f64 {
    faintest.map_or(requested, |faintest| {
        requested.max(faintest * BRIGHTNESS_MARGIN)
    })
}

/// Gaia's compact records have no radius. Assume solar luminous surface brightness;
/// this is only a rendering proxy, not an inferred physical stellar measurement.
pub fn estimated_radius(luminosity: f64) -> f64 {
    6.96e8 * (luminosity / osg_stars::SOLAR_LUMENS).sqrt()
}

/// Uniform-sphere surface radiance for `luminosity` and `radius_m`. The exact
/// apparent irradiance `radiance * π (r/d)²` equals the sprite illuminance
/// `luminosity / (4π d²)`, so spheres and sprites hand over seamlessly.
pub fn star_radiance(luminosity: f64, radius_m: f64) -> f64 {
    luminosity / (4.0 * std::f64::consts::PI * std::f64::consts::PI * radius_m * radius_m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(id: u64, position: DVec3) -> Star {
        Star {
            id: StarId::gaia(id),
            position: GalacticPosition::from_meters(position),
            luminosity: osg_stars::SOLAR_LUMENS,
            temperature_k: 5772.,
            colour: [1.0, 0.8, 0.6],
        }
    }

    #[test]
    fn snapshots_keep_sprites_and_nearby_sphere_candidates() {
        let radius = 6.96e8;
        let distance = 1.495978707e11;
        let sun = star(1, DVec3::Z * distance);
        let snapshot = Snapshot::new(
            vec![Source {
                star: sun,
                radius_m: radius,
                key: GeometryKey::Catalogue(sun.id),
            }],
            GalacticPosition::ZERO,
            6.0,
            0,
            0,
            1.0,
        );
        assert_eq!(snapshot.stars.len(), 1);
        assert_eq!(snapshot.stars[0].radius_m, radius);
        let geometry = &snapshot.geometry[0];
        assert_eq!(geometry.key, GeometryKey::Catalogue(sun.id));
        assert_eq!(geometry.position, sun.position);
        assert_eq!(geometry.radius_m, radius);
        assert_eq!(estimated_radius(osg_stars::SOLAR_LUMENS), radius);

        let snapshot = Snapshot::new(
            vec![Source {
                star: sun,
                radius_m: 0.0,
                key: GeometryKey::Catalogue(sun.id),
            }],
            GalacticPosition::ZERO,
            6.0,
            0,
            0,
            1.0,
        );
        assert!(snapshot.geometry.is_empty());
        // World axes, no cubemap flip.
        assert!(
            snapshot.stars[0]
                .offset
                .abs_diff_eq(DVec3::Z * distance, 1e-3)
        );
        assert_eq!(snapshot.stars[0].luminosity, osg_stars::SOLAR_LUMENS);
        assert_eq!(snapshot.stars[0].colour, [1.0, 0.8, 0.6]);
    }

    #[test]
    fn sprites_are_relative_to_the_snapshot_origin() {
        let origin = GalacticPosition::from_meters(DVec3::new(3.0e16, -2.0e16, 1.0e16));
        let snapshot = Snapshot::new(
            vec![Source {
                star: star(2, DVec3::new(3.0e16, -2.0e16, 1.5e16)),
                radius_m: 0.0,
                key: GeometryKey::Catalogue(StarId::gaia(2)),
            }],
            origin,
            6.0,
            0,
            0,
            1.0,
        );
        assert!(snapshot.stars[0].offset.abs_diff_eq(DVec3::Z * 0.5e16, 1.0));
    }

    #[test]
    fn validity_follows_travel_radius_and_settings() {
        let origin = GalacticPosition::ZERO;
        let snapshot = Snapshot::new(vec![], origin, 6., 1, 0, 1.0);
        assert!(snapshot.valid(
            origin.offset_by(DVec3::X * VALIDITY_RADIUS_M * 0.99),
            6.,
            1,
            0
        ));
        assert!(!snapshot.valid(
            origin.offset_by(DVec3::X * VALIDITY_RADIUS_M * 1.01),
            6.,
            1,
            0
        ));
        assert!(!snapshot.valid(origin, 7., 1, 0));
        assert!(!snapshot.valid(origin, 6., 2, 0));
        assert!(!snapshot.valid(origin, 6., 1, 1));
    }

    #[test]
    fn margin_covers_every_star_beyond_twice_the_validity_radius() {
        // Worst case: a star exactly 2R away, approached head-on by R.
        let before = 1.0 / (2.0 * VALIDITY_RADIUS_M).powi(2);
        let after = 1.0 / VALIDITY_RADIUS_M.powi(2);
        assert!(after / before <= BRIGHTNESS_MARGIN * (1.0 + 1e-12));
    }

    #[test]
    fn truncated_queries_raise_the_cutoff_to_keep_the_margin() {
        assert_eq!(effective_cutoff(1.0, None), 1.0);
        // The cap kept stars down to the requested margin: unchanged.
        assert_eq!(effective_cutoff(1.0, Some(0.25)), 1.0);
        // The cap stopped at 0.5: only stars brighter than 2.0 are complete.
        assert_eq!(effective_cutoff(1.0, Some(0.5)), 2.0);
    }

    #[test]
    fn emissive_sphere_radiance_matches_sprite_illuminance() {
        let radius = 6.96e8;
        let pi = std::f64::consts::PI;
        for distance in [radius / HANDOVER, radius / HANDOVER * 100.0] {
            let sphere = star_radiance(osg_stars::SOLAR_LUMENS, radius) * pi * radius * radius
                / (distance * distance);
            let sprite = osg_stars::SOLAR_LUMENS / (4.0 * pi * distance * distance);
            assert!((sphere - sprite).abs() < sprite * 1e-12);
        }
    }
}
