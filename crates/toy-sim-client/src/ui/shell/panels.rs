use super::instruments::navigation;
use super::overview::{overview_header, overview_row, selected_item};
use super::*;

pub(super) fn draw(
    ctx: &egui::Context,
    shell: &mut Shell,
    model: &FrameModel,
    selection: &Selection,
    results: &[CommandResult],
    intents: &mut Vec<Intent>,
) {
    let screen = ctx.content_rect();
    launcher(ctx, |ui| {
        ui.label(Icon::Layout.text(24.).color(ACCENT))
            .on_hover_text("Toy Sim workspace");
        ui.add_space(15.);
        for (spec, icon, label) in [
            (OVERVIEW, Icon::Overview, "Overview"),
            (SELECTED, Icon::Target, "Selected item"),
            (INVENTORY, Icon::Cargo, "Inventory"),
            (NAVIGATION, Icon::Navigation, "Navigation"),
            (MAP, Icon::Planet, "Gate network map"),
            (SOCIETY, Icon::Shield, "Society and ownership"),
        ] {
            if icon_button(ui, icon, label, shell.desktop.is_open(spec)).clicked() {
                shell.desktop.toggle(spec);
            }
        }
        ui.separator();
        if icon_button(ui, Icon::Planet, "Show orbital paths (O)", model.orbits).clicked() {
            intents.push(Intent::Orbits(!model.orbits));
        }
        if icon_button(ui, Icon::Look, "Return camera to your ship (Esc)", false).clicked() {
            intents.push(Intent::Look(None));
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            if icon_button(
                ui,
                Icon::Settings,
                "Interface settings",
                shell.desktop.is_open(SETTINGS),
            )
            .clicked()
            {
                shell.desktop.toggle(SETTINGS);
            }
        });
    });

    status_bar(ctx, |ui| {
        let seconds = model.time_ns / 1_000_000_000;
        ui.label(Icon::Clock.text(16.).color(ACCENT));
        let timestamp = model.calendar_unix_ms.map_or_else(
            || "Synchronizing calendar…".into(),
            toy_sim_model::calendar::format_utc,
        );
        ui.label(egui::RichText::new(timestamp).monospace().size(12.))
            .on_hover_text(format!(
                "Simulation T+{:02}:{:02}:{:02}.{}\nThe calendar follows real UTC + 400 years, including while simulation is paused.",
                seconds / 3600,
                (seconds / 60) % 60,
                seconds % 60,
                (model.time_ns / 100_000_000) % 10,
            ));
        ui.separator();
        let color = if model.connected {
            ACCENT
        } else {
            egui::Color32::from_rgb(230, 178, 104)
        };
        ui.colored_label(
            color,
            if model.connected {
                "Connected"
            } else if model.status.is_empty() {
                "Connecting…"
            } else {
                model.status
            },
        );
        ui.separator();
        let diagnostics = &model.diagnostics;
        ui.label(
            egui::RichText::new(format!(
                "Jitter {} ticks / Display {:.0} FPS{}",
                diagnostics.queued_frames,
                diagnostics.fps,
                if diagnostics.catching_up {
                    " / Catch-up"
                } else if diagnostics.buffering {
                    " / Buffering"
                } else {
                    ""
                },
            ))
            .monospace()
            .size(11.),
        )
        .on_hover_text(format!(
            "{:.0} ms buffered simulation time, {} underruns. FPS measures client frames.",
            diagnostics.buffered_ms, diagnostics.underruns,
        ));
        if screen.width() > 1250. {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(
                        "O  Orbits    ·    Esc  Own ship    ·    Right drag  Camera",
                    )
                    .size(11.)
                    .color(MUTED),
                );
            });
        }
    });

    egui::Area::new(egui::Id::new("current_location"))
        .fixed_pos(screen.min + egui::vec2(RAIL_WIDTH + 24., 24.))
        .movable(false)
        .show(ctx, |ui| {
            ui.set_max_width((screen.width() - 500.).max(180.));
            egui::Frame::new()
                .fill(SURFACE.linear_multiply(0.65))
                .inner_margin(12.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Icon::Planet.text(24.).color(ACCENT));
                        ui.label(
                            egui::RichText::new(&model.system)
                                .size(24.)
                                .strong()
                                .color(TEXT),
                        );
                    });
                    ui.label(egui::RichText::new(&model.vicinity).size(12.).color(MUTED));
                    if let Some(ship) = model.ship {
                        let ap = ship.travel.autopilot_enabled;
                        if ui
                            .add_enabled(
                                model.connected && ship.presence == travel::Presence::Space,
                                egui::Button::new(
                                    egui::RichText::new(if ap { "AP ON" } else { "AP OFF" })
                                        .strong()
                                        .monospace(),
                                )
                                .selected(ap)
                                .min_size(egui::vec2(96., 30.)),
                            )
                            .clicked()
                        {
                            intents
                                .push(Intent::Command(ShipCommand::SetAutopilot(!ap), "Autopilot"));
                        }
                        ui.horizontal(|ui| {
                            ui.label(Icon::Ship.text(14.).color(ACCENT));
                            ui.label(egui::RichText::new(ship_name(ship)).size(12.).color(TEXT));
                            if ship.travel.status != travel::Status::Idle {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "· {}",
                                        travel_status(&ship.travel.status)
                                    ))
                                    .size(11.)
                                    .color(ACCENT),
                                );
                            }
                        });
                        if ship
                            .travel
                            .fuel_budget
                            .as_ref()
                            .is_some_and(|budget| budget.exhausted())
                        {
                            ui.colored_label(
                                THREAT,
                                "FUEL EXHAUSTION RISK · replan in Gate Network",
                            );
                        }
                        instruments::itinerary(
                            ui,
                            &ship.travel,
                            model.navigation,
                            model.time_ns / 100_000_000,
                        );
                        if let Some(arrival) = ship
                            .travel
                            .estimated_arrival_tick
                            .filter(|_| matches!(ship.presence, travel::Presence::SlipTransit(_)))
                        {
                            let seconds =
                                (arrival as f64 * 0.1 - model.time_ns as f64 * 1e-9).max(0.);
                            ui.label(
                                egui::RichText::new(format!(
                                    "SLIP TRANSIT   ETA {:02}:{:02}",
                                    seconds as u64 / 60,
                                    seconds as u64 % 60
                                ))
                                .size(18.)
                                .color(ACCENT),
                            );
                        }
                        if matches!(ship.presence, travel::Presence::Docked { .. }) {
                            ui.label(egui::RichText::new("DOCKED · Hangar").color(ACCENT));
                            if ui.button("Undock").clicked() {
                                intents.push(Intent::Queue(vec![travel::Order::Undock], false));
                            }
                        }
                    }
                });
        });

    let selected = model
        .rows
        .iter()
        .find(|row| Some(row.target) == selection.target);
    let can_control = model.connected
        && model
            .ship
            .is_some_and(|ship| ship.presence == travel::Presence::Space);
    let mut stand_off = shell.stand_off;
    shell.desktop.show(ctx, SELECTED, |ui| {
        let weapons = model
            .details
            .and_then(|details| details.instruments.as_ref())
            .and_then(|instruments| instruments.weapons_state.as_ref());
        egui::ScrollArea::vertical()
            .max_height(SELECTED.size.y)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                selected_item(
                    ui,
                    selected,
                    can_control,
                    weapons,
                    &model.rows,
                    &mut stand_off,
                    intents,
                );
            });
    });
    shell.stand_off = stand_off;

    let marked = model
        .details
        .and_then(|details| details.instruments.as_ref())
        .and_then(|instruments| instruments.weapons_state.as_ref())
        .and_then(|weapons| weapons.target);
    let rows = sorted_rows(&model.rows, shell);
    let mut filter = shell.filter;
    let mut sort = shell.sort;
    let mut descending = shell.descending;
    let mut search = shell.search.clone();
    let mut overview_spec = OVERVIEW;
    if let Some(selected_rect) = shell.desktop.rect(SELECTED) {
        overview_spec.offset.y = selected_rect.height() + 14.;
    }
    shell.desktop.show(ctx, overview_spec, |ui| {
        ui.horizontal(|ui| {
            for (value, label) in [
                (Filter::All, "All"),
                (Filter::Ships, "Ships"),
                (Filter::Celestials, "Celestials"),
            ] {
                ui.selectable_value(&mut filter, value, label);
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut search)
                .hint_text("Filter by name or type…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(2.);
        overview_header(ui, &mut sort, &mut descending);
        let height = (ui.available_height() - 44.).max(27.);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(height)
            .show_rows(ui, 27., rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.;
                for index in range {
                    let row = rows[index];
                    let targeted =
                        marked.is_some_and(|target| row.target == SelectedTarget::Contact(target));
                    let response = overview_row(
                        ui,
                        row,
                        Some(row.target) == selection.target,
                        targeted,
                        index,
                    );
                    if response.clicked() {
                        intents.push(Intent::Select(row.target));
                    }
                    if response.double_clicked() {
                        intents.push(Intent::Look(Some(row.target)));
                    }
                    response.context_menu(|ui| {
                        if let Some(principal) = row.affiliation {
                            if ui.button("Show affiliation").clicked() {
                                intents.push(Intent::InspectAffiliation(principal));
                                ui.close();
                            }
                        }
                        if ui.button("Look at").clicked() {
                            intents.push(Intent::Look(Some(row.target)));
                            ui.close();
                        }
                        if ui
                            .add_enabled(can_control, egui::Button::new("Align to"))
                            .clicked()
                        {
                            intents.push(Intent::Align(row.target));
                            ui.close();
                        }
                        if let SelectedTarget::Contact(reference) = row.target {
                            if ui
                                .add_enabled(can_control, egui::Button::new("Keep range"))
                                .clicked()
                            {
                                intents.push(Intent::KeepRange(reference, stand_off));
                                ui.close();
                            }
                        }
                    });
                }
                if rows.is_empty() {
                    ui.weak("No matching objects in this view.");
                }
            });
        ui.separator();
        ui.label(
            egui::RichText::new(format!(
                "{} object{}  ·  Double-click to look at",
                rows.len(),
                if rows.len() == 1 { "" } else { "s" }
            ))
            .size(11.)
            .color(MUTED),
        );
    });
    shell.filter = filter;
    shell.sort = sort;
    shell.descending = descending;
    shell.search = search;

    shell
        .desktop
        .show(ctx, NAVIGATION, |ui| navigation(ui, model, intents));
    shell.desktop.show(ctx, INVENTORY, |ui| {
        inventory::draw(ui, &mut shell.inventory, model, intents)
    });
    shell
        .desktop
        .show(ctx, MAP, |ui| map::draw(ui, &mut shell.map, model, intents));
    shell.desktop.show(ctx, SOCIETY, |ui| {
        society::draw(ui, &mut shell.society, model, intents);
    });
    let mut locked = shell.desktop.locked;
    let mut reset = false;
    shell.desktop.show(ctx, SETTINGS, |ui| {
        ui.label(egui::RichText::new("Workspace").strong().color(ACCENT));
        ui.checkbox(&mut locked, "Lock window positions and sizes");
        ui.weak("Drag titles or empty window backgrounds to move windows. Resize at the edges. Nearby windows snap together.");
        let mut orbits = model.orbits;
        if ui.checkbox(&mut orbits, "Show orbital paths").changed() {
            intents.push(Intent::Orbits(orbits));
        }
        ui.add_space(8.);
        reset = ui.button("Restore default layout").clicked();
    });
    shell.desktop.locked = locked;
    if reset {
        shell.desktop.locked = false;
        shell.desktop.reset();
    }

    if let Some(feedback) = &mut shell.feedback {
        feedback.receive(results);
        let label = &feedback.label;
        let (message, color) = if let Some(error) = &feedback.error {
            (
                format!("{label}: {error}"),
                egui::Color32::from_rgb(237, 161, 130),
            )
        } else if feedback.pending.is_empty() {
            (
                format!("{label} accepted · tick {}", feedback.last_tick),
                ACCENT,
            )
        } else {
            (format!("{label} · awaiting server"), MUTED)
        };
        egui::Area::new(egui::Id::new("command_feedback"))
            .order(egui::Order::Foreground)
            .anchor(
                egui::Align2::CENTER_BOTTOM,
                egui::vec2(RAIL_WIDTH / 2., -STATUS_HEIGHT - 12.),
            )
            .movable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(SURFACE)
                    .inner_margin(8.)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(message).size(12.).color(color));
                            if ui.small_button("×").clicked() {
                                shell.feedback = None;
                            }
                        });
                    });
            });
    }
}
