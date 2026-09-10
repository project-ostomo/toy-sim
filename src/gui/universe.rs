use crate::orrery::{
    BodyClass, Universe,
    activity::{ActiveSystems, UniverseDebug},
};
use crate::starfield::{SkySettings, Starfield};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
pub fn window(
    mut contexts: EguiContexts,
    universe: Res<Universe>,
    active: Res<ActiveSystems>,
    mut debug: ResMut<UniverseDebug>,
    mut sky: ResMut<SkySettings>,
    field: Res<Starfield>,
    gaia: Option<Res<crate::gaia::GaiaCatalogue>>,
    mut matches: Local<Option<(String, usize, Vec<usize>)>>,
    mut page: Local<usize>,
) -> Result {
    egui::Window::new("Universe")
        .default_pos(egui::pos2(10.0, 370.0))
        .default_width(300.0)
        .show(contexts.ctx_mut()?, |ui| {
            ui.label(format!(
                "{} systems · {} active",
                universe.systems.len(),
                active.entities.len()
            ));
            ui.label(format!(
                "Sky: {} · {} stars · {} bakes",
                field.state, field.selected, field.generations
            ));
            ui.label(format!("Displayed: {} px/face · baking: {} px/face", field.resolution, field.baking_resolution));
            ui.label(format!("Last CPU level: {:.3} ms", field.last_seconds * 1000.0));
            ui.add(
                egui::Slider::new(&mut sky.0.magnitude_limit, -2.0..=16.0)
                    .text("Limiting magnitude"),
            );
            ui.add(
                egui::Slider::new(&mut sky.0.brightness, 0.01..=10000.0)
                    .logarithmic(true)
                    .text("Sky brightness"),
            );
            ui.small("CPU sky refines through 512 → 1024 → 2048 → 4096. Stale bakes are cancelled. Rotation reuses the cache. Sensors only see active bodies.");
            if let Some(gaia) = &gaia {
                ui.label(format!("Gaia: {}", gaia.state));
                ui.label(format!("{} candidates · {} buckets · {} resident stars", gaia.candidates, gaia.buckets_searched, gaia.total_stars));
                ui.label(format!("{} cached / {} matching · {:.1} ms query",gaia.selected,gaia.matched,gaia.seconds*1000.0));
            }
            ui.separator();
            ui.text_edit_singleline(&mut debug.search);
            let search = debug.search.to_lowercase();
            if matches
                .as_ref()
                .is_none_or(|(old, count, _)| old != &search || *count != universe.systems.len())
            {
                let ids = universe
                    .systems
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| {
                        search.is_empty()
                            || s.solver.name.to_lowercase().contains(&search)
                            || s.solver
                                .iter()
                                .any(|b| b.name.to_lowercase().contains(&search))
                    })
                    .map(|(i, _)| i)
                    .collect();
                *matches = Some((search, universe.systems.len(), ids));
                *page = 0;
            }
            let ids = &matches.as_ref().unwrap().2;
            let pages = ids.len().div_ceil(20).max(1);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(*page > 0, egui::Button::new("Previous"))
                    .clicked()
                {
                    *page -= 1;
                }
                ui.label(format!("{} / {}", *page + 1, pages));
                if ui
                    .add_enabled(*page + 1 < pages, egui::Button::new("Next"))
                    .clicked()
                {
                    *page += 1;
                }
            });
            egui::ScrollArea::vertical()
                .max_height(350.0)
                .show(ui, |ui| {
                    for &id in ids.iter().skip(*page * 20).take(20) {
                        let s = &universe.systems[id];
                        let reason = if active.ship_systems.contains(&id) {
                            "ships"
                        } else if active.entities.contains_key(&id) {
                            "inspection"
                        } else {
                            "inactive"
                        };
                        egui::CollapsingHeader::new(format!("{} ({reason})", s.solver.name))
                            .id_salt(id)
                            .show(ui, |ui| {
                                ui.small(format!(
                                    "Influence: {:.0} AU",
                                    s.influence / 1.495978707e11
                                ));
                                for body in s.solver.iter() {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .selectable_label(
                                                debug.inspect.as_ref() == Some(&body.name),
                                                body.name.as_str(),
                                            )
                                            .clicked()
                                        {
                                            debug.inspect = Some(body.name.clone());
                                            debug.follow_pending = true;
                                        }
                                        if !matches!(body.class_params, BodyClass::Star { .. })
                                            && ui.small_button("Relocate ship").clicked()
                                        {
                                            debug.relocate = Some(body.name.clone());
                                        }
                                    });
                                }
                            });
                    }
                });
        });
    Ok(())
}
