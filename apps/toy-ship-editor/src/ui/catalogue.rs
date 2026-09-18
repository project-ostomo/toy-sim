use crate::{Editor, previews::PartPreviews};
use toy_sim_ui::{
    egui,
    parts::{self, Category},
};

pub fn show(ui: &mut egui::Ui, editor: &mut Editor, previews: &PartPreviews) {
    ui.heading("Parts");
    ui.add(
        egui::TextEdit::singleline(&mut editor.catalogue_search)
            .hint_text("Search parts…")
            .desired_width(f32::INFINITY),
    );
    egui::ComboBox::from_id_salt("part-category")
        .width(ui.available_width())
        .height(320.)
        .selected_text(editor.category.label())
        .show_ui(ui, |ui| {
            for category in Category::ALL {
                ui.selectable_value(&mut editor.category, category, category.label());
            }
        });

    let filtered: Vec<_> = editor
        .catalogue
        .parts
        .iter()
        .enumerate()
        .filter(|(index, part)| {
            editor.descriptions[*index].matches(part, editor.category, &editor.catalogue_search)
        })
        .map(|(index, _)| index)
        .collect();
    let noun = if filtered.len() == 1 { "part" } else { "parts" };
    ui.weak(format!("{} {noun}", filtered.len()));
    ui.separator();

    let footer_height = if editor.place.is_some() { 132. } else { 54. };
    let height = (ui.available_height() - footer_height).max(64.);
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("part-grid-scroll")
        .auto_shrink([false, false])
        .max_height(height)
        .show(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            let columns = ((ui.available_width() + gap) / (80. + gap)).floor().max(1.) as usize;
            let width =
                ((ui.available_width() - gap * (columns - 1) as f32) / columns as f32).min(112.);
            if filtered.is_empty() {
                ui.add_space(12.);
                ui.label("No matching parts");
                if ui.button("Clear filters").clicked() {
                    editor.catalogue_search.clear();
                    editor.category = Category::All;
                }
            }
            egui::Grid::new("part-grid")
                .num_columns(columns)
                .spacing(egui::vec2(gap, 10.))
                .show(ui, |ui| {
                    for row in filtered.chunks(columns) {
                        for &index in row {
                            let part = &editor.catalogue.parts[index];
                            let description = &editor.descriptions[index];
                            let image = Some(previews.image(index));
                            ui.push_id(&part.id, |ui| {
                                let response = parts::tile(
                                    ui,
                                    &part.title,
                                    image,
                                    editor.place.as_ref() == Some(&part.id),
                                    width,
                                );
                                if response.clicked() {
                                    picked = Some(part.id.clone());
                                }
                                parts::show_tooltip(&response, &part.title, description, image);
                            });
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(prototype) = picked {
        editor.pick_up(prototype);
    }

    ui.separator();
    if let Some(prototype) = &editor.place {
        if let Some(definition) = editor.catalogue.part(prototype) {
            ui.strong(&definition.title);
        }
        ui.weak(format!(
            "Placement rotation {} / 24",
            editor.orientation + 1
        ));
        ui.horizontal_wrapped(|ui| {
            if ui.button("Rotate (R)").clicked() {
                editor.orientation = (editor.orientation + 1) % 24;
            }
            if ui.button("Select mode").clicked() {
                editor.cancel_placement();
            }
        });
        ui.small("Click in the assembly to place · Esc to cancel");
        ui.small("Hold Shift to snap to a 1 m grid");
    } else {
        ui.weak("Click a part to pick it up.");
    }
}
