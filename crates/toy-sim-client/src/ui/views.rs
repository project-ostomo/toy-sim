use super::{SelectedTarget, Selection, celestials, instruments};
use crate::state::*;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use toy_sim_model::*;

#[derive(Resource)]
pub(super) struct ViewControls {
    zoom: f64,
}

impl Default for ViewControls {
    fn default() -> Self {
        Self { zoom: 1000. }
    }
}

pub(super) fn window(
    mut contexts: EguiContexts,
    mut client: ResMut<ViewControls>,
    mut selection: ResMut<Selection>,
    views: Query<&ViewObservation>,
    contacts: Query<(&Contact, &DisplayPose)>,
    system_loads: Query<(&ViewObservation, &celestials::SystemLoadStatus)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let viewport = ctx.content_rect();
    let sidebar = (viewport.width() * 0.33).clamp(220., 360.);
    egui::Window::new("Views")
        .default_pos(egui::pos2(sidebar + 24., viewport.height() * 0.6))
        .default_size(egui::vec2(
            (viewport.width() - sidebar - 40.).max(200.),
            viewport.height() * 0.3,
        ))
        .max_height(viewport.height() * 0.35)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.add(
                egui::Slider::new(&mut client.zoom, 10. ..=1e20)
                    .logarithmic(true)
                    .text("View radius (m)"),
            );
            let mut ordered_views: Vec<_> = views.iter().map(|view| &view.0).collect();
            ordered_views.sort_by_key(|view| view.id);
            for view in ordered_views {
                ui.label(format!("View {} · {:?}", view.id, view.completion));
                if let Some((_, status)) = system_loads
                    .iter()
                    .find(|(observation, _)| observation.0.id == view.id)
                {
                    if !status.pending.is_empty() {
                        ui.label(format!(
                            "Loading celestial data for {} systems…",
                            status.pending.len()
                        ));
                    }
                    for (system, error) in &status.failed {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("System {system}: {error}"),
                        );
                    }
                }
                egui::ComboBox::from_id_salt(("contacts", view.id))
                    .selected_text("Select contact")
                    .show_ui(ui, |ui| {
                        for (_, track, _, label) in instruments::contact_rows(
                            contacts.iter().filter(|(contact, _)| {
                                contact.1.group == view.group
                                    && view.tracks.contains(&contact.1.track)
                            }),
                            "",
                            false,
                        ) {
                            ui.push_id((view.group, track.id), |ui| {
                                if ui
                                    .selectable_label(
                                        selection.contact()
                                            == Some(ContactRef {
                                                group: view.group,
                                                track: track.id,
                                            }),
                                        label,
                                    )
                                    .clicked()
                                {
                                    selection.target = Some(SelectedTarget::Contact(ContactRef {
                                        group: view.group,
                                        track: track.id,
                                    }));
                                    selection.view = Some(view.id);
                                }
                            });
                        }
                    });
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(
                        ui.available_width(),
                        (viewport.height() * 0.22 / views.iter().count().max(1) as f32)
                            .clamp(80., 220.),
                    ),
                    egui::Sense::drag(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0., egui::Color32::from_rgb(3, 7, 14));
                let origin = view
                    .focused_ship
                    .and_then(|ship| {
                        contacts
                            .iter()
                            .find(|(contact, _)| {
                                contact.1.group == view.group && contact.0.entity == Some(ship)
                            })
                            .map(|(_, pose)| pose.0.position)
                    })
                    .unwrap_or(view.origin);
                let scale = rect.width().min(rect.height()) as f64 / (2. * client.zoom);
                for id in &view.tracks {
                    if let Some((_, display)) = contacts.iter().find(|(contact, _)| {
                        contact.1.group == view.group
                            && contact.1.track == *id
                            && instruments::is_ship_contact(&contact.0)
                    }) {
                        let pose = &display.0;
                        let offset = pose.position.relative_to(origin);
                        let point = rect.center()
                            + egui::vec2((offset.x * scale) as f32, (offset.z * scale) as f32);
                        painter.circle_filled(point, 4., egui::Color32::LIGHT_GREEN);
                    }
                }
            }
        });
    Ok(())
}
