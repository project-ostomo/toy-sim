use super::*;
use industry_model::*;
use osg_model::{
    economy::{Currency, format_amount, parse_amount},
    ownership::Principal,
};

#[derive(Default)]
pub(crate) struct State {
    pub public: bool,
    pub facility: Option<Id>,
    pub work: Option<ServiceWork>,
    pub payer: Option<Principal>,
    search: String,
    after: Option<Id>,
    editing: Option<(Id, ServicePolicy)>,
    new_tier: Option<CustomerMatch>,
    job_open: bool,
    comparison_work: Option<ServiceWork>,
    sort: usize,
    preview_recipe: Option<String>,
}

pub(crate) enum Action {
    Publish(Id, ServicePolicy),
    Order(ServiceQuote),
    Cancel(Id, Id),
}

pub(crate) async fn submit(
    net: osg_net::OsgNetClient,
    operation: osg_model::rpc::Operation,
    action: Action,
) -> Result<(), String> {
    use crate::state::requests::call;
    match action {
        Action::Publish(facility, policy) => {
            call(net.publish_service_prices(operation, facility, policy)).await
        }
        Action::Order(quote) => call(net.order_industry_job(operation, quote)).await,
        Action::Cancel(facility, job) => {
            call(net.cancel_service_job(operation, facility, job)).await
        }
    }
}

impl State {
    #[cfg(test)]
    pub(crate) fn gallery_comparison(&mut self, work: ServiceWork) {
        self.comparison_work = Some(work);
        self.work = None;
    }

    pub fn query(
        &self,
        open: bool,
        account: Id,
    ) -> Option<crate::state::requests::services::Interest> {
        (open && self.public).then(|| crate::state::requests::services::Interest {
            search: self.search.clone(),
            after: self.after,
            facility: self.facility,
            payer: self.payer.unwrap_or(Principal::Player(account)),
            work: self.work.clone().or_else(|| self.comparison_work.clone()),
        })
    }
}

fn money(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut u64) {
    let id = ui.make_persistent_id(id);
    let mut text = ui
        .ctx()
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| format_amount(*value));
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .id(id)
            .desired_width(100.),
    );
    if let Some(amount) = parse_amount(&text) {
        *value = amount;
    }
    if response.has_focus() {
        ui.ctx().data_mut(|data| data.insert_temp(id, text));
    } else {
        ui.ctx().data_mut(|data| data.remove::<String>(id));
    }
}

pub(super) fn pricing(
    ui: &mut egui::Ui,
    state: &mut State,
    facility: &FacilityView,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    if !facility.can_configure_service {
        return;
    }
    if state
        .editing
        .as_ref()
        .is_none_or(|(id, _)| *id != facility.entity)
    {
        state.editing = Some((facility.entity, facility.service.clone()));
    }
    let (_, policy) = state.editing.as_mut().unwrap();
    let mut published = facility.service.clone();
    published.revision = policy.revision;
    if published == *policy {
        policy.revision = facility.service.revision;
    }
    osg_ui::components::action_header(
        ui,
        "service_prices",
        "Service & pricing",
        "Prices apply to new orders. Queued jobs retain their quote.",
        |ui| {
            if ui.button("Revert").clicked() {
                *policy = facility.service.clone();
            }
            if ui
                .add_enabled(model.connected, egui::Button::new("Publish prices"))
                .clicked()
            {
                intents.push(Intent::Service(Action::Publish(
                    facility.entity,
                    policy.clone(),
                )));
            }
        },
    );
    if policy.revision != facility.service.revision {
        ui.colored_label(
            WARNING,
            "Published prices changed. Revert to load the current revision.",
        );
    }
    egui::Frame::new()
        .fill(osg_ui::desktop::SURFACE_RAISED)
        .inner_margin(12.)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut policy.accepting, "Accept outside jobs");
                ui.separator();
                ui.weak("Pricing currency");
                ui.selectable_value(&mut policy.currency, Currency::Uec, "UEC");
                ui.selectable_value(&mut policy.currency, Currency::Lat, "LAT");
            });
            ui.weak("Customer inputs and outputs stay in their station storage.");
            ui.horizontal_wrapped(|ui| {
                ui.weak("Outside revenue");
                if let Some(revenue) = &facility.metrics.outside_revenue {
                    for (currency, amount) in revenue {
                        ui.colored_label(
                            osg_ui::desktop::POSITIVE,
                            format!("{} {}", format_amount(*amount), currency),
                        );
                    }
                } else {
                    ui.weak("Not available")
                        .on_hover_text("Revenue history is not recorded per facility.");
                }
            });
        });
    ui.add_space(12.);
    ui.strong("MODULE RATES");
    let capabilities: std::collections::BTreeSet<_> = facility
        .capabilities
        .iter()
        .map(|module| module.capability)
        .collect();
    egui::ScrollArea::horizontal()
        .id_salt("rate_scroll")
        .show(ui, |ui| {
            egui::Grid::new("service_rates")
                .striped(true)
                .min_row_height(30.)
                .spacing([18., 6.])
                .show(ui, |ui| {
                    for heading in ["Module", "Energy / MJ", "Time / lane-hour", "Public lanes"] {
                        ui.weak(heading);
                    }
                    ui.end_row();
                    for capability in capabilities {
                        if !policy
                            .rates
                            .iter()
                            .any(|rate| rate.capability == capability)
                        {
                            policy.rates.push(ServiceRate {
                                capability,
                                energy_per_mj: 0,
                                time_per_hour: 0,
                                public_lanes: 0,
                            });
                        }
                        let rate = policy
                            .rates
                            .iter_mut()
                            .find(|rate| rate.capability == capability)
                            .unwrap();
                        let count: u32 = facility
                            .capabilities
                            .iter()
                            .filter(|module| module.capability == capability)
                            .map(|module| module.lanes)
                            .sum();
                        let operational = facility
                            .capabilities
                            .iter()
                            .any(|module| module.capability == capability && module.operational);
                        if operational {
                            ui.colored_label(INFO, capability_name(capability));
                        } else {
                            ui.colored_label(
                                osg_ui::desktop::THREAT,
                                format!("{} · offline", capability_name(capability)),
                            );
                        }
                        money(
                            ui,
                            (capability_name(capability), "energy"),
                            &mut rate.energy_per_mj,
                        );
                        money(
                            ui,
                            (capability_name(capability), "time"),
                            &mut rate.time_per_hour,
                        );
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut rate.public_lanes).range(0..=count));
                            ui.weak(format!("of {count}"));
                        });
                        ui.end_row();
                    }
                });
        });
    ui.add_space(16.);
    ui.strong("CUSTOMER TIERS");
    ui.weak("Checked from top to bottom · first match wins");
    let mut move_tier = None;
    let mut remove = None;
    let length = policy.tiers.len();
    egui::ScrollArea::horizontal()
        .id_salt("tier_scroll")
        .show(ui, |ui| {
            egui::Grid::new("service_tiers")
                .striped(true)
                .min_row_height(28.)
                .spacing([18., 6.])
                .show(ui, |ui| {
                    for heading in ["Order", "Customer", "Price", ""] {
                        ui.weak(heading);
                    }
                    ui.end_row();
                    for (index, tier) in policy.tiers.iter_mut().enumerate() {
                        ui.label(format!("{}", index + 1));
                        ui.label(tier_name(&tier.customer, &model.society.directory));
                        egui::ComboBox::from_id_salt(("price", index))
                            .selected_text(
                                tier.price_basis_points.map_or("Refused".into(), |rate| {
                                    format!("{}% of list", rate / 100)
                                }),
                            )
                            .show_ui(ui, |ui| {
                                for (rate, label) in [
                                    (Some(10_000), "List price"),
                                    (Some(9_000), "10% discount"),
                                    (Some(8_000), "20% discount"),
                                    (Some(0), "Free"),
                                    (None, "Refused"),
                                ] {
                                    ui.selectable_value(&mut tier.price_basis_points, rate, label);
                                }
                            });
                        ui.horizontal(|ui| {
                            if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked() {
                                move_tier = Some((index, index - 1));
                            }
                            if ui
                                .add_enabled(index + 1 < length, egui::Button::new("↓"))
                                .clicked()
                            {
                                move_tier = Some((index, index + 1));
                            }
                            if ui.small_button("Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    if let Some((a, b)) = move_tier {
        policy.tiers.swap(a, b);
    }
    if let Some(index) = remove {
        policy.tiers.remove(index);
    }
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("new_customer_tier")
            .selected_text(
                state
                    .new_tier
                    .as_ref()
                    .map_or("Select customer".into(), |tier| {
                        tier_name(tier, &model.society.directory)
                    }),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut state.new_tier,
                    Some(CustomerMatch::Everyone),
                    "Everyone else",
                );
                for principal in society::principals(&model.society.directory) {
                    ui.selectable_value(
                        &mut state.new_tier,
                        Some(CustomerMatch::Principal(principal)),
                        society::name(&model.society.directory, principal),
                    );
                    if matches!(principal, Principal::Sovereignty(_)) {
                        ui.selectable_value(
                            &mut state.new_tier,
                            Some(CustomerMatch::Declaration {
                                source: principal,
                                category: osg_model::diplomacy::DeclarationCategory::Wanted,
                            }),
                            format!(
                                "{} · wanted list",
                                society::name(&model.society.directory, principal)
                            ),
                        );
                    }
                }
                for bloc in model.society.directory.diplomacy.blocs.values() {
                    ui.selectable_value(
                        &mut state.new_tier,
                        Some(CustomerMatch::Bloc(bloc.id)),
                        &bloc.name,
                    );
                }
            });
        if ui
            .add_enabled(
                state.new_tier.is_some() && policy.tiers.len() < 32,
                egui::Button::new("Add tier"),
            )
            .clicked()
        {
            policy.tiers.push(CustomerTier {
                customer: state.new_tier.clone().unwrap(),
                price_basis_points: Some(10_000),
            });
        }
    });
    ui.weak("Unmatched customers pay list price.");
    if let Some(catalogue) = &model.industry.catalogue {
        ui.add_space(12.);
        egui::Frame::new()
            .fill(osg_ui::desktop::SURFACE_RAISED)
            .inner_margin(12.)
            .show(ui, |ui| {
                ui.strong("LIST PRICE PREVIEW · 1 BATCH");
                let recipes: Vec<_> = catalogue.recipes.iter().filter(|recipe| {
                    policy.rates.iter().any(|rate| rate.capability == recipe.capability)
                }).collect();
                if state.preview_recipe.is_none() {
                    state.preview_recipe = recipes.first().map(|recipe| recipe.id.clone());
                }
                let recipe = recipes.iter().find(|recipe| Some(&recipe.id) == state.preview_recipe.as_ref());
                egui::ComboBox::from_id_salt("price_preview_recipe")
                    .selected_text(recipe.map_or("No compatible recipes", |recipe| recipe.name.as_str()))
                    .show_ui(ui, |ui| {
                        for recipe in &recipes {
                            ui.selectable_value(&mut state.preview_recipe, Some(recipe.id.clone()), &recipe.name);
                        }
                    });
                if let Some(recipe) = recipes.iter().find(|recipe| Some(&recipe.id) == state.preview_recipe.as_ref()) {
                    if let Some(rate) = policy.rates.iter().find(|rate| rate.capability == recipe.capability) {
                        let energy = (u128::from(recipe.energy_j) * u128::from(rate.energy_per_mj)).div_ceil(1_000_000);
                        let time = (u128::from(recipe.duration_ticks) * u128::from(rate.time_per_hour)).div_ceil(36_000);
                        if let (Ok(energy), Ok(time), Ok(total)) = (u64::try_from(energy), u64::try_from(time), u64::try_from(energy + time)) {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("Energy {}", format_amount(energy)));
                                ui.label(format!("Lane time {}", format_amount(time)));
                                ui.colored_label(ACCENT, format!("Total {} {}", format_amount(total), policy.currency));
                            });
                            ui.weak("Preview uses these draft rates. Customer tiers adjust the final quote.");
                        } else {
                            ui.colored_label(WARNING, "Price exceeds the maximum supported amount.");
                        }
                    }
                }
            });
    }
}

fn tier_name(tier: &CustomerMatch, directory: &ownership::OwnershipDirectory) -> String {
    match tier {
        CustomerMatch::Principal(principal) => society::name(directory, *principal),
        CustomerMatch::Bloc(id) => directory
            .diplomacy
            .blocs
            .get(id)
            .map_or_else(|| id.to_string(), |bloc| bloc.name.clone()),
        CustomerMatch::Declaration { source, category } => {
            format!("{} · {category:?}", society::name(directory, *source))
        }
        CustomerMatch::Everyone => "Everyone else".into(),
    }
}

pub(super) fn customer(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let view = model.services;
    if state.job_open || state.work.is_some() {
        if let Some(facility) = view
            .facilities
            .iter()
            .find(|facility| Some(facility.summary.entity) == state.facility)
        {
            let payer = state
                .payer
                .unwrap_or(Principal::Player(model.society.account));
            let width = ui.available_width();
            let height = ui.available_height();
            if width >= 850. {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width - 450., height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("public_job_overview")
                                .max_height(height)
                                .show(ui, |ui| {
                                    ui.heading(&facility.summary.name);
                                    ui.weak(society::name(
                                        &model.society.directory,
                                        facility.summary.owner,
                                    ));
                                    ui.add_space(12.);
                                    ui.colored_label(
                                        osg_ui::desktop::POSITIVE,
                                        "● Accepting outside work",
                                    );
                                    ui.label(format!(
                                        "{} queued · {} running",
                                        facility.queued_jobs, facility.running_jobs
                                    ));
                                    ui.separator();
                                    ui.strong("PUBLIC PRODUCTION LANES");
                                    for rate in &facility.policy.rates {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.colored_label(
                                                INFO,
                                                capability_name(rate.capability),
                                            );
                                            ui.label(format!("{} lanes", rate.public_lanes));
                                        });
                                    }
                                    ui.add_space(12.);
                                    ui.strong("YOUR STORAGE HERE");
                                    for stock in &view.stock {
                                        ui.label(format!(
                                            "{} × {}",
                                            stock.quantity.saturating_sub(stock.reserved),
                                            cargo::item_label(&stock.item)
                                        ));
                                    }
                                    if view.stock.is_empty() {
                                        ui.weak("No materials in your station storage.");
                                    }
                                    ui.add_space(16.);
                                    customer_jobs(ui, facility.summary.entity, model, intents);
                                });
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        egui::vec2(430., height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| job_sheet(ui, state, facility, payer, model, intents),
                    );
                });
            } else {
                job_sheet(ui, state, facility, payer, model, intents);
            }
            return;
        }
    }
    egui::ScrollArea::vertical()
        .id_salt("public_facility_browser")
        .auto_shrink([false, false])
        .min_scrolled_height(0.)
        .max_height(ui.available_height())
        .show(ui, |ui| search(ui, state, model, intents));
}

fn search(ui: &mut egui::Ui, state: &mut State, model: &FrameModel, intents: &mut Vec<Intent>) {
    let view = model.services;
    osg_ui::components::action_header(
        ui,
        "public_industry",
        "Find a facility",
        "Use your station storage to commission production or a ship.",
        |_| {},
    );
    if ui
        .add(egui::TextEdit::singleline(&mut state.search).hint_text("Search facilities…"))
        .changed()
    {
        state.after = None;
    }
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("compare_recipe")
            .selected_text(match &state.comparison_work {
                Some(ServiceWork::Recipe { recipe, .. }) => model
                    .industry
                    .catalogue
                    .as_ref()
                    .and_then(|catalogue| {
                        catalogue.recipes.iter().find(|entry| &entry.id == recipe)
                    })
                    .map_or("Select recipe", |entry| entry.name.as_str()),
                _ => "All modules",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.comparison_work, None, "All modules");
                if let Some(catalogue) = &model.industry.catalogue {
                    for recipe in &catalogue.recipes {
                        let selected = match &state.comparison_work {
                            Some(ServiceWork::Recipe { recipe: id, .. }) => id == &recipe.id,
                            _ => false,
                        };
                        if ui.selectable_label(selected, &recipe.name).clicked() {
                            state.comparison_work = Some(ServiceWork::Recipe {
                                recipe: recipe.id.clone(),
                                batches: 1,
                            });
                        }
                    }
                }
            });
        if let Some(ServiceWork::Recipe { batches, .. }) = &mut state.comparison_work {
            ui.label("Batches");
            ui.add(egui::DragValue::new(batches).range(1..=MAX_RECIPE_BATCHES));
        }
        let sorts = [
            "Facility name",
            "Quote / currency",
            "Queue length",
            "Public lanes",
            "Free facility lanes",
        ];
        egui::ComboBox::from_id_salt("facility_sort")
            .selected_text(sorts[state.sort])
            .show_ui(ui, |ui| {
                for (index, label) in sorts.iter().enumerate() {
                    ui.selectable_value(&mut state.sort, index, *label);
                }
            });
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(state.after.is_some(), egui::Button::new("First page"))
            .clicked()
        {
            state.after = None;
        }
        if ui
            .add_enabled(view.next.is_some(), egui::Button::new("Next page"))
            .clicked()
        {
            state.after = view.next;
        }
    });
    egui::ScrollArea::horizontal()
        .id_salt("public_facility_results")
        .show(ui, |ui| {
            egui::Grid::new("public_facility_comparison")
                .striped(true)
                .min_row_height(42.)
                .spacing([18., 8.])
                .show(ui, |ui| {
                    for heading in [
                        "Facility / operator",
                        "Module",
                        "Energy / MJ",
                        "Time / hour",
                        "Your quote / rate",
                        "Public lanes",
                        "Free facility lanes",
                        "Queue",
                    ] {
                        ui.weak(heading);
                    }
                    ui.end_row();
                    let mut facilities: Vec<_> = view.facilities.iter().collect();
                    facilities.sort_by(|a, b| match state.sort {
                        1 => {
                            let key = |facility: &PublicFacility| {
                                view.quotes
                                    .get(&facility.summary.entity)
                                    .and_then(|result| result.as_ref().ok())
                                    .map(|quote| (quote.currency.to_string(), quote.total))
                            };
                            match (key(a), key(b)) {
                                (Some(a), Some(b)) => a.cmp(&b),
                                (Some(_), None) => std::cmp::Ordering::Less,
                                (None, Some(_)) => std::cmp::Ordering::Greater,
                                _ => a.summary.name.cmp(&b.summary.name),
                            }
                        }
                        2 => a.queued_jobs.cmp(&b.queued_jobs),
                        3 => b
                            .policy
                            .rates
                            .iter()
                            .map(|rate| rate.public_lanes)
                            .sum::<u32>()
                            .cmp(
                                &a.policy
                                    .rates
                                    .iter()
                                    .map(|rate| rate.public_lanes)
                                    .sum::<u32>(),
                            ),
                        4 => b
                            .summary
                            .metrics
                            .total_lanes
                            .saturating_sub(b.summary.metrics.busy_lanes)
                            .cmp(
                                &a.summary
                                    .metrics
                                    .total_lanes
                                    .saturating_sub(a.summary.metrics.busy_lanes),
                            ),
                        _ => a.summary.name.cmp(&b.summary.name),
                    });
                    for facility in facilities {
                        for (index, rate) in facility.policy.rates.iter().enumerate() {
                            ui.vertical(|ui| {
                                if index != 0 {
                                    return;
                                }
                                if ui
                                    .selectable_label(
                                        state.facility == Some(facility.summary.entity),
                                        &facility.summary.name,
                                    )
                                    .clicked()
                                {
                                    state.facility = Some(facility.summary.entity);
                                    state.work = None;
                                }
                                ui.weak(society::name(
                                    &model.society.directory,
                                    facility.summary.owner,
                                ));
                            });
                            ui.colored_label(INFO, capability_name(rate.capability));
                            ui.monospace(format!(
                                "{} {}",
                                format_amount(rate.energy_per_mj),
                                facility.policy.currency
                            ));
                            ui.monospace(format!(
                                "{} {}",
                                format_amount(rate.time_per_hour),
                                facility.policy.currency
                            ));
                            match view.quotes.get(&facility.summary.entity) {
                                Some(Ok(quote)) => {
                                    ui.vertical(|ui| {
                                        ui.colored_label(
                                            ACCENT,
                                            format!(
                                                "{} {}",
                                                format_amount(quote.total),
                                                quote.currency
                                            ),
                                        );
                                        ui.weak(format!(
                                            "{}% of list",
                                            quote.price_basis_points / 100
                                        ));
                                    });
                                }
                                Some(Err(error)) => {
                                    ui.colored_label(WARNING, "Unavailable")
                                        .on_hover_text(error);
                                }
                                None => {
                                    ui.weak("Select recipe");
                                }
                            }
                            ui.colored_label(
                                osg_ui::desktop::POSITIVE,
                                rate.public_lanes.to_string(),
                            );
                            ui.label(
                                facility
                                    .summary
                                    .metrics
                                    .total_lanes
                                    .saturating_sub(facility.summary.metrics.busy_lanes)
                                    .to_string(),
                            );
                            ui.label(format!("{} queued", facility.queued_jobs));
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(error) = &view.error {
        ui.colored_label(WARNING, error);
    }
    let Some(facility) = view
        .facilities
        .iter()
        .find(|facility| Some(facility.summary.entity) == state.facility)
    else {
        osg_ui::components::empty_state(
            ui,
            "Select a public facility",
            "Operators publish prices and allocate public production lanes.",
        );
        return;
    };
    ui.add_space(12.);
    egui::Frame::new()
        .fill(osg_ui::desktop::SURFACE_RAISED)
        .inner_margin(14.)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading(&facility.summary.name);
            ui.weak(society::name(
                &model.society.directory,
                facility.summary.owner,
            ));
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(osg_ui::desktop::POSITIVE, "● Accepting outside work");
                ui.label(format!(
                    "{} queued · {} running · {}",
                    facility.queued_jobs, facility.running_jobs, facility.policy.currency
                ));
                if ui
                    .button(egui::RichText::new("+ New job").color(ACCENT))
                    .clicked()
                {
                    state.job_open = true;
                    state.work = state.comparison_work.clone();
                }
            });
        });
    ui.add_space(12.);
    customer_jobs(ui, facility.summary.entity, model, intents);
}

fn job_sheet(
    ui: &mut egui::Ui,
    state: &mut State,
    facility: &PublicFacility,
    payer: Principal,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let view = model.services;
    egui::Frame::new()
        .fill(osg_ui::desktop::SURFACE_RAISED)
        .inner_margin(12.)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.heading("New public job");
                if ui.small_button("Back").clicked() {
                    state.job_open = false;
                    state.work = None;
                }
            });
            ui.weak(&facility.summary.name);

            egui::ScrollArea::vertical()
                .id_salt("public_service_body")
                .max_height((ui.available_height() - 84.).max(0.))
                .show(ui, |ui| {
                    service_work_picker(ui, state, facility, payer, model);
                    if let Some(quote) = current_quote(state, facility, payer, model) {
                        quote_details(ui, quote, model, intents);
                    }
                });

            ui.separator();
            if let Some(quote) = current_quote(state, facility, payer, model) {
                let enough = quote
                    .inputs
                    .iter()
                    .all(|input| available_stock(model, &input.item) >= input.quantity);
                ui.colored_label(
                    ACCENT,
                    format!("Total {} {}", format_amount(quote.total), quote.currency),
                );
                let button = egui::Button::new("Reserve funds & queue")
                    .min_size(egui::vec2(ui.available_width(), 34.));
                let enabled = model.connected && enough && view.available >= quote.total;
                if ui.add_enabled(enabled, button).clicked() {
                    intents.push(Intent::Service(Action::Order(quote.clone())));
                }
            } else {
                ui.weak("Select work to request a quote.");
            }
        });
}

fn current_quote<'a>(
    state: &State,
    facility: &PublicFacility,
    payer: Principal,
    model: &'a FrameModel,
) -> Option<&'a ServiceQuote> {
    model.services.quote.as_ref().filter(|quote| {
        Some(&quote.work) == state.work.as_ref()
            && quote.payer == state.payer.unwrap_or(payer)
            && quote.facility == facility.summary.entity
    })
}

fn service_work_picker(
    ui: &mut egui::Ui,
    state: &mut State,
    facility: &PublicFacility,
    payer: Principal,
    model: &FrameModel,
) {
    ui.weak("Payer & storage owner");
    egui::ComboBox::from_id_salt("service_payer")
        .selected_text(society::name(&model.society.directory, payer))
        .show_ui(ui, |ui| {
            for principal in society::principals(&model.society.directory).filter(|principal| {
                model
                    .society
                    .directory
                    .administers(model.society.account, *principal)
            }) {
                ui.selectable_value(
                    &mut state.payer,
                    Some(principal),
                    society::name(&model.society.directory, principal),
                );
            }
        });
    ui.separator();

    if let Some(catalogue) = &model.industry.catalogue {
        ui.strong("RECIPE / BLUEPRINT");
        for recipe in &catalogue.recipes {
            if !facility
                .policy
                .rates
                .iter()
                .any(|rate| rate.capability == recipe.capability && rate.public_lanes > 0)
            {
                continue;
            }
            let selected = match &state.work {
                Some(ServiceWork::Recipe {
                    recipe: selected, ..
                }) => selected == &recipe.id,
                _ => false,
            };
            if ui.selectable_label(selected, &recipe.name).clicked() {
                state.work = Some(ServiceWork::Recipe {
                    recipe: recipe.id.clone(),
                    batches: 1,
                });
            }
        }

        if facility
            .policy
            .rates
            .iter()
            .any(|rate| rate.capability == IndustryCapability::Shipyard && rate.public_lanes > 0)
        {
            for blueprint in &catalogue.blueprints {
                let work = ServiceWork::Ship {
                    blueprint_hash: *blake3::hash(&blueprint.blueprint).as_bytes(),
                };
                let selected = state.work.as_ref() == Some(&work);
                if ui
                    .selectable_label(selected, format!("Build {}", blueprint.name))
                    .clicked()
                {
                    state.work = Some(work);
                }
            }
        }
    }

    if let Some(ServiceWork::Recipe { batches, .. }) = &mut state.work {
        ui.horizontal(|ui| {
            ui.label("Batches");
            ui.add(egui::DragValue::new(batches).range(1..=MAX_RECIPE_BATCHES));
        });
    }
}

fn available_stock(model: &FrameModel, item: &CargoItem) -> u64 {
    model
        .services
        .stock
        .iter()
        .find(|stock| &stock.item == item)
        .map_or(0, |stock| stock.quantity.saturating_sub(stock.reserved))
}

fn quote_details(
    ui: &mut egui::Ui,
    quote: &ServiceQuote,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    ui.separator();
    ui.strong("Inputs · from your storage here");
    let mut enough = true;
    egui::ScrollArea::horizontal()
        .id_salt("job_material_scroll")
        .show(ui, |ui| {
            egui::Grid::new("public_job_inputs")
                .striped(true)
                .min_row_height(28.)
                .spacing([14., 6.])
                .show(ui, |ui| {
                    for label in ["Material", "Need", "Here", "Short"] {
                        ui.weak(label);
                    }
                    ui.end_row();

                    for input in &quote.inputs {
                        let available = available_stock(model, &input.item);
                        enough &= available >= input.quantity;
                        ui.label(cargo::item_label(&input.item));
                        ui.monospace(input.quantity.to_string());
                        ui.monospace(available.to_string());
                        let color = if available >= input.quantity {
                            osg_ui::desktop::POSITIVE
                        } else {
                            WARNING
                        };
                        ui.colored_label(
                            color,
                            input.quantity.saturating_sub(available).to_string(),
                        );
                        ui.end_row();
                    }
                });
        });

    if !enough {
        ui.colored_label(
            WARNING,
            "Missing materials · bring inputs to this facility's storage.",
        );
        if ui.button("Show in Assets").clicked() {
            intents.push(Intent::OpenAssets);
        }
    }
    for output in &quote.outputs {
        ui.label(format!(
            "Output: {} × {} → your storage",
            output.quantity,
            cargo::item_label(&output.item),
        ));
    }

    ui.add_space(8.);
    ui.strong("PRICE QUOTE");
    let rows = [
        (
            "Energy",
            format!("{} {}", format_amount(quote.energy_charge), quote.currency),
        ),
        (
            "Lane time",
            format!("{} {}", format_amount(quote.time_charge), quote.currency),
        ),
        (
            "Customer rate",
            format!("{}% of list", quote.price_basis_points / 100),
        ),
        (
            "Duration",
            duration(quote.duration_ticks as f64 * osg_model::TICK_SECONDS),
        ),
        (
            "Available",
            format!(
                "{} {}",
                format_amount(model.services.available),
                quote.currency
            ),
        ),
    ];
    egui::ScrollArea::horizontal()
        .id_salt("job_price_scroll")
        .show(ui, |ui| {
            egui::Grid::new("public_job_price")
                .min_col_width(110.)
                .show(ui, |ui| {
                    for (label, value) in rows {
                        ui.weak(label);
                        ui.monospace(value);
                        ui.end_row();
                    }
                    ui.strong("Total");
                    ui.colored_label(
                        ACCENT,
                        format!("{} {}", format_amount(quote.total), quote.currency),
                    );
                    ui.end_row();
                });
        });
    ui.weak("Reserved funds incur daily demurrage. Payment transfers when work starts.");
    ui.weak("Cancelling unstarted work releases its funds and inputs.");
}

fn customer_jobs(ui: &mut egui::Ui, facility: Id, model: &FrameModel, intents: &mut Vec<Intent>) {
    let view = model.services;
    ui.strong("YOUR JOBS");
    if view.jobs.is_empty() {
        ui.weak("No work queued at this facility.");
    }
    for job in &view.jobs {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} · {:?}", job.name, job.status));
            if job.payment.as_ref().is_some_and(|payment| !payment.charged)
                && ui.button("Cancel & release").clicked()
            {
                intents.push(Intent::Service(Action::Cancel(facility, job.id)));
            }
        });
        ui.add(egui::ProgressBar::new(
            job.progress_ticks as f32 / job.duration_ticks.max(1) as f32,
        ));
    }
}
