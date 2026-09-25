use super::*;
use osg_ui::desktop::{BORDER, POSITIVE, SURFACE_RAISED, WARNING};

pub fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    if ui.available_width() < 1000. {
        egui::CollapsingHeader::new("Industry · facilities")
            .show(ui, |ui| sidebar(ui, state, model));
        content(ui, state, model, transfers, intents);
        return;
    }
    ui.horizontal_top(|ui| {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(250., ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| sidebar(ui, state, model),
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2((width - 275.).max(0.), ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| content(ui, state, model, transfers, intents),
        );
    });
}

fn sidebar(ui: &mut egui::Ui, state: &mut State, model: &FrameModel) {
    ui.horizontal(|ui| {
        ui.label(Icon::Industry.text(22.).color(ACCENT));
        ui.heading("Industry");
    });
    ui.horizontal(|ui| {
        if ui
            .selectable_label(!state.service.public, "My facilities")
            .clicked()
        {
            state.service.public = false;
            state.overview = true;
        }
        ui.selectable_value(&mut state.service.public, true, "Public");
    });
    ui.add(
        egui::TextEdit::singleline(&mut state.facility_search)
            .hint_text("Search facilities…")
            .desired_width(f32::INFINITY),
    );
    if ui
        .selectable_label(state.overview && !state.service.public, "☷  All facilities")
        .clicked()
    {
        state.overview = true;
        state.service.public = false;
    }
    let search = state.facility_search.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("industry_sidebar")
        .show(ui, |ui| {
            if state.service.public {
                for facility in model
                    .services
                    .as_ref()
                    .into_iter()
                    .flat_map(|view| &view.facilities)
                {
                    if !facility.summary.name.to_lowercase().contains(&search) {
                        continue;
                    }
                    egui::Frame::new()
                        .fill(SURFACE_RAISED)
                        .inner_margin(8.)
                        .show(ui, |ui| {
                            ui.set_min_width((ui.available_width() - 1.).max(0.));
                            if ui
                                .selectable_label(
                                    state.service.facility == Some(facility.summary.entity),
                                    &facility.summary.name,
                                )
                                .clicked()
                            {
                                state.service.facility = Some(facility.summary.entity);
                                state.service.work = None;
                            }
                            ui.weak(society::name(
                                &model.society.directory,
                                facility.summary.owner,
                            ));
                        });
                }
                return;
            }
            let mut facilities: Vec<_> = model.industry.summaries().collect();
            facilities.sort_by_key(|f| (f.metrics.system, f.name.as_str()));
            let mut previous_system = None;
            for facility in facilities {
                if !facility.name.to_lowercase().contains(&search)
                    || facility.capabilities.is_empty()
                {
                    continue;
                }
                if previous_system != Some(facility.metrics.system) {
                    let name = facility
                        .metrics
                        .system
                        .and_then(|id| {
                            model
                                .navigation
                                .systems
                                .iter()
                                .find(|system| system.id == id)
                        })
                        .map(|system| system.name.as_str())
                        .unwrap_or("Location unavailable");
                    ui.add_space(8.);
                    ui.small(egui::RichText::new(name.to_uppercase()).color(MUTED));
                    previous_system = Some(facility.metrics.system);
                }
                let stalled = facility.metrics.stalled_jobs > 0;
                let color = if stalled {
                    WARNING
                } else if facility.metrics.busy_lanes > 0 {
                    POSITIVE
                } else {
                    MUTED
                };
                let selected = state.facility == Some(facility.entity) && !state.overview;
                egui::Frame::new()
                    .fill(if selected {
                        egui::Color32::from_rgb(34, 66, 81)
                    } else {
                        SURFACE_RAISED
                    })
                    .inner_margin(8.)
                    .show(ui, |ui| {
                        ui.set_min_width((ui.available_width() - 1.).max(0.));
                        if ui
                            .selectable_label(
                                selected,
                                egui::RichText::new(format!("●  {}", facility.name)).color(color),
                            )
                            .clicked()
                        {
                            state.facility = Some(facility.entity);
                            state.overview = false;
                            state.service.public = false;
                        }
                        ui.small(society::name(&model.society.directory, facility.owner));
                        ui.weak(format!(
                            "{} / {} lanes · {} queued",
                            facility.metrics.busy_lanes,
                            facility.metrics.total_lanes,
                            facility.metrics.queued_jobs
                        ));
                    });
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(state.directory_after.is_some(), egui::Button::new("First"))
                    .clicked()
                {
                    state.directory_after = None;
                    state.previous_pages.clear();
                }
                if ui
                    .add_enabled(
                        model.industry.directory_next().is_some(),
                        egui::Button::new("Next"),
                    )
                    .clicked()
                {
                    state.directory_after = model.industry.directory_next();
                }
            });
        });
}

fn metric(ui: &mut egui::Ui, title: &str, value: String, note: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .inner_margin(12.)
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 1.).max(0.));
            ui.label(egui::RichText::new(title).small().color(MUTED));
            let size = if ui.available_width() < 180. {
                16.
            } else {
                21.
            };
            ui.label(egui::RichText::new(value).size(size).color(color));
            ui.weak(note);
        });
}

pub fn overview(ui: &mut egui::Ui, state: &mut State, model: &FrameModel) {
    let summaries: Vec<_> = model
        .industry
        .summaries()
        .filter(|facility| !facility.capabilities.is_empty())
        .collect();
    ui.heading("All facilities");
    ui.weak(format!("{} facilities you manage", summaries.len()));
    let facilities: Vec<_> = model
        .industry
        .facilities
        .values()
        .filter_map(QueryState::as_ref)
        .collect();
    let lanes: u32 = summaries.iter().map(|f| f.metrics.total_lanes).sum();
    let stalled: u32 = summaries.iter().map(|f| f.metrics.stalled_jobs).sum();
    let used: f64 = summaries
        .iter()
        .filter_map(|f| f.metrics.power_consumed_w)
        .sum();
    let generated: f64 = summaries
        .iter()
        .filter_map(|f| f.metrics.power_generated_w)
        .sum();
    let revenue = summaries
        .iter()
        .flat_map(|f| f.metrics.outside_revenue.iter().flatten())
        .fold([0_u64; 2], |mut total, (currency, amount)| {
            let index = if *currency == osg_model::economy::Currency::Uec {
                0
            } else {
                1
            };
            total[index] = total[index].saturating_add(*amount);
            total
        });
    ui.columns(4, |cols| {
        metric(
            &mut cols[0],
            "POWER",
            format!("{:.1} / {:.1} MW", used / 1e6, generated / 1e6),
            "consumption / generation",
            ACCENT,
        );
        metric(
            &mut cols[1],
            "LANES",
            format!(
                "{} / {} busy",
                summaries.iter().map(|f| f.metrics.busy_lanes).sum::<u32>(),
                lanes
            ),
            "facilities on this page",
            TEXT,
        );
        metric(
            &mut cols[3],
            "OUTSIDE REVENUE",
            format!("{} UEC", osg_model::economy::format_amount(revenue[0])),
            &format!(
                "+ {} LAT · cumulative",
                osg_model::economy::format_amount(revenue[1])
            ),
            POSITIVE,
        );
        metric(
            &mut cols[2],
            "STALLED",
            format!("{stalled} jobs"),
            "requiring attention",
            WARNING,
        );
    });
    if stalled > 0 {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(48, 39, 23))
            .stroke(egui::Stroke::new(1., WARNING))
            .inner_margin(12.)
            .show(ui, |ui| {
                ui.colored_label(WARNING, "NEEDS ATTENTION");
                for summary in summaries.iter().filter(|f| f.metrics.stalled_jobs > 0) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} · {} stalled jobs",
                            summary.name, summary.metrics.stalled_jobs
                        ));
                        if ui.small_button("Open").clicked() {
                            state.facility = Some(summary.entity);
                            state.overview = false;
                        }
                    });
                    if let Some(facility) = facilities.iter().find(|f| f.entity == summary.entity) {
                        for job in facility
                            .jobs
                            .iter()
                            .filter(|job| job.status == JobStatus::AwaitingPower)
                        {
                            ui.weak(format!(
                                "{} · waiting for power · {:.1} / {:.1} MW",
                                job.name,
                                job.supplied_power_w as f64 / 1e6,
                                job.requested_power_w as f64 / 1e6
                            ));
                        }
                    }
                }
            });
    }
    ui.add_space(10.);
    ui.weak("FACILITIES");
    egui::ScrollArea::both()
        .id_salt("industry_overview_table")
        .show(ui, |ui| {
            egui::Grid::new("industry_facilities")
                .striped(true)
                .min_col_width(95.)
                .spacing([18., 12.])
                .show(ui, |ui| {
                    for title in ["Facility", "Modules", "Power", "Lanes", "Queue", "Outside"] {
                        ui.weak(title);
                    }
                    ui.end_row();
                    for summary in &summaries {
                        if ui.selectable_label(false, &summary.name).clicked() {
                            state.facility = Some(summary.entity);
                            state.overview = false;
                        }
                        ui.horizontal(|ui| {
                            for capability in &summary.capabilities {
                                ui.label(module_icon(*capability).text(18.).color(ACCENT))
                                    .on_hover_text(capability_name(*capability));
                            }
                        });
                        ui.vertical(|ui| power(ui, &summary.metrics));
                        ui.label(format!(
                            "{} / {}",
                            summary.metrics.busy_lanes, summary.metrics.total_lanes
                        ));
                        ui.label(format!("{} queued", summary.metrics.queued_jobs));
                        ui.label(format!("{} jobs", summary.metrics.outside_jobs));
                        ui.end_row();
                    }
                });
        });
}

fn power(ui: &mut egui::Ui, metrics: &industry_model::FacilityMetrics) {
    match (metrics.power_consumed_w, metrics.power_generated_w) {
        (Some(used), Some(generated)) => {
            ui.label(format!("{:.1} / {:.1} MW", used / 1e6, generated / 1e6));
            ui.add(
                egui::ProgressBar::new((used / generated.max(1.)).clamp(0., 1.) as f32)
                    .desired_width(125.)
                    .desired_height(4.)
                    .fill(ACCENT),
            );
        }
        _ => {
            ui.weak("Unavailable");
        }
    }
}

fn module_icon(capability: IndustryCapability) -> Icon {
    match capability {
        IndustryCapability::Shipyard => Icon::Ship,
        IndustryCapability::FuelPlant => Icon::Power,
        IndustryCapability::Fabricator => Icon::Settings,
        IndustryCapability::Refinery => Icon::Industry,
    }
}

fn dashboard(
    ui: &mut egui::Ui,
    model: &FrameModel,
    facility: &FacilityView,
    intents: &mut Vec<Intent>,
) {
    ui.columns(3, |cols| {
        metric(
            &mut cols[0],
            "POWER",
            match (
                facility.metrics.power_consumed_w,
                facility.metrics.power_generated_w,
            ) {
                (Some(used), Some(generated)) => {
                    format!("{:.1} / {:.1} MW", used / 1e6, generated / 1e6)
                }
                _ => "Unavailable".into(),
            },
            "consumption / generation",
            ACCENT,
        );
        metric(
            &mut cols[2],
            "HANGAR BERTHS",
            match (facility.metrics.berths_used, facility.metrics.berths_total) {
                (Some(used), Some(total)) => format!("{used} / {total} used"),
                _ => "Unavailable".into(),
            },
            "docked ships",
            ACCENT,
        );
        metric(
            &mut cols[1],
            "CARGO SPACE",
            format!(
                "{:.0} / {:.0} m³",
                facility.cargo_used_m3, facility.cargo_capacity_m3
            ),
            "station storage",
            TEXT,
        );
    });
    for module in &facility.capabilities {
        egui::Frame::new()
            .stroke(egui::Stroke::new(1., BORDER))
            .inner_margin(12.)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 1.).max(0.));
                ui.horizontal_wrapped(|ui| {
                    ui.label(module_icon(module.capability).text(18.).color(ACCENT));
                    ui.colored_label(ACCENT, capability_name(module.capability));
                    ui.weak(format!(
                        "{} lanes · {:.1} MW / lane",
                        module.lanes,
                        module.power_per_lane_w as f64 / 1e6
                    ));
                    if !module.operational {
                        ui.colored_label(THREAT, "MODULE OFFLINE");
                    }
                });
                let mut view = facility.clone();
                view.jobs.retain(|job| {
                    job.capability == module.capability
                        && job.module_part.is_none_or(|part| part == module.part)
                });
                if view.jobs.is_empty() {
                    ui.weak("Idle · no queued work");
                } else {
                    jobs(ui, model, &view, intents);
                }
            });
    }
}

pub fn console(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    facility: &FacilityView,
    intents: &mut Vec<Intent>,
) {
    if !state.new_job {
        dashboard(ui, model, facility, intents);
        return;
    }
    if ui.available_width() >= 850. {
        ui.columns(2, |cols| {
            dashboard(&mut cols[0], model, facility, intents);
            sheet(&mut cols[1], "New job", |ui| {
                production(ui, state, model, facility, intents)
            });
        });
    } else {
        sheet(ui, "New job", |ui| {
            production(ui, state, model, facility, intents)
        });
    }
    if ui.button("Close job form").clicked() {
        state.new_job = false;
    }
}

fn sheet(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1., BORDER))
        .inner_margin(14.)
        .show(ui, |ui| {
            ui.heading(title);
            ui.separator();
            body(ui);
        });
}

pub fn build(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    facility: &FacilityView,
    intents: &mut Vec<Intent>,
) {
    if ui.available_width() >= 850. {
        ui.columns(2, |cols| {
            dashboard(&mut cols[0], model, facility, intents);
            sheet(&mut cols[1], "New build", |ui| {
                shipyard(ui, state, model, facility, intents)
            });
        });
    } else {
        sheet(ui, "New build", |ui| {
            shipyard(ui, state, model, facility, intents)
        });
    }
}
