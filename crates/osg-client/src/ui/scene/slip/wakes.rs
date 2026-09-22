use super::*;
use bevy::math::DVec3;
use osg_model::GalacticPosition;

pub(super) const WAKE_COUNT: usize = 96;

#[derive(Clone, Copy, Debug, Default, PartialEq, bevy::render::render_resource::ShaderType)]
pub(super) struct Ribbon {
    // Target pixels, reverse depth, Gaussian width in pixels.
    pub start: Vec4,
    pub end: Vec4,
    pub color_start: Vec4,
    pub color_end: Vec4,
}

pub(super) fn own_wake(
    ship: &OwnedShip,
    pose: &DisplayPose,
    state: &SlipView,
    view: u64,
    now: u64,
) -> Option<SlipWake> {
    if !matches!(ship.0.presence, Presence::SlipTransit(_)) {
        return None;
    }
    let speed = DVec3::from_array(pose.0.velocity).length().max(1.0);
    let age = now.saturating_sub(state.departure_ns.unwrap_or(now)) as f64 * 1e-9;
    let length = (speed * age).min(WAKE_VIEW_RANGE_M);
    Some(SlipWake {
        view,
        id: ship.0.ship,
        start: pose
            .0
            .position
            .offset_by(-state.direction.as_dvec3() * length),
        end: pose.0.position,
        start_ns: now.saturating_sub((length / speed * 1e9) as u64),
        end_ns: now,
        drift_m_s: [0.0; 3],
        radius_m: ship.0.radius_m,
        seed: 451,
        offset_m: 0.0,
    })
}

/// Clip and project in double precision before uploading bounded pixel
/// coordinates. No large world positions or ray/cylinder intersections reach
/// the GPU. Radii follow perspective continuously, including subpixel widths.
pub(super) fn project(
    wake: &SlipWake,
    origin: GalacticPosition,
    camera: &Transform,
    near: f64,
    tan: f64,
    viewport: Vec4,
    screen: Vec2,
    now: u64,
    exposure: f32,
) -> Option<Ribbon> {
    if wake.start_ns > now || wake.end_ns <= wake.start_ns {
        return None;
    }
    let duration = (wake.end_ns - wake.start_ns) as f64;
    let formed = ((now - wake.start_ns) as f64 / duration).min(1.0);
    let rotation = camera.rotation.as_dquat().inverse();
    let local = |f| {
        let p =
            rotation * (wake.position(f, now).relative_to(origin) - camera.translation.as_dvec3());
        DVec3::new(p.x, p.y, -p.z)
    };
    let a = local(0.0);
    let b = local(formed);
    if a.distance_squared(b) < 1e-12 {
        return None;
    }
    let age = |f: f64| ((now - wake.start_ns) as f64 - duration * f).max(0.0) * 1e-9;
    let width = |f| wake.radius_m.max(8.0) * (1.4 + age(f).sqrt() * 0.2);
    let aspect = (screen.x * viewport.z / (screen.y * viewport.w)) as f64;
    let sx = 1.0 / (tan * aspect);
    let sy = 1.0 / tan;
    let radius = width(0.0).max(width(formed));
    let mut lo: f64 = 0.0;
    let mut hi: f64 = 1.0;
    for (normal, margin) in [
        (DVec3::Z, -near),
        (DVec3::new(sx, 0.0, 1.0), radius * sx * 3.0),
        (DVec3::new(-sx, 0.0, 1.0), radius * sx * 3.0),
        (DVec3::new(0.0, sy, 1.0), radius * sy * 3.0),
        (DVec3::new(0.0, -sy, 1.0), radius * sy * 3.0),
    ] {
        let da = a.dot(normal) + margin;
        let db = b.dot(normal) + margin;
        if da < 0.0 && db < 0.0 {
            return None;
        }
        if da < 0.0 {
            lo = lo.max(da / (da - db));
        } else if db < 0.0 {
            hi = hi.min(da / (da - db));
        }
    }
    if hi <= lo {
        return None;
    }
    let endpoint = |fraction: f64| {
        let p = a.lerp(b, fraction);
        let depth = p.z.max(near);
        let f = fraction * formed;
        let position = Vec4::new(
            ((viewport.x as f64 + (0.5 + p.x * sx / depth * 0.5) * viewport.z as f64)
                * screen.x as f64) as f32,
            ((viewport.y as f64 + (0.5 - p.y * sy / depth * 0.5) * viewport.w as f64)
                * screen.y as f64) as f32,
            (near / depth) as f32,
            (width(f) * sy / depth * 0.5 * f64::from(screen.y * viewport.w)) as f32,
        );
        let age = age(f);
        let tint = Vec3::new(0.10, 0.32, 0.65)
            + Vec3::new(0.5, 0.75, 1.0) * ((-age / 0.18).exp() as f32 * 2.0);
        let color = (tint * wake_envelope(age) as f32 * exposure * 35000.0 * 0.35).extend(0.0);
        (position, color)
    };
    let (start, color_start) = endpoint(lo);
    let (end, color_end) = endpoint(hi);
    (start.is_finite() && end.is_finite()).then_some(Ribbon {
        start,
        end,
        color_start,
        color_end,
    })
}

pub(super) fn prepare(
    settings: &mut distortion::Distortion,
    wakes: impl Iterator<Item = SlipWake>,
    view: &ViewCamera,
    camera: &Transform,
    projection: &PerspectiveProjection,
    now: u64,
    exposure: f32,
) {
    let mut ribbons: Vec<_> = wakes
        .filter(|wake| wake.view == view.view)
        .filter_map(|wake| {
            project(
                &wake,
                view.origin,
                camera,
                projection.near as f64,
                (projection.fov as f64 * 0.5).tan(),
                settings.viewport,
                settings.screen.truncate().truncate(),
                now,
                exposure,
            )
        })
        .collect();
    // Preserve the strongest contributions if a crowded view exceeds the
    // fixed uniform budget (kept below the 16 KiB portable binding limit).
    ribbons.sort_by(|a, b| {
        let power =
            |r: &Ribbon| (r.start.w + r.end.w) * (r.color_start.length() + r.color_end.length());
        power(b).total_cmp(&power(a))
    });
    let count = ribbons.len().min(WAKE_COUNT);
    settings.wakes[..count].copy_from_slice(&ribbons[..count]);
    settings.screen.w = count as f32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_preserves_subpixel_tails_and_large_origin_translation() {
        use bevy::render::render_resource::ShaderType;
        assert!(distortion::Distortion::min_size().get() <= 16 * 1024);
        let mut wake = super::super::tests::wake(60.0);
        let camera = Transform::from_xyz(1e8, 4e7, 1e8).looking_at(Vec3::ZERO, Vec3::Y);
        let project_at = |wake: &SlipWake, origin| {
            project(
                wake,
                origin,
                &camera,
                0.1,
                0.5,
                Vec4::new(0.0, 0.0, 1.0, 1.0),
                Vec2::new(1280.0, 720.0),
                400_000_000_000,
                1e-4,
            )
            .unwrap()
        };
        let before = project_at(&wake, GalacticPosition::ZERO);
        assert!(before.start.w > 0.0 && before.start.w < 0.01);
        let shift = DVec3::new(1e17, -2e17, 3e17);
        wake.start = wake.start.offset_by(shift);
        wake.end = wake.end.offset_by(shift);
        assert_eq!(
            before,
            project_at(&wake, GalacticPosition::ZERO.offset_by(shift))
        );
    }

    #[test]
    fn end_on_and_near_plane_crossings_remain_finite() {
        let mut wake = super::super::tests::wake(0.1);
        let project_at = |wake: &SlipWake| {
            project(
                wake,
                GalacticPosition::ZERO,
                &Transform::default(),
                0.1,
                0.5,
                Vec4::new(0.0, 0.0, 1.0, 1.0),
                Vec2::new(1280.0, 720.0),
                400_000_000_000,
                1e-4,
            )
        };
        wake.start = GalacticPosition::ZERO.offset_by(DVec3::Z * 100.0);
        let ribbon = project_at(&wake).unwrap();
        assert!(ribbon.start.is_finite() && ribbon.end.is_finite());
        assert_eq!(
            ribbon.start.truncate().truncate(),
            ribbon.end.truncate().truncate()
        );
        wake.end = GalacticPosition::ZERO.offset_by(DVec3::Z * 200.0);
        assert!(project_at(&wake).is_none());
    }
}
