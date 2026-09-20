//! Client-owned presentations of the fixed attitude contract. Firmware supplies no UI.
use bevy::math::{DQuat, EulerRot};
use bevy_egui::egui;
use osg_ship_api::abi::{ATTITUDE_REFERENCE, AttitudeState, FlightState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttitudePresentation {
    Panel,
    Hud,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttitudeReading {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
    pub reference_error: Option<f64>,
}
pub fn attitude_reading(
    observation: &FlightState,
    instrument: &AttitudeState,
    control_frame: DQuat,
) -> AttitudeReading {
    let q = DQuat::from_array(observation.rotation).normalize();
    let (yaw, pitch, roll) = (q * control_frame).to_euler(EulerRot::YXZ);
    AttitudeReading {
        yaw,
        pitch,
        roll,
        reference_error: (instrument.present & ATTITUDE_REFERENCE != 0)
            .then(|| q.angle_between(DQuat::from_array(instrument.reference))),
    }
}
pub fn attitude(
    ui: &mut egui::Ui,
    observation: &FlightState,
    instrument: &AttitudeState,
    presentation: AttitudePresentation,
    control_frame: DQuat,
) {
    let v = attitude_reading(observation, instrument, control_frame);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(300., 155.), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);
    let color = match presentation {
        AttitudePresentation::Panel => egui::Color32::WHITE,
        AttitudePresentation::Hud => egui::Color32::LIGHT_GREEN,
    };
    if presentation == AttitudePresentation::Panel {
        painter.rect_filled(rect, 6., egui::Color32::from_rgb(16, 26, 42));
    }
    let centre = rect.center();
    let right = egui::vec2(v.roll.cos() as f32, -v.roll.sin() as f32);
    let up = egui::vec2(-right.y, right.x);
    let horizon = centre + up * (v.pitch.to_degrees() as f32 * 1.5);
    for degrees in [-30, -20, -10, 0, 10, 20, 30] {
        let at = horizon - up * degrees as f32 * 1.5;
        let half = if degrees == 0 { 100. } else { 35. };
        painter.line_segment(
            [at - right * half, at + right * half],
            egui::Stroke::new(1., color),
        );
    }
    painter.line_segment(
        [centre - egui::vec2(20., 0.), centre - egui::vec2(5., 0.)],
        egui::Stroke::new(2., color),
    );
    painter.line_segment(
        [centre + egui::vec2(5., 0.), centre + egui::vec2(20., 0.)],
        egui::Stroke::new(2., color),
    );
    painter.circle_stroke(centre, 3., egui::Stroke::new(1., color));
    painter.text(
        rect.left_top() + egui::vec2(8., 6.),
        egui::Align2::LEFT_TOP,
        format!(
            "HDG {:06.1}°   P {:+.1}°   R {:+.1}°",
            v.yaw.to_degrees().rem_euclid(360.),
            v.pitch.to_degrees(),
            v.roll.to_degrees()
        ),
        egui::FontId::monospace(12.),
        color,
    );
    painter.text(
        rect.left_bottom() + egui::vec2(8., -6.),
        egui::Align2::LEFT_BOTTOM,
        format!(
            "{}  ·  reference {}",
            ["MANUAL", "HOLD", "GUIDANCE"][instrument.mode.min(2) as usize],
            v.reference_error
                .map_or("—".into(), |e| format!("{:.2}°", e.to_degrees()))
        ),
        egui::FontId::monospace(12.),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_instrument_supports_two_presentations_without_changing_state() {
        let o = FlightState {
            rotation: DQuat::from_rotation_x(0.2).to_array(),
            ..Default::default()
        };
        let instrument = AttitudeState {
            present: ATTITUDE_REFERENCE,
            reference: DQuat::IDENTITY.to_array(),
            ..Default::default()
        };
        let before = attitude_reading(&o, &instrument, DQuat::IDENTITY);
        let ctx = egui::Context::default();
        for style in [AttitudePresentation::Panel, AttitudePresentation::Hud] {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                attitude(ui, &o, &instrument, style, DQuat::IDENTITY);
            });
            assert!(!output.shapes.is_empty());
            output.textures_delta.clear();
            assert_eq!(before, attitude_reading(&o, &instrument, DQuat::IDENTITY));
        }
        assert!((before.reference_error.unwrap() - 0.2).abs() < 1e-9);
    }
}
