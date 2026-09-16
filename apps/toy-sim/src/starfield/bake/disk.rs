//! Uniform angular disks rasterized directly onto all intersecting cube faces.
use super::{Point, direction, project, solid_angle, splat_point};

#[derive(Clone, Copy)]
struct Bounds {
    face: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

fn bounds(point: &Point, n: u32, face: usize) -> Option<Bounds> {
    let forward = direction(face, 0.0, 0.0);
    let f = point.direction.dot(forward);
    let sin = point.angular_radius.sin();
    if f < -sin {
        return None;
    }
    let axis_bounds = |axis| {
        if f <= sin {
            return (-1.0, 1.0); // Cap crosses this face's projection horizon.
        }
        let a = point.direction.dot(axis);
        let denominator = f * f - sin * sin;
        let spread = sin * (f * f + a * a - sin * sin).max(0.0).sqrt();
        (
            (a * f - spread) / denominator,
            (a * f + spread) / denominator,
        )
    };
    let (u0, u1) = axis_bounds(direction(face, 1.0, 0.0) - forward);
    let (v0, v1) = axis_bounds(direction(face, 0.0, 1.0) - forward);
    if u0 > 1.0 || u1 < -1.0 || v0 > 1.0 || v1 < -1.0 {
        return None;
    }
    let lower = |u: f64| (((u + 1.0) * 0.5 * n as f64).floor() - 1.0).clamp(0.0, n as f64) as usize;
    let upper = |u: f64| (((u + 1.0) * 0.5 * n as f64).ceil() + 1.0).clamp(0.0, n as f64) as usize;
    Some(Bounds {
        face,
        x0: lower(u0),
        x1: upper(u1),
        y0: lower(v0),
        y1: upper(v1),
    })
}

pub(super) fn splat(
    data: &mut [u8],
    n: u32,
    point: &Point,
    cancelled: &mut impl FnMut() -> bool,
) -> Option<()> {
    let (_, u, v) = project(point.direction);
    let x = (((u + 1.0) * 0.5 * n as f64) as usize).min(n as usize - 1);
    let y = (((v + 1.0) * 0.5 * n as f64) as usize).min(n as usize - 1);
    let radius_texels = point.angular_radius / solid_angle(n, x, y).sqrt();
    // Continuous point-to-disk transition for subpixel stars and small mip levels.
    let fraction = (radius_texels - 0.5).clamp(0.0, 1.0);
    if fraction == 0.0 {
        splat_point(data, n, point, 1.0);
        return Some(());
    }
    let faces: Vec<_> = (0..6).filter_map(|face| bounds(point, n, face)).collect();
    let edge_cos = point.angular_radius.cos();
    let inner_cos = (point.angular_radius - 2.0 / n as f64).max(0.0).cos();
    let outer_cos = (point.angular_radius + 2.0 / n as f64)
        .min(std::f64::consts::PI)
        .cos();
    let coverage = |face, x: usize, y: usize| {
        let ray = |dx, dy| {
            direction(
                face,
                2.0 * (x as f64 + dx) / n as f64 - 1.0,
                2.0 * (y as f64 + dy) / n as f64 - 1.0,
            )
            .normalize()
        };
        let dot = ray(0.5, 0.5).dot(point.direction);
        if dot >= inner_cos {
            return 1.0;
        }
        if dot < outer_cos {
            return 0.0;
        }
        // Supersample only boundary texels; interiors use one direction test.
        let mut hits = 0;
        for sy in 0..4 {
            for sx in 0..4 {
                if ray((sx as f64 + 0.5) / 4.0, (sy as f64 + 0.5) / 4.0).dot(point.direction)
                    >= edge_cos
                {
                    hits += 1;
                }
            }
        }
        hits as f64 / 16.0
    };
    // Two bounded passes avoid storing another image-sized buffer. Normalize by
    // covered solid angle, preserving total flux at seams and in every mip level.
    let mut covered_omega = 0.0;
    for b in &faces {
        for y in b.y0..b.y1 {
            if cancelled() {
                return None;
            }
            for x in b.x0..b.x1 {
                covered_omega += coverage(b.face, x, y) * solid_angle(n, x, y);
            }
        }
    }
    if covered_omega == 0.0 {
        splat_point(data, n, point, 1.0);
        return Some(());
    }
    if fraction < 1.0 {
        splat_point(data, n, point, 1.0 - fraction);
    }
    for b in &faces {
        for y in b.y0..b.y1 {
            if cancelled() {
                return None;
            }
            for x in b.x0..b.x1 {
                let weight = fraction * coverage(b.face, x, y) / covered_omega;
                if weight == 0.0 {
                    continue;
                }
                let offset = ((b.face * n as usize + y) * n as usize + x) * 16;
                for c in 0..3 {
                    let offset = offset + c * 4;
                    let old =
                        f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as f64;
                    let value = (old + point.flux[c] * weight) as f32;
                    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn disks_preserve_flux_and_angular_extent_across_cube_seams_and_mips() {
        for n in [8, 16, 64, 128] {
            for (axis, angular_radius) in [
                DVec3::X,
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::ONE,
                DVec3::new(-1.0, -1.0, 0.2),
            ]
            .into_iter()
            .flat_map(|axis| [0.12, 1.2].map(|radius| (axis, radius)))
            {
                let point = Point {
                    direction: axis.normalize(),
                    flux: [0.01, 0.02, 0.04],
                    angular_radius,
                };
                let mut data = vec![0; n as usize * n as usize * 6 * 16];
                splat(&mut data, n, &point, &mut || false).unwrap();
                let mut flux = [0.0; 3];
                let mut occupied = 0;
                for (pixel, bytes) in data.chunks_exact(16).enumerate() {
                    let x = pixel % n as usize;
                    let y = (pixel / n as usize) % n as usize;
                    let face = pixel / (n as usize * n as usize);
                    let value = f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64;
                    for c in 0..3 {
                        flux[c] += f32::from_le_bytes(bytes[c * 4..c * 4 + 4].try_into().unwrap())
                            as f64
                            * solid_angle(n, x, y);
                    }
                    if n >= 64 {
                        let ray = direction(
                            face,
                            2.0 * (x as f64 + 0.5) / n as f64 - 1.0,
                            2.0 * (y as f64 + 0.5) / n as f64 - 1.0,
                        )
                        .normalize();
                        let angle = ray.dot(point.direction).clamp(-1.0, 1.0).acos();
                        if angle < point.angular_radius - 2.0 / n as f64 {
                            assert!(
                                value > 0.0,
                                "missing disk interior: {n} {axis} {face} {x} {y}"
                            );
                        }
                        if value > 0.0 {
                            occupied += 1;
                            assert!(
                                angle <= point.angular_radius + 2.0 / n as f64,
                                "disk extends beyond its radius"
                            );
                        }
                    }
                }
                for c in 0..3 {
                    assert!(
                        (flux[c] / point.flux[c] - 1.0).abs() < 0.002,
                        "flux {flux:?} at {n} {axis}"
                    );
                }
                if n >= 64 {
                    assert!(occupied > 20, "disk collapsed to a point");
                }
            }
        }
    }

    #[test]
    fn approaching_a_star_increases_its_disk_area() {
        let count = |radius| {
            let point = Point {
                direction: DVec3::Z,
                flux: [0.01; 3],
                angular_radius: radius,
            };
            let mut data = vec![0; 128 * 128 * 6 * 16];
            splat(&mut data, 128, &point, &mut || false).unwrap();
            data.chunks_exact(16)
                .filter(|p| f32::from_le_bytes(p[..4].try_into().unwrap()) > 0.0)
                .count()
        };
        let small = count(0.1);
        let large = count(0.2);
        assert!(large > small * 3 && large < small * 5);
    }

    #[test]
    fn large_disks_can_be_cancelled_during_rasterization() {
        let point = Point {
            direction: DVec3::X,
            flux: [1.0; 3],
            angular_radius: 1.5,
        };
        let mut data = vec![0; 128 * 128 * 6 * 16];
        let mut checks = 0;
        assert!(
            splat(&mut data, 128, &point, &mut || {
                checks += 1;
                checks == 3
            })
            .is_none()
        );
        assert_eq!(checks, 3);
    }
}
