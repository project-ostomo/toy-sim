use bevy::math::{DQuat, DVec3};
use toy_sim_ui::egui;

pub(super) struct View {
    pub(super) rect: egui::Rect,
    pub(super) eye: DVec3,
    pub(super) rotation: DQuat,
    pub(super) sx: f64,
    pub(super) sy: f64,
}
impl View {
    pub(super) fn camera(&self, p: DVec3) -> DVec3 {
        self.rotation * (p - self.eye)
    }
    pub(super) fn screen(&self, p: DVec3) -> egui::Pos2 {
        self.rect.center()
            + egui::vec2(
                (p.x / (-p.z) * self.sx * self.rect.width() as f64 * 0.5) as f32,
                (-p.y / (-p.z) * self.sy * self.rect.height() as f64 * 0.5) as f32,
            )
    }
    pub(super) fn point(&self, p: DVec3) -> Option<egui::Pos2> {
        let p = self.camera(p);
        if p.z >= -0.1 {
            None
        } else {
            Some(self.screen(p))
        }
    }
    pub(super) fn segment(&self, a: DVec3, b: DVec3) -> Option<[egui::Pos2; 2]> {
        let (mut a, mut b) = (self.camera(a), self.camera(b));
        if a.z >= -0.1 && b.z >= -0.1 {
            return None;
        }
        if a.z > -0.1 {
            a = a.lerp(b, (-0.1 - a.z) / (b.z - a.z));
        }
        if b.z > -0.1 {
            b = b.lerp(a, (-0.1 - b.z) / (a.z - b.z));
        }
        // Clip in homogeneous view space before f32 conversion, even with enormous anchors.
        for (axis, sign, scale) in [
            (0, 1., self.sx),
            (0, -1., self.sx),
            (1, 1., self.sy),
            (1, -1., self.sy),
        ] {
            let fa = -a.z + sign * a[axis] * scale;
            let fb = -b.z + sign * b[axis] * scale;
            if fa < 0. && fb < 0. {
                return None;
            }
            if fa < 0. {
                a = a.lerp(b, fa / (fa - fb));
            } else if fb < 0. {
                b = b.lerp(a, fb / (fb - fa));
            }
        }
        let out = [self.screen(a), self.screen(b)];
        out.iter().all(|p| p.is_finite()).then_some(out)
    }
    pub(super) fn marker(&self, p: DVec3) -> (egui::Pos2, bool) {
        if let Some(p) = self.point(p).filter(|p| self.rect.shrink(16.).contains(*p)) {
            return (p, false);
        }
        let p = self.camera(p);
        let direction = bevy::math::DVec2::new(
            p.x * self.sx * self.rect.width() as f64,
            -p.y * self.sy * self.rect.height() as f64,
        )
        .try_normalize()
        .unwrap_or(bevy::math::DVec2::NEG_Y);
        let direction = egui::vec2(direction.x as f32, direction.y as f32);
        let half = (self.rect.size() * 0.5 - egui::vec2(22., 22.)).max(egui::vec2(1., 1.));
        let factor =
            (half.x / direction.x.abs().max(1e-8)).min(half.y / direction.y.abs().max(1e-8));
        (self.rect.center() + direction * factor, true)
    }
}
pub(super) fn occluded(eye: DVec3, p: DVec3, bodies: &[(DVec3, f64)]) -> bool {
    let ray = p - eye;
    let distance = ray.length();
    if distance <= 0. {
        return false;
    }
    let dir = ray / distance;
    bodies.iter().any(|(center, radius)| {
        let to_center = *center - eye;
        let along = to_center.dot(dir);
        if along <= 0. || to_center.length_squared() <= radius * radius {
            return false;
        }
        let perpendicular = to_center.cross(dir).length_squared();
        perpendicular < radius * radius
            && along - (radius * radius - perpendicular).sqrt() < distance - 1.
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> View {
        View {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 720.)),
            eye: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            sx: 1.,
            sy: 1280. / 720.,
        }
    }
    #[test]
    fn offscreen_markers_stay_inside_the_view_and_point_toward_the_target() {
        let view = view();
        let (center, offscreen) = view.marker(DVec3::new(0., 0., -10.));
        assert_eq!(center, view.rect.center());
        assert!(!offscreen);

        for point in [DVec3::new(100., 20., -1.), DVec3::new(100., 20., 1.)] {
            let (marker, offscreen) = view.marker(point);
            assert!(offscreen && view.rect.shrink(20.).contains(marker));
            let direction = marker - view.rect.center();
            assert!(direction.x > 0. && direction.y < 0.);
            assert!((direction.y / direction.x + 0.2).abs() < 1e-5);
        }
        let (behind, offscreen) = view.marker(DVec3::Z);
        assert!(offscreen && behind.is_finite());
        assert!(view.rect.shrink(20.).contains(behind));
    }

    #[test]
    fn clips_near_plane_and_viewport_before_float_conversion() {
        let view = view();
        assert!(view.segment(DVec3::Z, DVec3::Z * 2.).is_none());
        let line = view
            .segment(DVec3::new(-1e30, 0., -100.), DVec3::new(1e30, 0., -100.))
            .unwrap();
        assert!(line.iter().all(|p| view.rect.expand(0.01).contains(*p)));
        assert!(
            view.segment(DVec3::new(0., 0., 1.), DVec3::new(0., 0., -100.))
                .is_some()
        );
        assert!(
            view.segment(DVec3::new(1000., 0., -100.), DVec3::new(2000., 0., -100.))
                .is_none()
        );
    }
    #[test]
    fn occlusion_tests_depth_and_not_just_angular_overlap() {
        let bodies = vec![(DVec3::new(0., 0., -10.), 2.)];
        assert!(occluded(DVec3::ZERO, DVec3::new(0., 0., -20.), &bodies));
        assert!(!occluded(DVec3::ZERO, DVec3::new(0., 0., -5.), &bodies));
        assert!(!occluded(DVec3::ZERO, DVec3::new(10., 0., -20.), &bodies));
    }
    #[test]
    fn projection_is_independent_of_floating_origin_and_dpi() {
        let anchor = toy_sim_model::GalacticPosition::new(1_i128 << 100, -(1_i128 << 98), 0);
        let eye = anchor.offset_by(DVec3::new(100., 200., 300.));
        let point = anchor.offset_by(DVec3::new(101., 202., 200.));
        let mut v = view();
        v.eye = eye.relative_to(anchor);
        let a = v.point(point.relative_to(anchor)).unwrap();
        let shifted = anchor.offset_by(DVec3::splat(1e6));
        v.eye = eye.relative_to(shifted);
        let b = v.point(point.relative_to(shifted)).unwrap();
        assert_eq!(a, b);
        // View is measured in egui points; doubling physical resolution/DPI leaves it identical.
        let physical = egui::vec2(2560., 1440.);
        assert_eq!(physical / 2., v.rect.size());
    }
}
