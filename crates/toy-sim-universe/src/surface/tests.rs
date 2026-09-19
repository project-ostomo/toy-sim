use super::*;

fn parameters(kind: PlanetKind) -> SurfaceParameters {
    SurfaceParameters {
        seed: [7; 32],
        kind,
        radius_m: 6e6,
        temperature_k: 288.0,
        bond_albedo: 0.3,
        ocean_fraction: if kind == PlanetKind::Ocean { 0.65 } else { 0.0 },
        relief_m: 12_000.0,
        base_color: [0.38, 0.30, 0.23],
        atmospheric_pressure_pa: 1e5,
        obliquity_rad: 0.3,
        cloud_fraction: 0.0,
        cloud_altitude_m: 5000.0,
        cloud_rotation_period_s: 1e6,
        biosphere: false,
    }
}

#[test]
fn sample_seams_poles_and_generation_order_are_consistent() {
    let generator = SurfaceGenerator::new(parameters(PlanetKind::Ocean)).unwrap();
    let repeated = SurfaceGenerator::new(parameters(PlanetKind::Ocean)).unwrap();
    for latitude in [0.0, 0.01, 0.2, 0.6, 0.99, 1.0] {
        let a = generator.sample(direction(0.0, latitude));
        let b = generator.sample(direction(1.0, latitude));
        assert!((a.height_m - b.height_m).abs() < 1e-7);
        assert_eq!(a.color, b.color);
        assert_eq!(a.color, repeated.sample(direction(0.0, latitude)).color);
    }
    for pole in [0.0, 1.0] {
        let expected = generator.sample(direction(0.0, pole));
        for longitude in [0.0, 0.13, 0.5, 0.97] {
            let actual = generator.sample(direction(longitude, pole));
            assert!((expected.height_m - actual.height_m).abs() < 1e-7);
            assert_eq!(expected.color, actual.color);
        }
    }
    let mut changed = parameters(PlanetKind::Ocean);
    changed.seed[0] += 1;
    let other = SurfaceGenerator::new(changed).unwrap();
    assert_ne!(
        generator.bake(32).unwrap().color,
        other.bake(32).unwrap().color
    );
    assert_eq!(
        generator.bake(32).unwrap().color,
        repeated.bake(32).unwrap().color
    );
}

#[test]
fn families_have_finite_bounded_outputs_and_exact_memory_estimates() {
    for kind in [
        PlanetKind::Rocky,
        PlanetKind::Ice,
        PlanetKind::Ocean,
        PlanetKind::GasGiant,
        PlanetKind::IceGiant,
    ] {
        let mut params = parameters(kind);
        params.cloud_fraction = if kind == PlanetKind::Ocean { 0.45 } else { 0.0 };
        let generator = SurfaceGenerator::new(params.clone()).unwrap();
        for index in 0..2048 {
            let sample = generator.sample(equal_area_direction(index, 2048));
            assert!(
                sample
                    .color
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            );
            assert!((-params.relief_m..=params.relief_m).contains(&sample.height_m));
            assert!((0.0..=1.0).contains(&sample.roughness));
        }
        let texture = generator.bake(64).unwrap();
        let bytes = texture.color.len()
            + texture.normal.len()
            + texture.material.len()
            + texture.clouds.as_ref().map_or(0, Vec::len);
        assert_eq!(bytes, texture_bytes(64, params.has_clouds()));
        assert!(peak_work_bytes(64, params.has_clouds()) >= bytes + 64 * 32 * 4);
        assert_eq!(texture.mip_count, 7);
        for normal in texture.normal.chunks_exact(4) {
            let normal = DVec3::new(normal[0] as f64, normal[1] as f64, normal[2] as f64) / 127.5
                - DVec3::ONE;
            assert!((normal.length() - 1.0).abs() < 0.015);
        }
    }
}

#[test]
fn spherical_height_normals_match_analytic_slopes_in_bevy_tangent_basis() {
    let width = 256;
    let height = width / 2;
    let radius = 6e6;
    let amplitude = radius * 0.002;
    for axis in [DVec3::X, DVec3::Z, DVec3::new(1.0, 2.0, 3.0).normalize()] {
        let heights: Vec<_> = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    (direction(
                        (x as f64 + 0.5) / width as f64,
                        (y as f64 + 0.5) / height as f64,
                    )
                    .dot(axis)
                        * amplitude) as f32
                })
            })
            .collect();
        for y in [0, 1, height / 2, height - 2, height - 1] {
            for x in [0, 1, width / 4, width - 2, width - 1] {
                let u = (x as f64 + 0.5) / width as f64;
                let v = (y as f64 + 0.5) / height as f64;
                let radial = direction(u, v);
                let east = DVec3::new(-(TAU * u).sin(), (TAU * u).cos(), 0.0);
                let south = east.cross(radial);
                let tangent = texture::height_normal(&heights, width, height, radius, x, y);
                let actual = east * tangent.x + south * tangent.y + radial * tangent.z;
                let expected = (radial - (axis - radial * radial.dot(axis)) * (amplitude / radius))
                    .normalize();
                assert!(
                    (actual - expected).length() < 2e-6,
                    "x={x} y={y} actual={actual} expected={expected}"
                );
            }
        }
    }
}

#[test]
fn color_mips_average_linear_light_and_spherical_area() {
    let mut pixels = vec![0_u8; 4 * 2 * 4 + 2 * 4 + 4];
    for y in 0..2 {
        for x in 0..4 {
            let value = if x % 2 == 0 { 0 } else { 255 };
            pixels[(y * 4 + x) * 4..(y * 4 + x + 1) * 4]
                .copy_from_slice(&[value, value, value, 255]);
        }
    }
    assert!(texture::build_mips(
        &mut pixels,
        4,
        2,
        texture::TextureKind::Color,
        &mut || false
    ));
    assert!((pixels[pixels.len() - 4] as i32 - 188).abs() <= 1);

    let mut caps = vec![0_u8; (8 * 4 + 4 * 2 + 2 + 1) * 4];
    for y in 0..4 {
        for x in 0..8 {
            let value = if y == 0 || y == 3 { 255 } else { 0 };
            caps[(y * 8 + x) * 4..(y * 8 + x + 1) * 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
    assert!(texture::build_mips(
        &mut caps,
        8,
        4,
        texture::TextureKind::Color,
        &mut || false
    ));
    // Polar rows cover 29.3% of the sphere, although they occupy half the pixels.
    let white_fraction = 1.0 - (PI / 4.0).cos();
    let expected = (1.055 * white_fraction.powf(1.0 / 2.4) - 0.055) * 255.0;
    assert!((caps[caps.len() - 4] as f64 - expected).abs() < 2.0);
}

#[test]
fn ocean_cloud_coverage_and_biosphere_follow_declared_parameters() {
    for fraction in [0.0, 0.2, 0.65, 0.9, 1.0] {
        let mut params = parameters(PlanetKind::Ocean);
        params.ocean_fraction = fraction;
        params.cloud_fraction = fraction;
        let generator = SurfaceGenerator::new(params).unwrap();
        let mut water = 0;
        let mut cloudy = 0;
        for index in 0..4096 {
            let point = equal_area_direction(index, 4096);
            water += usize::from(generator.sample(point).water);
            cloudy += usize::from(generator.cloud_opacity(point, 0.0) > 0.465);
        }
        assert!((water as f64 / 4096.0 - fraction).abs() < 0.03);
        assert!((cloudy as f64 / 4096.0 - fraction).abs() < 0.04);
    }
    let params = parameters(PlanetKind::Ocean);
    let without_life = SurfaceGenerator::new(params.clone()).unwrap();
    let with_life = SurfaceGenerator::new(SurfaceParameters {
        biosphere: true,
        ..params
    })
    .unwrap();
    let mut different = 0;
    for index in 0..1024 {
        let point = equal_area_direction(index, 1024);
        let a = without_life.sample(point);
        let b = with_life.sample(point);
        assert_eq!(a.height_m, b.height_m);
        different += usize::from(a.color != b.color);
    }
    assert!(different > 10);
}

#[test]
fn cloud_interiors_vary_in_density_and_edges_remain_translucent() {
    let mut params = parameters(PlanetKind::Ocean);
    params.cloud_fraction = 0.5;
    let generator = SurfaceGenerator::new(params).unwrap();
    let mut interiors = Vec::new();
    let mut wisps = 0;
    let mut coarse_opacity = 0.0;
    let mut fine_opacity = 0.0;

    for index in 0..4096 {
        let point = equal_area_direction(index, 4096);
        let alpha = generator.cloud_opacity(point, TAU / 1024.0);
        assert!((0.0..=0.93).contains(&alpha));
        if alpha > 0.465 {
            interiors.push(alpha);
        }
        wisps += usize::from((0.05..0.35).contains(&alpha));
        fine_opacity += alpha as f64;
        coarse_opacity += generator.cloud_opacity(point, TAU / 128.0) as f64;
    }

    interiors.sort_by(f32::total_cmp);
    let lower = interiors[interiors.len() / 10];
    let upper = interiors[interiors.len() * 9 / 10];
    assert!(upper - lower > 0.12);
    assert!(upper < 0.90);
    assert!(wisps > 400);
    assert!((coarse_opacity - fine_opacity).abs() / 4096.0 < 0.06);

    for v in [0.0, 0.2, 0.6, 1.0] {
        let a = generator.cloud_opacity(direction(0.0, v), 0.0);
        let b = generator.cloud_opacity(direction(1.0, v), 0.0);
        assert_eq!(a, b);
    }
}

#[test]
fn octave_filter_removes_unresolvable_noise() {
    let noise = Noise::new(&[1; 32], "filter-test");
    for index in 0..128 {
        let point = equal_area_direction(index, 128) * 17.0;
        assert_eq!(noise.fractal(point, 12, 1.0), 0.0);
        assert_eq!(noise.fractal(point, 12, 0.6), noise.fractal(point, 1, 0.6));
    }
}

#[test]
fn lod_filters_detail_without_relocating_continents_or_giant_belts() {
    for kind in [PlanetKind::Ocean, PlanetKind::GasGiant] {
        let generator = SurfaceGenerator::new(parameters(kind)).unwrap();
        let mut water_changes = 0;
        let mut color_error = 0.0;
        let count = 4096;
        for index in 0..count {
            let point = equal_area_direction(index, count);
            let coarse = generator.sample_filtered(point, TAU / 256.0);
            let fine = generator.sample_filtered(point, TAU / 2048.0);
            water_changes += usize::from(coarse.water != fine.water);
            color_error += coarse
                .color
                .iter()
                .zip(fine.color)
                .map(|(a, b)| (*a as f64 - b as f64).abs())
                .sum::<f64>();
        }
        assert!(water_changes < count / 10, "kind={kind:?}");
        assert!(color_error / ((count * 3) as f64) < 0.07, "kind={kind:?}");
    }
}

#[test]
fn cancellation_can_stop_sampling_normals_and_mip_generation() {
    let generator = SurfaceGenerator::new(parameters(PlanetKind::Rocky)).unwrap();
    for limit in [1, 5, 36, 70] {
        let mut calls = 0;
        let result = generator
            .bake_while(64, || {
                calls += 1;
                calls >= limit
            })
            .unwrap();
        assert!(result.is_none());
        assert_eq!(calls, limit);
    }
    assert!(generator.bake(17).is_err());
}

#[test]
fn invalid_recipes_are_rejected_and_cache_keys_cover_weather() {
    let params = parameters(PlanetKind::Ocean);
    let mut changed = params.clone();
    changed.cloud_rotation_period_s = -changed.cloud_rotation_period_s;
    assert_ne!(params.cache_key(), changed.cache_key());
    changed = params.clone();
    changed.relief_m = f64::NAN;
    assert!(SurfaceGenerator::new(changed).is_err());
    changed = params;
    changed.cloud_fraction = 0.5;
    changed.atmospheric_pressure_pa = 0.0;
    assert!(SurfaceGenerator::new(changed).is_err());
}

#[test]
fn cached_materials_are_invalidated_when_effective_recipe_changes() {
    let params = parameters(PlanetKind::Ocean);
    let key = params.cache_key();
    let mut variants = Vec::new();
    let mut change = |edit: fn(&mut SurfaceParameters)| {
        let mut variant = params.clone();
        edit(&mut variant);
        variants.push(variant.cache_key());
    };
    change(|p| p.seed[31] ^= 1);
    change(|p| p.kind = PlanetKind::Ice);
    change(|p| p.radius_m *= 2.0);
    change(|p| p.temperature_k += 10.0);
    change(|p| p.bond_albedo *= 0.5);
    change(|p| p.ocean_fraction *= 0.5);
    change(|p| p.relief_m *= 0.5);
    change(|p| p.base_color[1] *= 0.5);
    change(|p| p.atmospheric_pressure_pa *= 0.5);
    change(|p| p.obliquity_rad += 0.1);
    change(|p| p.cloud_fraction = 0.4);
    change(|p| p.cloud_altitude_m *= 2.0);
    change(|p| p.cloud_rotation_period_s *= -1.0);
    change(|p| p.biosphere = true);
    assert!(variants.iter().all(|variant| *variant != key));
    variants.sort();
    variants.dedup();
    assert_eq!(variants.len(), 14);
}
