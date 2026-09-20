use super::{SurfaceGenerator, direction};
use anyhow::{Result, ensure};
use glam::DVec3;
use std::f64::consts::{PI, TAU};

pub struct SurfaceTextures {
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub color: Vec<u8>,
    pub normal: Vec<u8>,
    pub material: Vec<u8>,
    pub clouds: Option<Vec<u8>>,
}

fn valid_width(width: u32) -> bool {
    width.is_power_of_two() && (16..=4096).contains(&width)
}

fn mip_bytes(mut width: u32, mut height: u32) -> usize {
    let mut bytes = 0;
    loop {
        bytes += width as usize * height as usize * 4;
        if width == 1 && height == 1 {
            return bytes;
        }
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
}

pub fn texture_bytes(width: u32, clouds: bool) -> usize {
    assert!(
        valid_width(width),
        "surface width must be a power of two from 16 to 4096"
    );
    3 * mip_bytes(width, width / 2)
        + if clouds {
            mip_bytes(width / 2, width / 4)
        } else {
            0
        }
}

pub fn peak_work_bytes(width: u32, clouds: bool) -> usize {
    // All mip arrays are allocated once at their final length. Only the f32
    // height field, generator state, and small calibration buffers are extra.
    texture_bytes(width, clouds) + width as usize * (width as usize / 2) * 4 + 128 * 1024
}

impl SurfaceGenerator {
    pub fn bake(&self, width: u32) -> Result<SurfaceTextures> {
        Ok(self.bake_while(width, || false)?.expect("uncancelled bake"))
    }

    pub fn bake_while(
        &self,
        width: u32,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<SurfaceTextures>> {
        ensure!(valid_width(width), "invalid surface texture width");
        if cancelled() {
            return Ok(None);
        }

        let height = width / 2;
        let pixels = width as usize * height as usize;
        let bytes = mip_bytes(width, height);
        let mut color = vec![0; bytes];
        let mut normal = vec![0; bytes];
        let mut material = vec![0; bytes];
        let mut heights = vec![0.0_f32; pixels];
        let footprint = TAU / width as f64;

        for y in 0..height {
            if cancelled() {
                return Ok(None);
            }
            for x in 0..width {
                let sample = self.sample_filtered(
                    direction(
                        (x as f64 + 0.5) / width as f64,
                        (y as f64 + 0.5) / height as f64,
                    ),
                    footprint,
                );
                let pixel = (y * width + x) as usize;
                heights[pixel] = sample.height_m as f32;
                color[pixel * 4..pixel * 4 + 3].copy_from_slice(&sample.color.map(to_byte));
                color[pixel * 4 + 3] = 255;
                material[pixel * 4..pixel * 4 + 4].copy_from_slice(&[
                    255,
                    to_byte(sample.roughness),
                    0,
                    255,
                ]);
            }
        }

        for y in 0..height {
            if cancelled() {
                return Ok(None);
            }
            for x in 0..width {
                let pixel = (y * width + x) as usize;
                let n = height_normal(&heights, width, height, self.params.radius_m, x, y);
                normal[pixel * 4..pixel * 4 + 3]
                    .copy_from_slice(&n.to_array().map(|v| to_byte((v * 0.5 + 0.5) as f32)));
                normal[pixel * 4 + 3] = 255;
            }
        }

        drop(heights);
        for (pixels, kind) in [
            (&mut color, TextureKind::Color),
            (&mut normal, TextureKind::Normal),
            (&mut material, TextureKind::Material),
        ] {
            if !build_mips(pixels, width, height, kind, &mut cancelled) {
                return Ok(None);
            }
        }

        let clouds = if self.params.has_clouds() {
            let cloud_width = width / 2;
            let cloud_height = height / 2;
            let mut pixels = vec![0; mip_bytes(cloud_width, cloud_height)];
            for y in 0..cloud_height {
                if cancelled() {
                    return Ok(None);
                }
                for x in 0..cloud_width {
                    let point = direction(
                        (x as f64 + 0.5) / cloud_width as f64,
                        (y as f64 + 0.5) / cloud_height as f64,
                    );
                    let alpha = self.cloud_opacity(point, TAU / cloud_width as f64);
                    let pixel = (y * cloud_width + x) as usize;
                    pixels[pixel * 4..pixel * 4 + 4].copy_from_slice(&[
                        239,
                        242,
                        245,
                        to_byte(alpha),
                    ]);
                }
            }
            if !build_mips(
                &mut pixels,
                cloud_width,
                cloud_height,
                TextureKind::Color,
                &mut cancelled,
            ) {
                return Ok(None);
            }
            Some(pixels)
        } else {
            None
        };

        Ok(Some(SurfaceTextures {
            width,
            height,
            mip_count: width.ilog2() + 1,
            color,
            normal,
            material,
            clouds,
        }))
    }
}

fn reflected_height(heights: &[f32], width: u32, height: u32, mut x: i32, mut y: i32) -> f64 {
    if y < 0 {
        y = -y - 1;
        x += width as i32 / 2;
    } else if y >= height as i32 {
        y = 2 * height as i32 - y - 1;
        x += width as i32 / 2;
    }
    heights[y as usize * width as usize + x.rem_euclid(width as i32) as usize] as f64
}

pub(super) fn height_normal(
    heights: &[f32],
    width: u32,
    height: u32,
    radius: f64,
    x: u32,
    y: u32,
) -> DVec3 {
    let colatitude = PI * (y as f64 + 0.5) / height as f64;
    let east_spacing = colatitude.sin() * TAU * radius / width as f64;
    let south_spacing = PI * radius / height as f64;
    let at = |dx, dy| reflected_height(heights, width, height, x as i32 + dx, y as i32 + dy);
    let dx = (at(1, 0) - at(-1, 0)) / (2.0 * east_spacing);
    let dy = (at(0, 1) - at(0, -1)) / (2.0 * south_spacing);
    // Bevy's UV sphere tangent follows increasing longitude; its bitangent
    // follows increasing v, towards the south pole.
    DVec3::new(-dx, -dy, 1.0)
        .try_normalize()
        .unwrap_or(DVec3::Z)
}

#[derive(Clone, Copy)]
pub(super) enum TextureKind {
    Color,
    Normal,
    Material,
}

pub(super) fn build_mips(
    pixels: &mut [u8],
    mut width: u32,
    mut height: u32,
    kind: TextureKind,
    cancelled: &mut impl FnMut() -> bool,
) -> bool {
    let mut offset = 0;
    while width > 1 || height > 1 {
        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let next_offset = offset + width as usize * height as usize * 4;
        for y in 0..next_height {
            if cancelled() {
                return false;
            }
            let mut row_weights = [0.0; 2];
            for (dy, weight) in row_weights.iter_mut().enumerate() {
                let row = (y * 2 + dy as u32).min(height - 1);
                *weight = ((PI * row as f64 / height as f64).cos()
                    - (PI * (row + 1) as f64 / height as f64).cos())
                    as f32;
            }
            let total_weight = 2.0 * row_weights.iter().sum::<f32>();
            for x in 0..next_width {
                let mut value = [0.0_f32; 4];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = (x * 2 + dx).min(width - 1);
                        let sy = (y * 2 + dy).min(height - 1);
                        let source = offset + (sy * width + sx) as usize * 4;
                        let weight = row_weights[dy as usize] / total_weight;
                        for channel in 0..4 {
                            let mut sample = pixels[source + channel] as f32 / 255.0;
                            if matches!(kind, TextureKind::Color) && channel < 3 {
                                sample = srgb_to_linear(sample);
                            }
                            value[channel] += sample * weight;
                        }
                    }
                }
                match kind {
                    TextureKind::Color => {
                        for channel in &mut value[..3] {
                            *channel = linear_to_srgb(*channel);
                        }
                    }
                    TextureKind::Normal => {
                        let normal = DVec3::new(
                            value[0] as f64 * 2.0 - 1.0,
                            value[1] as f64 * 2.0 - 1.0,
                            value[2] as f64 * 2.0 - 1.0,
                        )
                        .try_normalize()
                        .unwrap_or(DVec3::Z);
                        for channel in 0..3 {
                            value[channel] = (normal[channel] * 0.5 + 0.5) as f32;
                        }
                    }
                    TextureKind::Material => {}
                }
                let destination = next_offset + (y * next_width + x) as usize * 4;
                pixels[destination..destination + 4].copy_from_slice(&value.map(to_byte));
            }
        }

        offset = next_offset;
        width = next_width;
        height = next_height;
    }
    true
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        value.powf(1.0 / 2.4) * 1.055 - 0.055
    }
}
