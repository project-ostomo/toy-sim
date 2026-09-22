use super::distortion::{Distortion, STREAK_COUNT, Streak};
use bevy::prelude::*;

fn random(index: u32, event: u32, channel: u32) -> f32 {
    let mut bits = index.wrapping_mul(0x9e3779b9)
        ^ event.wrapping_mul(0x85ebca6b)
        ^ channel.wrapping_mul(0xc2b2ae35);
    bits ^= bits >> 16;
    bits = bits.wrapping_mul(0x7feb352d);
    bits ^= bits >> 15;
    bits = bits.wrapping_mul(0x846ca68b);
    bits ^= bits >> 16;
    (bits >> 8) as f32 / 16_777_216.0
}

// Clip a homogeneous segment, including an endpoint at infinity. Interpolating
// its reverse depth after division gives the correct depth along the screen line.
fn clip(mut a: Vec4, mut b: Vec4) -> Option<(Vec3, Vec3)> {
    for plane in [
        Vec4::new(0.0, 0.0, -1.0, 1.0),
        Vec4::new(1.0, 0.0, 0.0, 1.0),
        Vec4::new(-1.0, 0.0, 0.0, 1.0),
        Vec4::new(0.0, 1.0, 0.0, 1.0),
        Vec4::new(0.0, -1.0, 0.0, 1.0),
    ] {
        let da = a.dot(plane);
        let db = b.dot(plane);
        if da < 0.0 && db < 0.0 {
            return None;
        }
        if da < 0.0 {
            a = a.lerp(b, da / (da - db));
        } else if db < 0.0 {
            b = b.lerp(a, db / (db - da));
        }
    }
    if a.w <= 1e-8 || b.w <= 1e-8 {
        return None;
    }
    let endpoints = (a.truncate() / a.w, b.truncate() / b.w);
    (endpoints.0.is_finite() && endpoints.1.is_finite()).then_some(endpoints)
}

pub(super) fn prepare(settings: &mut Distortion, flow: f64) {
    let right = settings.right.truncate();
    let up = settings.up.truncate();
    let forward = settings.forward.truncate();
    let project = |point: Vec3, near: f32| {
        Vec4::new(
            point.dot(right) / right.length_squared(),
            point.dot(up) / up.length_squared(),
            near,
            point.dot(forward),
        )
    };
    let tail = project(Vec3::Z, 0.0);
    let viewport = settings.viewport;
    let to_target = |point: Vec3| {
        let uv = Vec2::new(point.x * 0.5 + 0.5, 0.5 - point.y * 0.5);
        let uv = viewport.truncate().truncate() + uv * Vec2::new(viewport.z, viewport.w);
        Vec3::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, point.z)
    };
    let coverage = settings.time.y.clamp(0.0, 1.0);
    let coverage = coverage * coverage * (3.0 - 2.0 * coverage);
    let mut count = 0;
    for index in 0..STREAK_COUNT as u32 {
        let interval = 5.0 + random(index, 0, 1) as f64 * 11.0;
        let clock = flow / interval + random(index, 0, 2) as f64;
        let event = clock.floor() as u32;
        let age = clock.fract() as f32;
        let lifetime = 0.65 + random(index, event, 3) * 0.3;
        let fade = (age / 0.06).clamp(0.0, 1.0) * ((lifetime - age) / 0.25).clamp(0.0, 1.0);
        if fade <= 0.0 {
            continue;
        }
        let angle = random(index, event, 4) * std::f32::consts::TAU;
        let radius = 100.0 + random(index, event, 5).powi(2) * 4500.0;
        let head = Vec3::new(
            angle.cos() * radius,
            angle.sin() * radius,
            random(index, event, 6) * 10_000.0 - age * interval as f32 * 5000.0,
        );
        let Some((a, b)) = clip(
            project(head - settings.eye.truncate(), settings.eye.w),
            tail,
        ) else {
            continue;
        };
        let a = to_target(a);
        let b = to_target(b);
        let pixels = (b - a).truncate() * settings.screen.truncate().truncate() * 0.5;
        let length = pixels.length();
        if length < 1.0 {
            continue;
        }
        let brightness =
            fade * coverage * (0.7 + random(index, event, 7) * 1.3) * settings.forward.w * 0.8;
        settings.streaks[count] = Streak {
            start: a.extend(0.0),
            end: b.extend(length),
            color: (Vec3::new(0.96, 0.91, 0.82) * brightness).extend(0.5),
        };
        count += 1;
    }
    settings.screen.z = count as f32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infinite_streaks_clip_at_the_near_plane_and_keep_reverse_depth() {
        let (a, b) = clip(Vec4::new(0.0, 0.0, 0.1, -1.0), Vec4::W).unwrap();
        assert!((a.z - 1.0).abs() < 1e-5);
        assert_eq!(b, Vec3::ZERO);
        assert!(clip(Vec4::new(2.0, 0.0, 0.1, 1.0), Vec4::new(3.0, 0.0, 0.0, 1.0)).is_none());
        assert!(clip(Vec4::new(0.0, 0.0, 0.1, -1.0), -Vec4::W).is_none());
    }

    #[test]
    fn streaks_keep_the_vanishing_point_in_their_viewport_with_camera_parallax() {
        let mut settings = Distortion::default();
        settings.right = Vec4::X;
        settings.up = Vec4::Y;
        settings.forward = Vec4::new(0.0, 0.0, 1.0, 1.0);
        settings.eye.w = 0.1;
        settings.viewport = Vec4::new(0.5, 0.0, 0.5, 1.0);
        settings.screen = Vec4::new(1280.0, 720.0, 0.0, 0.0);
        settings.time.y = 1.0;
        prepare(&mut settings, 9.0);
        let count = settings.screen.z as usize;
        assert!(count > 0 && count <= STREAK_COUNT);
        for streak in &settings.streaks[..count] {
            assert!(streak.start.is_finite() && streak.end.is_finite());
            assert!(
                (-1e-5..=1.0 + 1e-5).contains(&streak.start.x),
                "{:?}",
                streak.start
            );
            assert!(
                (-1.0 - 1e-5..=1.0 + 1e-5).contains(&streak.start.y),
                "{:?}",
                streak.start
            );
            assert_eq!(streak.end.truncate(), Vec3::new(0.5, 0.0, 0.0));
        }
        let before = settings.streaks;
        settings.eye.x = 40.0;
        prepare(&mut settings, 9.0);
        assert!(
            settings
                .streaks
                .iter()
                .zip(before)
                .any(|(a, b)| a.start != b.start)
        );
        for streak in &settings.streaks[..settings.screen.z as usize] {
            assert_eq!(streak.end.truncate(), Vec3::new(0.5, 0.0, 0.0));
        }
    }
}
