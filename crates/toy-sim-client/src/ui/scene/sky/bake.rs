mod disk;
use bevy::{
    asset::RenderAssetUsages, image::ImageSampler, math::DVec3, prelude::*,
    render::render_resource::*,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use toy_sim_space::GalacticPosition;
use toy_sim_stars::{Star, min_brightness};
pub const RESOLUTION: u32 = 2048;
pub struct Point {
    direction: DVec3,
    flux: [f64; 3],
    angular_radius: f64,
}
pub struct Snapshot {
    pub stars: Vec<Point>,
    origin: GalacticPosition,
    nearest: f64,
    magnitude: f64,
    revision: u64,
    relocation: u64,
}
impl Snapshot {
    pub fn new(
        sources: Vec<(Star, f64)>,
        origin: GalacticPosition,
        magnitude: f64,
        revision: u64,
        relocation: u64,
        mut nearest: f64,
    ) -> Self {
        let cutoff = min_brightness(magnitude);
        let mut stars = Vec::new();
        for (star, radius) in sources {
            let delta = star.position.relative_to(origin);
            let d2 = delta.length_squared();
            if d2 <= 0.0 {
                continue;
            }
            nearest = nearest.min(d2.sqrt());
            let brightness = star.luminosity / d2;
            if brightness < cutoff {
                continue;
            }
            let flux = brightness / (4.0 * std::f64::consts::PI)
                * (5.0 * (brightness / cutoff).log10()).clamp(0.0, 1.0);
            // Bevy's built-in skybox flips Z before sampling the cubemap.
            let mut direction = delta.normalize();
            direction.z = -direction.z;
            stars.push(Point {
                direction,
                flux: star.colour.map(|c| c as f64 * flux),
                angular_radius: (radius / d2.sqrt()).clamp(0.0, 1.0).asin(),
            });
        }
        Self {
            stars,
            origin,
            nearest,
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
        self.magnitude == magnitude && self.revision == revision && self.relocation == relocation
            // Conservative half-texel parallax budget, also well inside the
            // catalogue's 1% brightness movement budget. No rotation dependency.
            && position.relative_to(self.origin).length() <= self.nearest * (0.25 / RESOLUTION as f64)
    }
}
/// Gaia's compact records have no radius. Assume solar luminous surface brightness;
/// this is only a rendering proxy, not an inferred physical stellar measurement.
pub fn estimated_radius(luminosity: f64) -> f64 {
    6.96e8 * (luminosity / toy_sim_stars::SOLAR_LUMENS).sqrt()
}

pub struct Baked {
    pub image: Image,
    pub seconds: f64,
}
// Standard cube sampling order: +X, -X, +Y, -Y, +Z, -Z.
fn project(d: DVec3) -> (usize, f64, f64) {
    let a = d.abs();
    if a.x >= a.y && a.x >= a.z {
        if d.x >= 0. {
            (0, -d.z / a.x, -d.y / a.x)
        } else {
            (1, d.z / a.x, -d.y / a.x)
        }
    } else if a.y >= a.z {
        if d.y >= 0. {
            (2, d.x / a.y, d.z / a.y)
        } else {
            (3, d.x / a.y, -d.z / a.y)
        }
    } else if d.z >= 0. {
        (4, d.x / a.z, -d.y / a.z)
    } else {
        (5, -d.x / a.z, -d.y / a.z)
    }
}
fn direction(face: usize, u: f64, v: f64) -> DVec3 {
    match face {
        0 => DVec3::new(1., -v, -u),
        1 => DVec3::new(-1., -v, u),
        2 => DVec3::new(u, 1., v),
        3 => DVec3::new(u, -1., -v),
        4 => DVec3::new(u, -v, 1.),
        _ => DVec3::new(-u, -v, -1.),
    }
}
fn solid_angle(n: u32, x: usize, y: usize) -> f64 {
    let u = 2.0 * (x as f64 + 0.5) / n as f64 - 1.0;
    let v = 2.0 * (y as f64 + 0.5) / n as f64 - 1.0;
    4.0 / ((n as f64).powi(2) * (1.0 + u * u + v * v).powf(1.5))
}
fn splat_point(data: &mut [u8], n: u32, point: &Point, fraction: f64) {
    let (face, u, v) = project(point.direction);
    let px = (u + 1.) * 0.5 * n as f64 - 0.5;
    let py = (v + 1.) * 0.5 * n as f64 - 0.5;
    let x0 = px.floor();
    let y0 = py.floor();
    for dy in 0..2 {
        for dx in 0..2 {
            let x = x0 + dx as f64;
            let y = y0 + dy as f64;
            let weight = (1.0 - (px - x).abs()) * (1.0 - (py - y).abs());
            // Reproject taps crossing cube edges onto the adjacent face.
            let (f, u, v) = project(direction(
                face,
                2. * (x + 0.5) / n as f64 - 1.,
                2. * (y + 0.5) / n as f64 - 1.,
            ));
            let ix = (((u + 1.) * 0.5 * n as f64).floor() as usize).min(n as usize - 1);
            let iy = (((v + 1.) * 0.5 * n as f64).floor() as usize).min(n as usize - 1);
            let offset = ((f * n as usize + iy) * n as usize + ix) * 16;
            let omega = solid_angle(n, ix, iy);
            for c in 0..3 {
                let offset = offset + c * 4;
                let old = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as f64;
                let value = (old + point.flux[c] * fraction * weight / omega) as f32;
                data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
}
pub fn bake(snapshot: &Snapshot, cancelled: &AtomicBool) -> Option<Baked> {
    bake_while(snapshot, RESOLUTION, || cancelled.load(Ordering::Relaxed))
}
fn bake_while(
    snapshot: &Snapshot,
    resolution: u32,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Baked> {
    if cancelled() {
        return None;
    }
    let start = Instant::now();
    // Allocate the final upload representation directly: no second full-size buffer
    // or per-texel processing. Only star footprints touch the zero-filled image.
    let mip_count = resolution.ilog2() + 1;
    let bytes: usize = (0..mip_count)
        .map(|mip| {
            let n = (resolution >> mip) as usize;
            n * n * 6 * 16
        })
        .sum();
    let mut data = vec![0; bytes];
    let mut offset = 0;
    for mip in 0..mip_count {
        let n = resolution >> mip;
        let len = n as usize * n as usize * 6 * 16;
        // Analytic, flux-conserving mip levels avoid scanning/downsampling a
        // gigabyte of black texels and preserve subpixel stars when minified.
        if cancelled() {
            return None;
        }
        for chunk in snapshot.stars.chunks(256) {
            if cancelled() {
                return None;
            }
            for point in chunk {
                disk::splat(&mut data[offset..offset + len], n, point, &mut cancelled)?;
            }
        }
        offset += len;
    }
    if cancelled() {
        return None;
    }
    // Image::new validates base-level length, so set mip storage afterward.
    let mut image = Image::new_uninit(
        Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        TextureFormat::Rgba32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.data_order = TextureDataOrder::MipMajor;
    image.texture_descriptor.mip_level_count = mip_count;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    Some(Baked {
        image,
        seconds: start.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn solar_brightness_survives_disk_and_point_baking_in_every_mip() {
        let distance = 1.495978707e10;
        for radius in [0.0, 6.96e8] {
            let star = Star {
                id: toy_sim_stars::StarId::gaia(1),
                position: GalacticPosition::from_meters(DVec3::Z * distance),
                luminosity: 3.6e28,
                colour: [1.0, 0.8, 0.6],
            };
            let snapshot = Snapshot::new(
                vec![(star, radius)],
                GalacticPosition::ZERO,
                6.0,
                0,
                0,
                distance,
            );
            let baked = bake_while(&snapshot, 256, || false).unwrap();
            let data = baked.image.data.unwrap();
            let mut offset = 0;
            for mip in 0..=8 {
                let n = 256 >> mip;
                let len = n as usize * n as usize * 6 * 16;
                let mut flux = [0.0; 3];
                let mut peak = 0.0_f32;
                for (pixel, bytes) in data[offset..offset + len].chunks_exact(16).enumerate() {
                    let omega =
                        solid_angle(n, pixel % n as usize, (pixel / n as usize) % n as usize);
                    for c in 0..3 {
                        let value = f32::from_le_bytes(bytes[c * 4..c * 4 + 4].try_into().unwrap());
                        assert!(value.is_finite());
                        peak = peak.max(value);
                        flux[c] += value as f64 * omega;
                    }
                }
                if mip == 0 {
                    assert!(peak > 1e9, "stellar radiance was clipped: {peak}");
                }
                for c in 0..3 {
                    assert!((flux[c] / snapshot.stars[0].flux[c] - 1.0).abs() < 1e-6);
                }
                offset += len;
            }
            assert_eq!(offset, data.len());
        }
    }

    #[test]
    fn snapshot_uses_physical_radius_for_angular_size() {
        let radius = 6.96e8;
        let distance = 1.495978707e11;
        let star = Star {
            id: toy_sim_stars::StarId::gaia(1),
            position: GalacticPosition::from_meters(DVec3::Z * distance),
            luminosity: toy_sim_stars::SOLAR_LUMENS,
            colour: [1.0; 3],
        };
        let snapshot = Snapshot::new(
            vec![(star, radius)],
            GalacticPosition::ZERO,
            6.0,
            0,
            0,
            distance,
        );
        assert!((snapshot.stars[0].angular_radius - (radius / distance).asin()).abs() < 1e-12);
        assert!(snapshot.stars[0].direction.abs_diff_eq(DVec3::NEG_Z, 1e-12));
        assert_eq!(estimated_radius(toy_sim_stars::SOLAR_LUMENS), radius);
    }

    #[test]
    fn cube_axes_and_edges_round_trip() {
        for face in 0..6 {
            for u in [-0.999, -0.2, 0., 0.7, 0.999] {
                for v in [-0.999, 0., 0.999] {
                    let d = direction(face, u, v).normalize();
                    let (f, a, b) = project(d);
                    assert_eq!(f, face);
                    assert!(direction(f, a, b).normalize().distance(d) < 1e-12);
                }
            }
        }
    }
    #[test]
    fn splats_conserve_flux_at_centres_edges_and_corners_at_each_scale() {
        for n in [16, 32, 64] {
            for d in [
                DVec3::X,
                DVec3::NEG_Z,
                DVec3::new(1., 1., 0.),
                DVec3::ONE,
                DVec3::new(-1., -1., -1.),
            ] {
                let mut data = vec![0; n as usize * n as usize * 6 * 16];
                let point = Point {
                    direction: d.normalize(),
                    flux: [1e-4, 2e-4, 4e-4],
                    angular_radius: 0.0,
                };
                splat_point(&mut data, n, &point, 1.0);
                let mut sum = [0.; 3];
                for (pixel, bytes) in data.chunks_exact(16).enumerate() {
                    let x = pixel % n as usize;
                    let y = (pixel / n as usize) % n as usize;
                    for c in 0..3 {
                        sum[c] += f32::from_le_bytes(bytes[c * 4..c * 4 + 4].try_into().unwrap())
                            as f64
                            * solid_angle(n, x, y);
                    }
                }
                for c in 0..3 {
                    assert!(
                        (sum[c] / point.flux[c] - 1.).abs() < 0.002,
                        "{n} {d} {sum:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn cache_accuracy_uses_fixed_resolution_and_settings_invalidate() {
        let origin = GalacticPosition::ZERO;
        let snapshot = Snapshot::new(vec![], origin, 6., 1, 0, 1e9);
        assert!(snapshot.valid(origin.offset_by(DVec3::X * 100_000.0), 6., 1, 0));
        assert!(!snapshot.valid(origin.offset_by(DVec3::X * 130_000.0), 6., 1, 0));
        assert!(snapshot.valid(origin, 6., 1, 0));
        assert!(!snapshot.valid(origin, 7., 1, 0));
        assert!(!snapshot.valid(origin, 6., 2, 0));
        assert!(!snapshot.valid(origin, 6., 1, 1));
    }
    #[test]
    fn cancelled_bakes_stop_before_allocation_and_during_splatting() {
        let mut snapshot = Snapshot::new(
            vec![],
            GalacticPosition::new(0, 0, 0),
            6.,
            0,
            0,
            f64::INFINITY,
        );
        // An invalid resolution would panic if cancellation did not precede allocation.
        assert!(bake_while(&snapshot, 0, || true).is_none());
        snapshot.stars = (0..1024)
            .map(|_| Point {
                direction: DVec3::X,
                flux: [1e-4; 3],
                angular_radius: 0.0,
            })
            .collect();
        let mut checks = 0;
        let result = bake_while(&snapshot, 16, || {
            checks += 1;
            checks == 4 // Cancel after the first 256-star chunk, within the first mip.
        });
        assert!(result.is_none());
        assert_eq!(checks, 4);
        // A subsequent generation has an independent token and completes normally.
        assert!(bake_while(&snapshot, 16, || false).is_some());
    }
    #[test]
    fn image_is_hdr_cube_with_render_only_ownership() {
        let snapshot = Snapshot::new(
            vec![],
            GalacticPosition::new(0, 0, 0),
            6.,
            0,
            0,
            f64::INFINITY,
        );
        let baked = bake_while(&snapshot, 16, || false).unwrap();
        assert_eq!(baked.image.texture_descriptor.size.depth_or_array_layers, 6);
        assert_eq!(
            baked.image.texture_descriptor.format,
            TextureFormat::Rgba32Float
        );
        assert_eq!(
            baked.image.texture_view_descriptor.unwrap().dimension,
            Some(TextureViewDimension::Cube)
        );
        assert_eq!(baked.image.asset_usage, RenderAssetUsages::RENDER_WORLD);
        assert_eq!(baked.image.texture_descriptor.mip_level_count, 5);
        assert_eq!(
            baked.image.data.unwrap().len(),
            (256 + 64 + 16 + 4 + 1) * 6 * 16
        );
        assert_eq!(RESOLUTION, 2048);
    }
}
