use glam::DVec3;
use osg_universe::surface::{
    SurfaceGenerator, SurfaceParameters, SurfaceTextures, peak_work_bytes, texture_bytes,
};
use std::{fs::File, io::Write, time::Instant};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let path = arguments
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("/tmp/sequential-planet-surfaces.ppm");
    let configs = osg_universe::handcrafted_configs();
    let names = [
        "Earth",
        "Mars",
        "Helion I d Rime",
        "Jupiter",
        "Neptune",
        "Helion I Neris",
    ];
    let mut variants = Vec::new();
    for name in names {
        let body = configs
            .iter()
            .flat_map(|config| &config.bodies)
            .find(|body| body.name == name)
            .ok_or_else(|| anyhow::anyhow!("missing preview body {name}"))?;
        let parameters = SurfaceParameters::from_body(body)
            .ok_or_else(|| anyhow::anyhow!("missing surface metadata for {name}"))?;
        variants.push((name, parameters));
    }
    if arguments.iter().any(|arg| arg == "--benchmark") {
        for (name, parameters) in [&variants[0], &variants[3]] {
            let started = Instant::now();
            let generator = SurfaceGenerator::new(parameters.clone())?;
            println!(
                "{name}: initialization {:.2} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
            for width in [256, 512, 1024, 2048] {
                let started = Instant::now();
                let texture = generator.bake(width)?;
                println!(
                    "{name}: {width} bake {:.2} ms, textures {:.2} MiB, peak estimate {:.2} MiB, hash {}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    texture_bytes(width, parameters.has_clouds()) as f64 / 1048576.0,
                    peak_work_bytes(width, parameters.has_clouds()) as f64 / 1048576.0,
                    blake3::hash(&texture.color)
                );
            }
        }
    }

    let tile = 320;
    let width = tile * 3;
    let height = tile * 2;
    let mut image = vec![3_u8; width * height * 3];
    let light = DVec3::new(-0.8, 0.4, 1.0).normalize();
    for (index, (name, parameters)) in variants.into_iter().enumerate() {
        let texture = SurfaceGenerator::new(parameters)?.bake(512)?;
        println!("preview cell{index}: {name}");
        for y in 0..tile {
            for x in 0..tile {
                let px = (x as f64 + 0.5 - tile as f64 * 0.5) / (tile as f64 * 0.44);
                let py = -(y as f64 + 0.5 - tile as f64 * 0.5) / (tile as f64 * 0.44);
                if px * px + py * py > 1.0 {
                    continue;
                }
                let radial = DVec3::new(px, py, (1.0 - px * px - py * py).sqrt());
                let body_point = DVec3::new(radial.x, -radial.z, radial.y);
                let color = shade(&texture, body_point, radial, light);
                let pixel = (((index / 3) * tile + y) * width + (index % 3) * tile + x) * 3;
                for channel in 0..3 {
                    image[pixel + channel] = (linear_to_srgb(color[channel]) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    let mut file = File::create(path)?;
    write!(file, "P6\n{width} {height}\n255\n")?;
    file.write_all(&image)?;
    println!("{path}");
    Ok(())
}

fn shade(texture: &SurfaceTextures, point: DVec3, radial: DVec3, light: DVec3) -> [f32; 3] {
    let longitude = point.y.atan2(point.x);
    let u = (longitude / std::f64::consts::TAU).rem_euclid(1.0);
    let v = point.z.acos() / std::f64::consts::PI;
    let color = sample(&texture.color, texture.width, texture.height, u, v);
    let packed = sample(&texture.normal, texture.width, texture.height, u, v);
    let normal =
        DVec3::new(packed[0] as f64, packed[1] as f64, packed[2] as f64) * 2.0 - DVec3::ONE;
    let east = DVec3::new(-longitude.sin(), longitude.cos(), 0.0);
    let south = east.cross(point);
    let body_normal = (east * normal.x + south * normal.y + point * normal.z).normalize();
    let surface_normal = DVec3::new(body_normal.x, body_normal.z, -body_normal.y);
    let illumination = (surface_normal.dot(light).max(0.0) * 0.95 + 0.012) as f32;
    let mut radiance = std::array::from_fn(|channel| srgb_to_linear(color[channel]) * illumination);
    if let Some(clouds) = &texture.clouds {
        let cloud = sample(clouds, texture.width / 2, texture.height / 2, u, v);
        let illumination = (radial.dot(light).max(0.0) * 0.95 + 0.012) as f32;
        for channel in 0..3 {
            radiance[channel] = radiance[channel] * (1.0 - cloud[3])
                + srgb_to_linear(cloud[channel]) * illumination * cloud[3];
        }
    }
    radiance
}

fn sample(pixels: &[u8], width: u32, height: u32, u: f64, v: f64) -> [f32; 4] {
    let x = u * width as f64 - 0.5;
    let y = v * height as f64 - 0.5;
    let tx = x.fract().rem_euclid(1.0) as f32;
    let ty = y.fract().rem_euclid(1.0) as f32;
    let mut result = [0.0; 4];
    for dy in 0..2 {
        for dx in 0..2 {
            let sx = (x.floor() as i32 + dx).rem_euclid(width as i32) as u32;
            let sy = (y.floor() as i32 + dy).clamp(0, height as i32 - 1) as u32;
            let weight =
                (if dx == 0 { 1.0 - tx } else { tx }) * (if dy == 0 { 1.0 - ty } else { ty });
            let index = (sy * width + sx) as usize * 4;
            for channel in 0..4 {
                result[channel] += pixels[index + channel] as f32 / 255.0 * weight;
            }
        }
    }
    result
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        v.powf(1.0 / 2.4) * 1.055 - 0.055
    }
}
