mod noise;
mod texture;

use crate::orrery_cfg::{Body, BodyClass, PlanetKind};
use anyhow::{Result, ensure};
use glam::DVec3;
use noise::{Noise, lerp, mix, random, smooth};
use rand::RngExt;
use std::f64::consts::{PI, TAU};

pub use texture::{SurfaceTextures, peak_work_bytes, texture_bytes};

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceParameters {
    pub seed: [u8; 32],
    pub kind: PlanetKind,
    pub radius_m: f64,
    pub temperature_k: f64,
    pub bond_albedo: f64,
    pub ocean_fraction: f64,
    pub relief_m: f64,
    pub base_color: [f32; 3],
    pub atmospheric_pressure_pa: f64,
    pub obliquity_rad: f64,
    pub cloud_fraction: f64,
    pub cloud_altitude_m: f64,
    pub cloud_rotation_period_s: f64,
    pub biosphere: bool,
}

impl SurfaceParameters {
    pub fn from_body(body: &Body) -> Option<Self> {
        if !matches!(body.class_params, BodyClass::Planet) {
            return None;
        }
        let planet = body.planet.as_ref()?;

        Some(Self {
            seed: planet.seed,
            kind: planet.kind,
            radius_m: body.radius,
            temperature_k: planet.temperature_k,
            bond_albedo: planet.bond_albedo,
            ocean_fraction: planet.ocean_fraction,
            relief_m: planet.relief_m,
            base_color: body.surface_color,
            atmospheric_pressure_pa: body.atmosphere.as_ref().map_or(0.0, |atmosphere| {
                atmosphere.surface_density
                    * atmosphere.specific_gas_constant
                    * atmosphere.temperature
            }),
            obliquity_rad: body.rotation.obliquity,
            cloud_fraction: planet.cloud_fraction,
            cloud_altitude_m: planet.cloud_altitude_m,
            cloud_rotation_period_s: planet.cloud_rotation_period_s,
            biosphere: planet.biosphere,
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.radius_m.is_finite() && self.radius_m > 0.0,
            "invalid planet radius"
        );
        ensure!(
            self.temperature_k.is_finite() && self.temperature_k > 0.0,
            "invalid surface temperature"
        );
        ensure!(
            (0.0..=1.0).contains(&self.bond_albedo),
            "invalid surface albedo"
        );
        ensure!(
            (0.0..=1.0).contains(&self.ocean_fraction),
            "invalid ocean coverage"
        );
        ensure!(
            (0.0..=1e6_f64.min(self.radius_m * 0.1)).contains(&self.relief_m),
            "invalid surface relief"
        );
        ensure!(
            self.base_color
                .iter()
                .all(|value| (0.0..=1.0).contains(value)),
            "invalid land color"
        );
        ensure!(
            self.atmospheric_pressure_pa.is_finite() && self.atmospheric_pressure_pa >= 0.0,
            "invalid surface pressure"
        );
        ensure!(self.obliquity_rad.is_finite(), "invalid surface obliquity");
        ensure!(
            (0.0..=1.0).contains(&self.cloud_fraction),
            "invalid cloud coverage"
        );
        ensure!(
            self.cloud_altitude_m.is_finite() && self.cloud_altitude_m >= 0.0,
            "invalid cloud altitude"
        );
        ensure!(
            self.cloud_rotation_period_s.is_finite()
                && (self.cloud_rotation_period_s == 0.0
                    || self.cloud_rotation_period_s.abs() >= 60.0),
            "invalid cloud rotation"
        );
        ensure!(
            !self.has_clouds()
                || (self.cloud_altitude_m > 0.0 && self.atmospheric_pressure_pa > 0.0),
            "clouds need an atmosphere"
        );
        Ok(())
    }

    pub fn has_clouds(&self) -> bool {
        self.cloud_fraction > 0.0
            && !matches!(self.kind, PlanetKind::GasGiant | PlanetKind::IceGiant)
    }

    pub fn cloud_rotation_rad_s(&self) -> f64 {
        if self.cloud_rotation_period_s == 0.0 {
            0.0
        } else {
            TAU / self.cloud_rotation_period_s
        }
    }

    pub fn cache_key(&self) -> [u8; 32] {
        let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame CPU planetary surface recipe");
        hash.update(&osg_ship_api::GAME_VERSION.to_le_bytes());
        hash.update(&self.seed);
        hash.update(&[match self.kind {
            PlanetKind::Rocky => 0,
            PlanetKind::Ice => 1,
            PlanetKind::Ocean => 2,
            PlanetKind::IceGiant => 3,
            PlanetKind::GasGiant => 4,
        }]);
        for value in [
            self.radius_m,
            self.temperature_k,
            self.bond_albedo,
            self.ocean_fraction,
            self.relief_m,
            self.atmospheric_pressure_pa,
            self.obliquity_rad,
            self.cloud_fraction,
            self.cloud_altitude_m,
            self.cloud_rotation_period_s,
        ] {
            hash.update(&value.to_le_bytes());
        }
        for value in self.base_color {
            hash.update(&value.to_le_bytes());
        }
        hash.update(&[u8::from(self.biosphere)]);
        *hash.finalize().as_bytes()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceSample {
    pub color: [f32; 3],
    pub height_m: f64,
    pub roughness: f32,
    pub water: bool,
}

struct Crater {
    centre: DVec3,
    radius: f64,
    depth: f64,
}

struct Storm {
    centre: DVec3,
    radius: f64,
    twist: f64,
}

struct Band {
    latitude: f64,
    color: [f32; 3],
}

struct Landforms {
    domain: DVec3,
    continent: f64,
    detail: f64,
    ridge: f64,
    elevation: f64,
}

pub struct SurfaceGenerator {
    params: SurfaceParameters,
    terrain: Noise,
    weather: Noise,
    tint: [f32; 3],
    craters: Vec<Crater>,
    storms: Vec<Storm>,
    bands: Vec<Band>,
    sea_level: f64,
    cloud_level: f64,
}

impl SurfaceGenerator {
    pub fn new(params: SurfaceParameters) -> Result<Self> {
        params.validate()?;

        let terrain = Noise::new(&params.seed, "surface-terrain-v1");
        let weather = Noise::new(&params.seed, "surface-weather-v1");
        let mut palette = random(&params.seed, "surface-palette-v1");
        let tint = mix(
            params.base_color,
            match params.kind {
                PlanetKind::Ice => [0.73, 0.78, 0.80],
                PlanetKind::IceGiant => [0.18, 0.48, 0.62],
                PlanetKind::GasGiant => [0.69, 0.52, 0.34],
                _ => [0.43, 0.32, 0.23],
            },
            palette.random_range(0.1..0.35),
        );

        let mut impacts = random(&params.seed, "surface-impacts-v1");
        let craters = (0..48)
            .map(|_| Crater {
                centre: random_direction(&mut impacts),
                radius: impacts.random_range(0.012_f64..0.18).powf(1.2),
                depth: impacts.random_range(0.1..0.32),
            })
            .collect();

        let mut winds = random(&params.seed, "surface-winds-v1");
        let storms = (0..5)
            .map(|index| {
                let latitude: f64 = winds.random_range(-0.7..0.7);
                let longitude: f64 = winds.random_range(0.0..TAU);
                Storm {
                    centre: DVec3::new(
                        latitude.cos() * longitude.cos(),
                        latitude.cos() * longitude.sin(),
                        latitude.sin(),
                    ),
                    radius: if index == 0 {
                        0.22
                    } else {
                        winds.random_range(0.07..0.16)
                    },
                    twist: winds.random_range(2.0..5.0) * if index % 2 == 0 { 1.0 } else { -1.0 },
                }
            })
            .collect();

        let mut belts = random(&params.seed, "surface-belts-v1");
        let count = belts.random_range(9..16);
        let mut bands = Vec::with_capacity(count + 2);
        for index in 0..=count {
            let latitude = -1.0
                + 2.0 * index as f64 / count as f64
                + if index > 0 && index < count {
                    belts.random_range(-0.045..0.045)
                } else {
                    0.0
                };
            let pale = if params.kind == PlanetKind::IceGiant {
                [0.54, 0.72, 0.77]
            } else {
                [0.86, 0.79, 0.64]
            };
            bands.push(Band {
                latitude,
                color: mix(tint.map(|v| v * 0.62), pale, belts.random_range(0.05..0.95)),
            });
        }

        let mut generator = Self {
            params,
            terrain,
            weather,
            tint,
            craters,
            storms,
            bands,
            sea_level: -2.0,
            cloud_level: 2.0,
        };

        if generator.params.ocean_fraction > 0.0 {
            generator.sea_level = if generator.params.ocean_fraction >= 1.0 {
                2.0
            } else {
                quantile(
                    (0..2048).map(|index| {
                        generator
                            .landforms(equal_area_direction(index, 2048), 0.0)
                            .elevation
                    }),
                    generator.params.ocean_fraction,
                )
            };
        }

        if generator.params.has_clouds() {
            generator.cloud_level = if generator.params.cloud_fraction >= 1.0 {
                -2.0
            } else {
                quantile(
                    (0..2048)
                        .map(|index| generator.cloud_field(equal_area_direction(index, 2048), 0.0)),
                    1.0 - generator.params.cloud_fraction,
                )
            };
        }

        Ok(generator)
    }

    pub fn sample(&self, direction: DVec3) -> SurfaceSample {
        self.sample_filtered(direction, 0.0)
    }

    pub fn sample_filtered(&self, direction: DVec3, angular_footprint: f64) -> SurfaceSample {
        let point = direction.try_normalize().unwrap_or(DVec3::Z);
        let footprint = finite_footprint(angular_footprint);
        match self.params.kind {
            PlanetKind::GasGiant | PlanetKind::IceGiant => self.giant(point, footprint),
            _ => self.ground(point, footprint),
        }
    }

    fn landforms(&self, point: DVec3, footprint: f64) -> Landforms {
        let warp = DVec3::new(
            self.terrain.fractal(point * 2.3, 3, footprint * 2.3),
            self.terrain
                .fractal(point * 2.3 + DVec3::splat(32.1), 3, footprint * 2.3),
            self.terrain
                .fractal(point * 2.3 - DVec3::splat(13.7), 3, footprint * 2.3),
        );
        let domain = point + warp * 0.22;
        let continent = self.terrain.fractal(domain * 2.4, 5, footprint * 3.5);
        let detail = self.terrain.fractal(domain * 40.0, 4, footprint * 56.0);
        let ridge = (1.0
            - self
                .terrain
                .fractal(domain * 15.0, 4, footprint * 21.0)
                .abs()
                * 2.0)
            .max(0.0)
            .powi(5);
        let mountains = smooth(
            0.02,
            0.35,
            self.terrain.fractal(domain * 5.0, 3, footprint * 7.0),
        );
        Landforms {
            domain,
            continent,
            detail,
            ridge,
            elevation: continent + detail * 0.045 + ridge * mountains * 0.15,
        }
    }

    fn ground(&self, point: DVec3, footprint: f64) -> SurfaceSample {
        let Landforms {
            domain,
            continent,
            detail,
            ridge,
            mut elevation,
        } = self.landforms(point, footprint);
        let mut crater_tint = 1.0;
        if self.params.ocean_fraction == 0.0 {
            for crater in &self.craters {
                let resolved = 1.0 - smooth(crater.radius * 0.1, crater.radius * 0.6, footprint);
                if resolved == 0.0 {
                    continue;
                }
                let squared = 2.0 * (1.0 - point.dot(crater.centre));
                if squared > (crater.radius * 1.5).powi(2) {
                    continue;
                }
                let r = squared.max(0.0).sqrt() / crater.radius;
                let bowl = -crater.depth * (1.0 - smooth(0.1, 1.0, r));
                let rim_width = 0.009 + (footprint / crater.radius).powi(2);
                let rim = (-(r - 1.0).powi(2) / rim_width).exp() * crater.depth * 0.4;
                elevation += (bowl + rim) * resolved;
                crater_tint *= 1.0 - 0.2 * (1.0 - smooth(0.65, 0.95, r)) * resolved;
            }
        }

        let water = self.params.ocean_fraction > 0.0 && elevation < self.sea_level;
        let land_height = if self.params.ocean_fraction > 0.0 {
            elevation - self.sea_level
        } else {
            elevation
        };

        let transport = (1.0 + self.params.atmospheric_pressure_pa / 1e5).powf(-0.25);
        let contrast = 85.0 * transport * self.params.obliquity_rad.cos().abs().max(0.2);
        let temperature = self.params.temperature_k + contrast * (0.4 - point.z.abs().powf(1.5))
            - land_height.max(0.0) * self.params.relief_m * 0.003;
        let moisture = self
            .terrain
            .fractal(domain * 7.0 + DVec3::splat(12.0), 4, footprint * 10.0);

        let mut roughness = 0.94;
        let mut color = if water {
            roughness = 0.18;
            mix(
                [0.016, 0.045, 0.085],
                [0.045, 0.21, 0.25],
                1.0 - smooth(0.0, 0.10, self.sea_level - elevation),
            )
        } else if self.params.kind == PlanetKind::Ice {
            roughness = 0.72;
            let cracks = 1.0
                - smooth(
                    0.0,
                    (footprint * 5.0).max(0.018),
                    self.terrain
                        .fractal(domain * 16.0, 3, footprint * 24.0)
                        .abs(),
                );
            mix(self.tint, [0.25, 0.24, 0.22], cracks * 0.55)
        } else {
            let rock = mix(self.tint, [0.34, 0.34, 0.33], ridge * 0.3);
            let damp = smooth(-0.12, 0.24, moisture)
                * (1.0 - smooth(305.0, 335.0, temperature))
                * smooth(260.0, 280.0, temperature);
            let lowland = if self.params.biosphere {
                [0.15, 0.24, 0.11]
            } else {
                self.tint.map(|v| v * 0.72)
            };
            let land = mix(
                rock,
                lowland,
                damp * self.params.ocean_fraction.min(0.5) * 2.0,
            );
            if self.params.ocean_fraction > 0.0 {
                mix(
                    land,
                    [0.65, 0.59, 0.44],
                    1.0 - smooth(0.0, 0.012, land_height),
                )
            } else {
                mix(
                    land,
                    self.tint.map(|v| v * 0.65),
                    smooth(-0.25, 0.18, continent) * 0.4,
                )
            }
        };

        if self.params.ocean_fraction > 0.0 || self.params.kind == PlanetKind::Ice {
            let snow = (1.0 - smooth(250.0, 272.0, temperature + moisture * 12.0))
                * if self.params.kind == PlanetKind::Ice {
                    0.5
                } else {
                    1.0
                };
            color = mix(color, [0.83, 0.88, 0.90], snow);
            roughness = lerp(roughness, 0.76, snow);
        }

        let albedo_tint = (self.params.bond_albedo / 0.3).sqrt().clamp(0.65, 1.35);
        let shade = if water {
            1.0
        } else {
            (1.0 + detail * 0.2) * crater_tint
        };
        SurfaceSample {
            color: color.map(|value| (value as f64 * shade * albedo_tint).clamp(0.0, 1.0) as f32),
            height_m: if water {
                0.0
            } else {
                (land_height * self.params.relief_m)
                    .clamp(-self.params.relief_m, self.params.relief_m)
            },
            roughness: roughness as f32,
            water,
        }
    }

    fn swirl(&self, mut point: DVec3, footprint: f64) -> DVec3 {
        for storm in &self.storms {
            let squared = (point - storm.centre).length_squared();
            if squared > (storm.radius * 1.8).powi(2) || footprint > storm.radius {
                continue;
            }
            let angle = storm.twist * (-squared / storm.radius.powi(2)).exp();
            let (sin, cos) = angle.sin_cos();
            point = point * cos
                + storm.centre.cross(point) * sin
                + storm.centre * storm.centre.dot(point) * (1.0 - cos);
        }
        point
    }

    fn giant(&self, point: DVec3, footprint: f64) -> SurfaceSample {
        let wind = self.swirl(point, footprint);
        let turbulence = self.weather.fractal(wind * 18.0, 4, footprint * 26.0);
        let latitude = wind.z
            + turbulence * 0.025
            + self.weather.fractal(wind * 4.0, 3, footprint * 6.0) * 0.03;
        let pair = self
            .bands
            .windows(2)
            .find(|pair| latitude < pair[1].latitude)
            .unwrap_or_else(|| &self.bands[self.bands.len() - 2..]);
        let width = pair[1].latitude - pair[0].latitude;
        let transition = smooth(0.1, 0.9, (latitude - pair[0].latitude) / width);
        let mut color = mix(pair[0].color, pair[1].color, transition);
        let filament = self.weather.fractal(wind * 55.0, 3, footprint * 90.0);
        color = color.map(|v| v * (1.0 + filament as f32 * 0.12));

        let major = &self.storms[0];
        let distance = (point - major.centre).length();
        let storm = 1.0 - smooth(major.radius * 0.18, major.radius * 0.7, distance);
        let pigment = if self.params.kind == PlanetKind::IceGiant {
            [0.16, 0.34, 0.41]
        } else {
            [0.63, 0.40, 0.28]
        };
        color = mix(color, pigment, storm * 0.55);
        SurfaceSample {
            color: color.map(|v| v.clamp(0.0, 1.0)),
            height_m: 0.0,
            roughness: 0.95,
            water: false,
        }
    }

    fn cloud_field(&self, point: DVec3, footprint: f64) -> f64 {
        let wind = self.swirl(point, footprint);
        let warp = self.weather.fractal(wind * 4.0, 3, footprint * 6.0);
        let domain = wind + DVec3::new(warp, -warp * 0.5, warp * 0.25) * 0.35;
        self.weather.fractal(domain * 3.5, 5, footprint * 6.0) * 0.65
            + self.weather.fractal(domain * 18.0, 4, footprint * 30.0) * 0.65
            + self.weather.fractal(domain * 48.0, 3, footprint * 72.0) * 0.20
            + (wind.z * 10.0 + warp * 2.0).sin() * 0.025
    }

    pub fn cloud_opacity(&self, direction: DVec3, angular_footprint: f64) -> f32 {
        if !self.params.has_clouds() {
            return 0.0;
        }
        let point = direction.try_normalize().unwrap_or(DVec3::Z);
        let footprint = finite_footprint(angular_footprint);
        let edge = (0.11 + footprint * 1.5).min(0.30);
        let envelope = ((self.cloud_field(point, footprint) - self.cloud_level) / edge).tanh();
        let detail = self
            .weather
            .fractal(point * 35.0 + DVec3::splat(17.0), 4, footprint * 52.0);
        let density = 0.45 + smooth(-0.25, 0.25, detail) * 0.55;

        // The calibrated weather boundary stays at half opacity. Density varies
        // inside it, while a smooth envelope leaves translucent outer wisps.
        let opacity = if envelope > 0.0 {
            0.465 * (1.0 + envelope * density)
        } else {
            0.465 * (1.0 + envelope)
        };
        opacity as f32
    }
}

pub fn direction(u: f64, v: f64) -> DVec3 {
    let latitude = PI * (0.5 - v);
    let longitude = TAU * u;
    DVec3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    )
}

fn equal_area_direction(index: usize, count: usize) -> DVec3 {
    let z = 1.0 - 2.0 * (index as f64 + 0.5) / count as f64;
    let longitude = index as f64 * PI * (3.0 - 5_f64.sqrt());
    let radial = (1.0 - z * z).sqrt();
    DVec3::new(radial * longitude.cos(), radial * longitude.sin(), z)
}

fn random_direction(random: &mut impl rand::Rng) -> DVec3 {
    let z: f64 = random.random_range(-1.0..1.0);
    let longitude: f64 = random.random_range(0.0..TAU);
    let radial = (1.0 - z * z).sqrt();
    DVec3::new(radial * longitude.cos(), radial * longitude.sin(), z)
}

fn quantile(values: impl Iterator<Item = f64>, fraction: f64) -> f64 {
    let mut values: Vec<_> = values.collect();
    let index = (fraction * (values.len() - 1) as f64) as usize;
    *values.select_nth_unstable_by(index, f64::total_cmp).1
}

fn finite_footprint(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, PI)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests;
