use glam::{DQuat, DVec3};
use toy_sim_ui::egui;

pub(super) struct Camera {
    pub center: DVec3,
    target: DVec3,
    pub scale: f64,
    target_scale: f64,
    yaw: f64,
    pitch: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: DVec3::ZERO,
            target: DVec3::ZERO,
            scale: 0.,
            target_scale: 0.,
            yaw: 0.45,
            pitch: 0.35,
        }
    }
}

impl Camera {
    fn rotation(&self) -> DQuat {
        DQuat::from_rotation_x(self.pitch) * DQuat::from_rotation_y(self.yaw)
    }

    pub fn project(&self, point: DVec3, rect: egui::Rect) -> (egui::Pos2, f64) {
        let view = self.rotation() * (point - self.center);
        (
            rect.center() + egui::vec2((view.x * self.scale) as f32, (-view.y * self.scale) as f32),
            view.z,
        )
    }

    pub fn fit(&mut self, points: impl Iterator<Item = DVec3>, rect: egui::Rect) {
        let rotation = self.rotation();
        let mut lo = DVec3::splat(f64::INFINITY);
        let mut hi = DVec3::splat(f64::NEG_INFINITY);
        for point in points {
            let view = rotation * point;
            lo = lo.min(view);
            hi = hi.max(view);
        }
        if !lo.is_finite() {
            lo = DVec3::splat(-1.);
            hi = DVec3::ONE;
        }
        self.target = rotation.inverse() * ((lo + hi) * 0.5);
        let span = (hi - lo).max(DVec3::ONE);
        self.target_scale = ((rect.width() as f64 - 60.).max(1.) / span.x)
            .min((rect.height() as f64 - 60.).max(1.) / span.y)
            .clamp(0.01, 1000.);
        if self.scale == 0. {
            self.center = self.target;
            self.scale = self.target_scale;
        }
    }

    pub fn focus(&mut self, point: DVec3) {
        self.target = point;
        self.target_scale = self.target_scale.max(20.);
    }

    pub fn zoom_at(&mut self, factor: f64, offset: egui::Vec2) {
        let old = self.scale;
        self.scale = (old * factor).clamp(0.01, 1000.);
        self.target_scale = self.scale;
        self.center += self.rotation().inverse()
            * DVec3::new(offset.x as f64, -offset.y as f64, 0.)
            * (1. / old - 1. / self.scale);
        self.target = self.center;
    }

    pub fn update(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let dt = ui.input(|input| input.stable_dt).min(0.1) as f64;
        let blend = 1. - (-dt / 0.12).exp();
        self.center = self.center.lerp(self.target, blend);
        self.scale += (self.target_scale - self.scale) * blend;
        if self.center.distance(self.target) * self.scale > 0.1
            || (self.scale - self.target_scale).abs() > 0.001
        {
            ui.ctx().request_repaint();
        }

        let delta = ui.input(|input| input.pointer.delta());
        let right = response.dragged_by(egui::PointerButton::Secondary);
        let pan = response.dragged_by(egui::PointerButton::Middle)
            || (right && ui.input(|input| input.modifiers.shift));
        if pan {
            self.center -= self.rotation().inverse()
                * DVec3::new(delta.x as f64, -delta.y as f64, 0.)
                / self.scale;
            self.target = self.center;
        } else if right {
            self.yaw += delta.x as f64 * 0.006;
            self.pitch = (self.pitch + delta.y as f64 * 0.006).clamp(-1.55, 1.55);
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y) as f64;
            if scroll != 0. {
                if let Some(pointer) = response.hover_pos() {
                    self.zoom_at((scroll * 0.003).exp(), pointer - response.rect.center());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_reveals_depth_and_zoom_keeps_the_point_under_the_cursor() {
        let rect = egui::Rect::from_min_size(egui::pos2(120., 80.), egui::vec2(600., 400.));
        let mut camera = Camera::default();
        camera.fit([DVec3::splat(-10.), DVec3::splat(10.)].into_iter(), rect);
        let point = DVec3::new(3., 4., 7.);
        let before = camera.project(point, rect).0;
        camera.zoom_at(2., before - rect.center());
        assert!(camera.project(point, rect).0.distance(before) < 0.001);
        camera.yaw += 0.7;
        assert!(camera.project(point, rect).0.distance(before) > 1.);
        let shallow = camera.project(DVec3::new(1., 2., -5.), rect).0;
        let deep = camera.project(DVec3::new(1., 2., 5.), rect).0;
        assert!(shallow.distance(deep) > 1.);
    }
}
