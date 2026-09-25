use super::*;
use osg_model::assets::{AssetKind, AssetSummary};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Grouping {
    #[default]
    Location,
    Type,
    Owner,
}

#[derive(Default)]
pub struct State {
    grouping: Grouping,
    query: AssetsQuery,
    focused: Option<Id>,
    system: Option<String>,
    status: Option<String>,
    sharing: u8,
}

impl State {
    pub fn query(&mut self, open: bool) -> Option<AssetsQuery> {
        self.query.limit = 128;
        open.then(|| self.query.clone())
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    state: &mut super::State,
    model: &FrameModel,
    snapshot: &AssetsView,
    intents: &mut Vec<Intent>,
) {
    let previous_owner = state.browser.query.owner;
    components::filter_bar(ui, "assets_filter", &mut state.search, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (grouping, label) in [
                (Grouping::Location, "By location"),
                (Grouping::Type, "By type"),
                (Grouping::Owner, "By owner"),
            ] {
                ui.selectable_value(&mut state.browser.grouping, grouping, label);
            }
            egui::ComboBox::from_id_salt("assets_owner_filter")
                .selected_text(state.browser.query.owner.map_or_else(
                    || "All authorized owners".into(),
                    |owner| society::name(&model.society.directory, owner),
                ))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.browser.query.owner,
                        None,
                        "All authorized owners",
                    );
                    let owners: BTreeSet<_> = snapshot
                        .assets
                        .iter()
                        .map(|asset| asset.owner)
                        .chain(
                            model
                                .society
                                .gas_accounts
                                .iter()
                                .map(|account| account.owner),
                        )
                        .collect();
                    for owner in owners {
                        ui.selectable_value(
                            &mut state.browser.query.owner,
                            Some(owner),
                            society::name(&model.society.directory, owner),
                        );
                    }
                });
        });
    });
    // Bound search input before sending a query.
    state.search = state.search.chars().take(128).collect();
    if previous_owner != state.browser.query.owner || state.browser.query.search != state.search {
        state.browser.query.search = state.search.clone();
        state.browser.query.after = None;
        state.browser.query.goods_after = None;
        state.browser.query.sources_after = None;
        state.browser.query.item = None;
        state.browser.focused = None;
        state.selected.clear();
    }

    if snapshot.query.owner != state.browser.query.owner
        || snapshot.query.search != state.browser.query.search
        || snapshot.query.after != state.browser.query.after
        || snapshot.query.goods_after != state.browser.query.goods_after
    {
        ui.weak("Loading selected page…");
        return;
    }

    ui.horizontal_wrapped(|ui| {
        for (value, title) in [(0, "All"), (1, "Mine"), (2, "Shared with me")] {
            ui.selectable_value(&mut state.browser.sharing, value, title);
        }
        egui::ComboBox::from_id_salt("asset_system")
            .selected_text(state.browser.system.as_deref().unwrap_or("Any system"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.browser.system, None, "Any system");
                let systems: BTreeSet<_> = snapshot
                    .assets
                    .iter()
                    .filter_map(|asset| asset.system.as_ref())
                    .collect();
                for system in systems {
                    ui.selectable_value(&mut state.browser.system, Some(system.clone()), system);
                }
            });
        egui::ComboBox::from_id_salt("asset_status")
            .selected_text(state.browser.status.as_deref().unwrap_or("Any status"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.browser.status, None, "Any status");
                let statuses: BTreeSet<_> =
                    snapshot.assets.iter().map(|asset| &asset.status).collect();
                for status in statuses {
                    ui.selectable_value(&mut state.browser.status, Some(status.clone()), status);
                }
            });
    });

    ui.horizontal_wrapped(|ui| {
        if ui.button("Select manageable on page").clicked() {
            state.selected.extend(
                snapshot
                    .assets
                    .iter()
                    .filter(|asset| asset.can_manage)
                    .map(|asset| asset.id),
            );
        }
        if ui.button("Clear selection").clicked() {
            state.selected.clear();
        }
        ui.weak(format!("{} selected", state.selected.len()));
    });
    ui.separator();

    let wide = ui.available_width() >= 760.;
    egui::ScrollArea::vertical()
        .id_salt("assets_browser")
        .max_height(
            (ui.available_height() - if state.selected.is_empty() { 0. } else { 128. }).max(50.),
        )
        .show(ui, |ui| {
            if wide {
                let list_width = ui.available_width() - 320.;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(list_width, 0.),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            list(ui, state, model, snapshot);
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        egui::vec2(280., 0.),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            details(ui, state, model, snapshot, intents);
                        },
                    );
                });
            } else {
                details(ui, state, model, snapshot, intents);
                ui.separator();
                list(ui, state, model, snapshot);
            }
        });
}

fn list(ui: &mut egui::Ui, state: &mut super::State, model: &FrameModel, snapshot: &AssetsView) {
    let search = state.search.to_lowercase();
    let mut groups = BTreeMap::<String, Vec<&AssetSummary>>::new();
    for asset in &snapshot.assets {
        let mine = asset.owner == Principal::Player(model.society.account);
        if state
            .browser
            .system
            .as_ref()
            .is_some_and(|system| asset.system.as_ref() != Some(system))
            || state
                .browser
                .status
                .as_ref()
                .is_some_and(|status| &asset.status != status)
            || state.browser.sharing == 1 && !mine
            || state.browser.sharing == 2 && mine
        {
            continue;
        }
        let owner = society::name(&model.society.directory, asset.owner);
        let group = match state.browser.grouping {
            Grouping::Location => asset.location.clone(),
            Grouping::Type => match asset.kind {
                AssetKind::Ship => "Ships".into(),
                AssetKind::Installation => "Installations".into(),
            },
            Grouping::Owner => owner,
        };
        groups.entry(group).or_default().push(asset);
    }
    if groups.is_empty() {
        ui.weak("No matching ships or installations on this page.");
    }
    let row_width = ui.available_width();
    let table_left = ui.cursor().left();
    let widths = [0.35, 0.25, 0.23, 0.17].map(|fraction| (row_width - 24.) * fraction);
    components::table_row(
        ui,
        &widths,
        &["Name", "Location", "Owner", "Status"].map(|text| egui::RichText::new(text).color(MUTED)),
        false,
    );
    for (group, assets) in groups {
        egui::CollapsingHeader::new(format!("{} · {}", group, assets.len()))
            .id_salt(("asset_group", group))
            .default_open(true)
            .show(ui, |ui| {
                for (index, asset) in assets.into_iter().enumerate() {
                    ui.push_id(asset.id, |ui| {
                        let width = ui.available_width();
                        let active = state.browser.focused == Some(asset.id)
                            || state.selected.contains(&asset.id);
                        let rect =
                            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, 26.));
                        if active || index % 2 == 0 {
                            ui.painter().rect_filled(
                                rect,
                                0.,
                                if active {
                                    ACCENT.gamma_multiply(0.22)
                                } else {
                                    SURFACE_RAISED
                                },
                            );
                        }
                        ui.horizontal(|ui| {
                            let mut selected = state.selected.contains(&asset.id);
                            if ui
                                .add_enabled(
                                    asset.can_manage,
                                    egui::Checkbox::without_text(&mut selected),
                                )
                                .changed()
                            {
                                if selected {
                                    state.selected.insert(asset.id);
                                } else {
                                    state.selected.remove(&asset.id);
                                }
                            }
                            let name_width =
                                (widths[0] - (rect.left() - table_left) - 30.).max(60.);
                            ui.allocate_ui_with_layout(
                                egui::vec2(name_width, 24.),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_width(name_width);
                                    let icon = if asset.kind == AssetKind::Ship {
                                        osg_ui::icons::Icon::Ship
                                    } else {
                                        osg_ui::icons::Icon::Industry
                                    };
                                    ui.label(icon.text(15.).color(ACCENT));
                                    if ui
                                        .add(
                                            egui::Label::new(&asset.name)
                                                .truncate()
                                                .sense(egui::Sense::click()),
                                        )
                                        .clicked()
                                    {
                                        state.browser.focused = Some(asset.id);
                                        state.browser.query.item = None;
                                        state.browser.query.sources_after = None;
                                    }
                                },
                            );
                            asset_cell(ui, widths[1], &asset.location, MUTED);
                            asset_cell(
                                ui,
                                widths[2],
                                &society::name(&model.society.directory, asset.owner),
                                TEXT,
                            );
                            asset_cell(
                                ui,
                                widths[3],
                                &asset.status,
                                if asset.status.contains("Dock") {
                                    ACCENT
                                } else if asset.status.contains("fuel") {
                                    WARNING
                                } else {
                                    MINT
                                },
                            );
                        });
                    });
                }
            });
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.browser.query.after.is_some(),
                egui::Button::new("First assets"),
            )
            .clicked()
        {
            state.browser.query.after = None;
        }
        if ui
            .add_enabled(snapshot.next.is_some(), egui::Button::new("More assets"))
            .clicked()
        {
            state.browser.query.after = snapshot.next;
            state.browser.focused = None;
        }
    });

    ui.separator();
    ui.colored_label(
        ACCENT,
        format!(
            "Goods · {} kinds across authorized holdings",
            snapshot.total_goods
        ),
    );
    ui.weak("Totals include ship holds and principal station storage.");
    for goods in snapshot
        .goods
        .iter()
        .filter(|goods| search.is_empty() || goods.name.to_lowercase().contains(&search))
    {
        let rect =
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 26.));
        if state.browser.query.item.as_ref() == Some(&goods.item) {
            ui.painter()
                .rect_filled(rect, 0., ACCENT.gamma_multiply(0.22));
        }
        let cells = [
            egui::RichText::new(&goods.name),
            egui::RichText::new(format!("{} holdings", goods.locations)).color(MUTED),
            egui::RichText::new(format!("{} units", goods.quantity)),
            egui::RichText::new(format!("{} reserved", goods.reserved)).color(WARNING),
        ];
        components::table_row_aligned(ui, &widths, &cells, &[false, false, true, true], false);
        if ui
            .interact(
                rect,
                ui.id().with((&goods.name, "goods")),
                egui::Sense::click(),
            )
            .clicked()
        {
            state.browser.query.item = Some(goods.item.clone());
            state.browser.query.sources_after = None;
            state.browser.focused = None;
        }
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.browser.query.goods_after.is_some(),
                egui::Button::new("First goods"),
            )
            .clicked()
        {
            state.browser.query.goods_after = None;
        }
        if ui
            .add_enabled(
                snapshot.goods_next.is_some(),
                egui::Button::new("More goods"),
            )
            .clicked()
        {
            state.browser.query.goods_after = snapshot.goods_next.clone();
        }
    });
}

fn details(
    ui: &mut egui::Ui,
    state: &mut super::State,
    model: &FrameModel,
    snapshot: &AssetsView,
    intents: &mut Vec<Intent>,
) {
    if state.selected.len() > 1 {
        ui.heading(format!("{} assets selected", state.selected.len()));
        ui.weak("Access and ownership");
        ui.separator();
        let mut locations = BTreeMap::<String, usize>::new();
        let mut profiles = BTreeMap::<String, usize>::new();
        let directory = &model.society.directory;
        for asset in snapshot
            .assets
            .iter()
            .filter(|asset| state.selected.contains(&asset.id))
        {
            *locations.entry(asset.location.clone()).or_default() += 1;
            let profile = directory
                .access_bindings
                .get(&asset.id)
                .and_then(|binding| directory.access_profiles.get(&binding.profile))
                .map_or("Private", |profile| profile.name.as_str());
            *profiles.entry(profile.to_owned()).or_default() += 1;
        }
        ui.weak("ACCESS PROFILES IN USE");
        for (name, count) in profiles {
            detail_row(ui, &name, &format!("{count} assets"), TEXT);
        }
        ui.separator();
        ui.weak("LOCATIONS ON THIS PAGE");
        for (name, count) in locations {
            detail_row(ui, &name, &count.to_string(), TEXT);
        }
        if let Some(profile) = state
            .profile
            .and_then(|id| directory.access_profiles.get(&id))
        {
            ui.separator();
            ui.colored_label(ACCENT, format!("APPLY “{}”", profile.name));
            ui.weak("The selected assets will use this permission profile.");
        }
        ui.add_space(8.);
        ui.weak("Choose a profile or ownership action in the selection toolbar.");
        return;
    }
    if let Some(item) = &state.browser.query.item {
        let goods = snapshot.goods.iter().find(|goods| &goods.item == item);
        ui.heading(goods.map_or("Goods", |goods| goods.name.as_str()));
        if let Some(goods) = goods {
            egui::Frame::new()
                .fill(SURFACE_RAISED)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.columns(3, |columns| {
                        for (index, label, value, color) in [
                            (0, "TOTAL", goods.quantity, TEXT),
                            (1, "AVAILABLE", goods.quantity - goods.reserved, MINT),
                            (2, "RESERVED", goods.reserved, WARNING),
                        ] {
                            columns[index].weak(label);
                            columns[index].colored_label(color, value.to_string());
                        }
                    });
                });
            ui.weak(format!("{} holdings", goods.locations));
        }
        ui.separator();
        if snapshot.query.item != state.browser.query.item
            || snapshot.query.sources_after != state.browser.query.sources_after
        {
            ui.weak("Loading holdings…");
            return;
        }
        for source in &snapshot.sources {
            ui.push_id(&source.key, |ui| {
                ui.label(&source.name);
                ui.weak(society::name(&model.society.directory, source.key.owner));
                ui.label(format!(
                    "{} units · {} reserved",
                    source.quantity, source.reserved
                ));
                if source.key.storage {
                    if ui.button("Open storage / market").clicked() {
                        intents.push(Intent::OpenStorage {
                            owner: source.key.owner,
                            station: source.key.entity,
                            item: item.clone(),
                        });
                    }
                } else if ui.button("Open cargo").clicked() {
                    intents.push(Intent::InspectInventory(source.key.entity));
                }
                ui.separator();
            });
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    state.browser.query.sources_after.is_some(),
                    egui::Button::new("First holdings"),
                )
                .clicked()
            {
                state.browser.query.sources_after = None;
            }
            if ui
                .add_enabled(
                    snapshot.sources_next.is_some(),
                    egui::Button::new("More holdings"),
                )
                .clicked()
            {
                state.browser.query.sources_after = snapshot.sources_next.clone();
            }
        });
        return;
    }

    let Some(asset) = snapshot
        .assets
        .iter()
        .find(|asset| Some(asset.id) == state.browser.focused)
    else {
        components::empty_state(
            ui,
            "Asset details",
            "Select a ship, installation or good to inspect it.",
        );
        return;
    };
    ui.heading(&asset.name);
    ui.colored_label(ACCENT, format!("{:?} · {}", asset.kind, asset.status));
    ui.separator();
    detail_row(
        ui,
        "Owner",
        &society::name(&model.society.directory, asset.owner),
        TEXT,
    );
    detail_row(ui, "Location", &asset.location, TEXT);
    detail_row(ui, "Status", &asset.status, ACCENT);
    if let Some(telemetry) = &asset.telemetry {
        detail_row(
            ui,
            "Hold",
            &format!(
                "{:.1} / {:.1} m³",
                telemetry.cargo_used_m3, telemetry.cargo_capacity_m3
            ),
            TEXT,
        );
        if telemetry.energy_capacity_j > 0 {
            detail_row(
                ui,
                "Battery",
                &format!(
                    "{:.0}%",
                    100. * telemetry.energy_j as f64 / telemetry.energy_capacity_j as f64
                ),
                POSITIVE,
            );
        }
        for consumable in &telemetry.consumables {
            detail_row(
                ui,
                &consumable.name,
                &format!(
                    "{:.0}%",
                    100. * consumable.quantity as f64 / consumable.capacity.max(1) as f64
                ),
                if consumable.quantity * 5 < consumable.capacity {
                    WARNING
                } else {
                    TEXT
                },
            );
        }
    }
    detail_row(
        ui,
        "Access",
        if asset.can_manage {
            "Manage"
        } else if asset.can_focus {
            "Pilot, cargo"
        } else if asset.can_open {
            "Cargo"
        } else {
            "View"
        },
        TEXT,
    );
    if let Some(binding) = model.society.directory.access_bindings.get(&asset.id) {
        if let Some(profile) = model
            .society
            .directory
            .access_profiles
            .get(&binding.profile)
        {
            ui.weak("Permission profile");
            ui.colored_label(MINT, &profile.name);
        }
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                asset.can_focus && model.connected,
                egui::Button::new("Focus ship"),
            )
            .clicked()
        {
            intents.push(Intent::FocusShip(asset.id));
        }
        if ui
            .add_enabled(asset.can_open, egui::Button::new("Open cargo"))
            .clicked()
        {
            intents.push(Intent::InspectInventory(asset.id));
        }
        if ui.button("Owner").clicked() {
            intents.push(Intent::InspectAffiliation(asset.owner));
        }
        if ui
            .add_enabled(asset.can_manage, egui::Button::new("Access / transfer…"))
            .clicked()
        {
            state.selected.insert(asset.id);
        }
    });
}

fn asset_cell(ui: &mut egui::Ui, width: f32, text: &str, color: egui::Color32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 24.),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(width);
            ui.add(egui::Label::new(egui::RichText::new(text).color(color)).truncate());
        },
    );
}

fn detail_row(ui: &mut egui::Ui, label: &str, value: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.weak(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(value).color(color)).truncate());
        });
    });
}
