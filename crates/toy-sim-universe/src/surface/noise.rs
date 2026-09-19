use glam::DVec3;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha20Rng;

pub(super) fn random(seed: &[u8; 32], domain: &str) -> ChaCha20Rng {
    ChaCha20Rng::from_seed(crate::generation::seed(domain, seed))
}

pub(super) struct Noise {
    lattice: [u8; 256],
    offset: DVec3,
}

impl Noise {
    pub fn new(seed: &[u8; 32], domain: &str) -> Self {
        let mut random = random(seed, domain);
        let mut lattice = std::array::from_fn(|index| index as u8);
        for index in (1..256).rev() {
            lattice.swap(index, random.random_range(0..=index));
        }
        Self {
            lattice,
            offset: DVec3::new(
                random.random_range(0.0..256.0),
                random.random_range(0.0..256.0),
                random.random_range(0.0..256.0),
            ),
        }
    }

    fn value(&self, point: DVec3) -> f64 {
        let floor = point.floor();
        let fraction = point - floor;
        let fade = |v: f64| v * v * v * (v * (v * 6.0 - 15.0) + 10.0);
        let u = fraction.map(fade);
        let cell = floor.as_ivec3();
        let value = |dx: i32, dy: i32, dz: i32| {
            let x = self.lattice[((cell.x + dx) & 255) as usize] as i32;
            let y = self.lattice[((cell.y + dy + x) & 255) as usize] as i32;
            let z = self.lattice[((cell.z + dz + y) & 255) as usize];
            z as f64 / 127.5 - 1.0
        };
        let a = lerp(value(0, 0, 0), value(1, 0, 0), u.x);
        let b = lerp(value(0, 1, 0), value(1, 1, 0), u.x);
        let c = lerp(value(0, 0, 1), value(1, 0, 1), u.x);
        let d = lerp(value(0, 1, 1), value(1, 1, 1), u.x);
        lerp(lerp(a, b, u.y), lerp(c, d, u.y), u.z)
    }

    pub fn fractal(&self, mut point: DVec3, octaves: u32, mut footprint: f64) -> f64 {
        let mut value = 0.0;
        let mut amplitude = 0.5;
        for _ in 0..octaves {
            let weight = 1.0 - smooth(0.35, 0.9, footprint);
            if weight == 0.0 {
                break;
            }
            value += self.value(point + self.offset) * amplitude * weight;
            point = point * 2.03 + DVec3::new(19.3, 7.7, 11.1);
            footprint *= 2.03;
            amplitude *= 0.5;
        }
        value
    }
}

pub(super) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub(super) fn mix(a: [f32; 3], b: [f32; 3], t: f64) -> [f32; 3] {
    std::array::from_fn(|index| a[index] + (b[index] - a[index]) * t as f32)
}

pub(super) fn smooth(a: f64, b: f64, value: f64) -> f64 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
