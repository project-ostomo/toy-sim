pub(super) mod construction;

use super::*;
use bevy::tasks::{IoTaskPool, Task, futures::check_ready};
use industry_model::{
    BlueprintView, CargoItem, FacilityView, IndustryCapability, IndustryCommand, ItemStack,
    JobStatus,
};

#[derive(Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Factory,
    Shipyard,
    Jobs,
    Storage,
}

#[derive(Default)]
pub(super) struct State {
    pub facility: Option<Id>,
    pub directory_after: Option<Id>,
    previous_pages: Vec<Option<Id>>,
    tab: Tab,
    recipe: Option<String>,
    blueprint: usize,
    batches: u32,
    owner: Option<ownership::Principal>,
    search: String,
    import_path: String,
    imported: Option<Result<BlueprintView, String>>,
    importing: Option<Task<Result<BlueprintView, String>>>,
    pub construction: construction::State,
    cargo: cargo::PaneState,
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    if let Some(task) = &mut state.importing {
        if let Some(result) = check_ready(task) {
            state.imported = Some(result);
            state.importing = None;
        }
    }

    ui.horizontal(|ui| {
        ui.label(Icon::Industry.text(22.0).color(ACCENT));
        ui.strong("INDUSTRY NETWORK");
        ui.weak("Remote management · physical deliveries");
    });
    if let Some(error) = &model.industry.error {
        ui.colored_label(THREAT, error);
    }
    state.construction.draw(ui, model.connected, intents);
    let facilities: Vec<_> = model
        .industry
        .directory
        .iter()
        .filter(|facility| !facility.capabilities.is_empty())
        .collect();
    if state.facility.is_none() {
        state.facility = facilities.first().map(|facility| facility.entity);
    }
    ui.horizontal(|ui| {
        let name = state
            .facility
            .and_then(|id| {
                model
                    .industry
                    .directory
                    .iter()
                    .find(|facility| facility.entity == id)
                    .map(|facility| facility.name.as_str())
                    .or_else(|| cargo::facility(model, id).map(|facility| facility.name.as_str()))
            })
            .unwrap_or("Select a facility");
        egui::ComboBox::from_id_salt("industry_facility")
            .selected_text(name)
            .width(310.0)
            .show_ui(ui, |ui| {
                for facility in &facilities {
                    ui.selectable_value(&mut state.facility, Some(facility.entity), &facility.name);
                }
            });
        if ui
            .add_enabled(
                !state.previous_pages.is_empty(),
                egui::Button::new("Previous"),
            )
            .clicked()
        {
            state.directory_after = state.previous_pages.pop().flatten();
        }
        if ui
            .add_enabled(
                model.industry.directory_next.is_some(),
                egui::Button::new("Next"),
            )
            .clicked()
        {
            state.previous_pages.push(state.directory_after);
            state.directory_after = model.industry.directory_next;
        }
    });
    let Some(facility) = state.facility.and_then(|id| cargo::facility(model, id)) else {
        ui.weak(if state.facility.is_some() {
            "Loading facility inventory and jobs…"
        } else {
            "No authorized facilities on this directory page."
        });
        return;
    };
    ui.horizontal(|ui| {
        ui.label(society::name(&model.society.directory, facility.owner));
        if !facility.can_manage {
            ui.colored_label(MUTED, "VIEW ONLY");
        }
        if ui.button("Open cargo").clicked() {
            intents.push(Intent::InspectInventory(facility.entity));
        }
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.tab, Tab::Factory, "Production");
        ui.selectable_value(&mut state.tab, Tab::Shipyard, "Shipyard");
        ui.selectable_value(&mut state.tab, Tab::Storage, "Storage");
        ui.selectable_value(
            &mut state.tab,
            Tab::Jobs,
            format!("Jobs ({})", facility.jobs.len()),
        );
    });
    ui.separator();
    if state.tab == Tab::Storage {
        ui.weak("Facility storage · production inputs and finished goods");
        cargo::draw(
            ui,
            &mut state.cargo,
            facility.entity,
            model,
            transfers,
            intents,
        );
        return;
    }
    let height = ui.available_height().max(0.0);
    egui::ScrollArea::vertical()
        .id_salt("industry_content")
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_height(height)
        .show(ui, |ui| {
            capabilities(ui, facility);
            ui.separator();
            match state.tab {
                Tab::Factory => production(ui, state, model, facility, intents),
                Tab::Shipyard => shipyard(ui, state, model, facility, intents),
                Tab::Jobs => jobs(ui, model, facility, intents),
                Tab::Storage => unreachable!(),
            }
        });
}

fn capability_name(capability: IndustryCapability) -> &'static str {
    match capability {
        IndustryCapability::Refinery => "Refinery",
        IndustryCapability::FuelPlant => "Fuel plant",
        IndustryCapability::Fabricator => "Fabricator",
        IndustryCapability::Shipyard => "Shipyard",
    }
}

fn capabilities(ui: &mut egui::Ui, facility: &FacilityView) {
    for module in &facility.capabilities {
        ui.horizontal(|ui| {
            ui.colored_label(
                if module.operational { ACCENT } else { THREAT },
                if module.operational { "●" } else { "○" },
            );
            ui.label(format!(
                "{} · {} lanes · {:.1} MW / lane",
                capability_name(module.capability),
                module.lanes,
                module.power_per_lane_w as f64 / 1e6
            ));
            if let Some(radius) = module.max_radius_m {
                ui.weak(format!("max hull radius {}", distance(radius)));
            }
            if !module.operational {
                ui.colored_label(THREAT, "Unavailable");
            }
        });
    }
}

fn production(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    facility: &FacilityView,
    intents: &mut Vec<Intent>,
) {
    let Some(catalogue) = &model.industry.catalogue else {
        ui.weak("Loading manufacturing catalogue…");
        return;
    };
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Filter recipes…")
            .desired_width(f32::INFINITY),
    );
    let search = state.search.to_lowercase();
    let recipes: Vec<_> = catalogue
        .recipes
        .iter()
        .filter(|recipe| {
            facility
                .capabilities
                .iter()
                .any(|module| module.capability == recipe.capability)
                && recipe.name.to_lowercase().contains(&search)
        })
        .collect();
    if !recipes
        .iter()
        .any(|recipe| state.recipe.as_deref() == Some(&recipe.id))
    {
        state.recipe = recipes.first().map(|recipe| recipe.id.clone());
    }
    let Some(recipe) = recipes
        .iter()
        .find(|recipe| state.recipe.as_deref() == Some(&recipe.id))
    else {
        ui.weak("No matching recipes for the installed modules.");
        return;
    };
    egui::ComboBox::from_id_salt("industry_recipe")
        .selected_text(&recipe.name)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for recipe in &recipes {
                ui.selectable_value(&mut state.recipe, Some(recipe.id.clone()), &recipe.name);
            }
        });
    state.batches = state.batches.max(1);
    ui.horizontal(|ui| {
        ui.label("Batches");
        ui.add(
            egui::DragValue::new(&mut state.batches).range(1..=industry_model::MAX_RECIPE_BATCHES),
        );
    });
    ui.label(format!(
        "{} · {:.2} MJ process energy",
        duration(recipe.duration_ticks as f64 * 0.1 * f64::from(state.batches)),
        recipe.energy_j as f64 * f64::from(state.batches) / 1e6
    ));
    ui.strong("Inputs reserved when the job starts");
    let enough = requirements(ui, &recipe.inputs, facility, u64::from(state.batches));
    ui.strong("Outputs");
    outputs(ui, &recipe.outputs, facility, u64::from(state.batches));
    if recipe.stored_energy_j > 0 {
        ui.small(format!(
            "Output energy content: {:.2} MJ per batch",
            recipe.stored_energy_j as f64 / 1e6
        ));
    }
    if ui
        .add_enabled(
            model.connected && facility.can_manage && enough,
            egui::Button::new("Start production"),
        )
        .clicked()
    {
        intents.push(Intent::Industry(
            IndustryCommand::StartRecipe {
                facility: facility.entity,
                recipe: recipe.id.clone(),
                batches: state.batches,
            },
            "Start production",
        ));
        state.tab = Tab::Jobs;
    }
}

fn shipyard(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    facility: &FacilityView,
    intents: &mut Vec<Intent>,
) {
    if !facility
        .capabilities
        .iter()
        .any(|module| module.capability == IndustryCapability::Shipyard)
    {
        ui.weak("This facility has no shipyard module.");
        return;
    }
    let Some(catalogue) = &model.industry.catalogue else {
        ui.weak("Loading ship blueprints…");
        return;
    };
    let mut owners = vec![facility.owner];
    if facility.can_transfer {
        owners.push(ownership::Principal::Player(model.society.account));
        owners.extend(
            model
                .society
                .directory
                .organizations
                .keys()
                .copied()
                .map(ownership::Principal::Organization)
                .filter(|owner| {
                    model
                        .society
                        .directory
                        .administers(model.society.account, *owner)
                }),
        );
    }
    if facility.can_transfer {
        owners.extend(
            model
                .society
                .directory
                .sovereignties
                .keys()
                .copied()
                .map(ownership::Principal::Sovereignty)
                .filter(|owner| {
                    model
                        .society
                        .directory
                        .administers(model.society.account, *owner)
                }),
        );
    }
    owners.sort();
    owners.dedup();
    if state.owner.is_none_or(|owner| !owners.contains(&owner)) {
        state.owner = Some(if facility.can_transfer {
            ownership::Principal::Player(model.society.account)
        } else {
            facility.owner
        });
    }
    egui::ComboBox::from_id_salt("constructed_owner")
        .selected_text(society::name(
            &model.society.directory,
            state.owner.unwrap(),
        ))
        .show_ui(ui, |ui| {
            for owner in owners {
                ui.selectable_value(
                    &mut state.owner,
                    Some(owner),
                    society::name(&model.society.directory, owner),
                );
            }
        });
    state.blueprint = state
        .blueprint
        .min(catalogue.blueprints.len().saturating_sub(1));
    if let Some(blueprint) = catalogue.blueprints.get(state.blueprint) {
        egui::ComboBox::from_id_salt("construction_blueprint")
            .selected_text(&blueprint.name)
            .show_ui(ui, |ui| {
                for (index, blueprint) in catalogue.blueprints.iter().enumerate() {
                    ui.selectable_value(&mut state.blueprint, index, &blueprint.name);
                }
            });
        blueprint_controls(
            ui,
            blueprint,
            state.owner.unwrap(),
            model,
            facility,
            state.construction.busy(),
            intents,
        );
    }
    ui.separator();
    ui.label("Import a .ship design");
    ui.weak("Drop a ship file here, or enter its local path.");
    let dropped = ui.input(|input| input.raw.dropped_files.clone());
    for file in dropped {
        let path = file.path().to_owned();
        state.import_path = path.display().to_string();
        state.imported = None;
        state.importing = Some(IoTaskPool::get().spawn(async move { import_blueprint(&path) }));
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.import_path).hint_text("/path/to/design.ship"),
        );
        if ui
            .add_enabled(state.importing.is_none(), egui::Button::new("Load design"))
            .clicked()
        {
            let path = std::path::PathBuf::from(&state.import_path);
            state.imported = None;
            state.importing = Some(IoTaskPool::get().spawn(async move { import_blueprint(&path) }));
        }
    });
    if state.importing.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading and validating ship design…");
        });
    }
    if let Some(imported) = &state.imported {
        match imported {
            Ok(blueprint) => blueprint_controls(
                ui,
                blueprint,
                state.owner.unwrap(),
                model,
                facility,
                state.construction.busy(),
                intents,
            ),
            Err(error) => {
                ui.colored_label(THREAT, error);
            }
        }
    }
}

fn import_blueprint(path: &std::path::Path) -> Result<BlueprintView, String> {
    let result = (|| -> anyhow::Result<BlueprintView> {
        let blueprint = osg_ships::ShipBlueprint::load(path)?;
        let catalogue = osg_ships::Catalogue::builtin();
        let design = blueprint.compile(&catalogue)?;
        let bill = osg_ships::industry::construction_requirements(&design, &catalogue)?;
        let bytes = blueprint.to_bytes()?;
        anyhow::ensure!(
            bytes.len() <= osg_ships::MAX_FILE,
            "Design exceeds the 16 MiB ship file limit"
        );
        Ok(BlueprintView {
            name: blueprint.name.clone(),
            blueprint: bytes,
            inputs: bill.inputs,
            duration_ticks: bill.duration_ticks,
            energy_j: bill.energy_j,
        })
    })();
    result.map_err(|error| error.to_string())
}

fn blueprint_controls(
    ui: &mut egui::Ui,
    blueprint: &BlueprintView,
    owner: ownership::Principal,
    model: &FrameModel,
    facility: &FacilityView,
    uploading: bool,
    intents: &mut Vec<Intent>,
) {
    ui.strong(&blueprint.name);
    ui.label(format!(
        "{} · {:.2} MJ assembly energy",
        duration(blueprint.duration_ticks as f64 * 0.1),
        blueprint.energy_j as f64 / 1e6
    ));
    let enough = requirements(ui, &blueprint.inputs, facility, 1);
    ui.weak("Delivered into this station's hangar with empty tanks and batteries. Refill and request dock power before undocking.");
    if ui
        .add_enabled(
            model.connected && facility.can_manage && enough && !uploading,
            egui::Button::new("Build ship"),
        )
        .clicked()
    {
        intents.push(Intent::BuildShip(construction::Request {
            facility: facility.entity,
            facility_name: facility.name.clone(),
            owner,
            name: blueprint.name.clone(),
            bytes: blueprint.blueprint.clone().into(),
        }));
    }
}

fn item_name(item: &CargoItem, facility: &FacilityView) -> String {
    facility
        .items
        .iter()
        .find(|stack| &stack.item == item)
        .map(|stack| stack.name.clone())
        .unwrap_or_else(|| match item {
            CargoItem::Resource(id) => id.replace('_', " "),
            CargoItem::Part(id) => format!("{} kit", id.replace('_', " ")),
        })
}

fn quantity_label(item: &CargoItem, quantity: u64, facility: &FacilityView) -> String {
    static CATALOGUE: std::sync::OnceLock<osg_ships::Catalogue> = std::sync::OnceLock::new();
    let mass = facility
        .items
        .iter()
        .find(|stack| &stack.item == item)
        .map(|stack| stack.unit_mass_kg)
        .or_else(|| {
            osg_ships::industry::item_mass_kg(
                item,
                CATALOGUE.get_or_init(osg_ships::Catalogue::builtin),
            )
            .ok()
        })
        .unwrap_or(1.0);
    cargo::quantity_label(item, quantity, mass)
}

fn requirements(
    ui: &mut egui::Ui,
    inputs: &[ItemStack],
    facility: &FacilityView,
    batches: u64,
) -> bool {
    let mut enough = true;
    for input in inputs {
        let quantity = input.quantity.checked_mul(batches);
        let available = facility
            .items
            .iter()
            .find(|stack| stack.item == input.item)
            .map_or(0, cargo::available);
        let satisfied = quantity.is_some_and(|quantity| available >= quantity);
        enough &= satisfied;
        ui.horizontal(|ui| {
            ui.label(
                match input.item {
                    CargoItem::Resource(_) => Icon::Cargo,
                    CargoItem::Part(_) => Icon::Settings,
                }
                .text(16.0)
                .color(ACCENT),
            );
            ui.label(item_name(&input.item, facility));
            ui.colored_label(
                if satisfied { TEXT } else { THREAT },
                format!(
                    "{} required / {} available",
                    quantity.map_or("Too many".into(), |value| quantity_label(
                        &input.item,
                        value,
                        facility
                    )),
                    quantity_label(&input.item, available, facility),
                ),
            )
            .on_hover_text(format!(
                "{} required / {available} available inventory units",
                quantity.map_or("Overflow".into(), |value| value.to_string())
            ));
        });
    }
    enough
}

fn outputs(ui: &mut egui::Ui, outputs: &[ItemStack], facility: &FacilityView, batches: u64) {
    for output in outputs {
        ui.label(format!(
            "{} × {}",
            quantity_label(
                &output.item,
                output.quantity.saturating_mul(batches),
                facility
            ),
            item_name(&output.item, facility)
        ));
    }
}

fn jobs(ui: &mut egui::Ui, model: &FrameModel, facility: &FacilityView, intents: &mut Vec<Intent>) {
    if facility.jobs.is_empty() {
        ui.weak("No queued production or construction jobs.");
    }
    for job in &facility.jobs {
        ui.horizontal(|ui| {
            ui.strong(&job.name);
            if ui
                .add_enabled(
                    model.connected && facility.can_manage,
                    egui::Button::new("Cancel"),
                )
                .clicked()
            {
                intents.push(Intent::Industry(
                    IndustryCommand::CancelJob {
                        facility: facility.entity,
                        job: job.id,
                    },
                    "Cancel job",
                ));
            }
        });
        let status = match job.status {
            JobStatus::Queued => "Queued",
            JobStatus::Running => "Running",
            JobStatus::AwaitingPower => "Waiting for power",
            JobStatus::AwaitingCargoSpace => "Complete · waiting for cargo space",
            JobStatus::AwaitingBerth => "Complete · waiting for hangar capacity",
            JobStatus::ModuleUnavailable => "Installed module unavailable",
        };
        let fraction = job.progress_ticks as f32 / job.duration_ticks.max(1) as f32;
        ui.add(
            egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                .text(format!(
                    "{status} · {} remaining",
                    duration(job.duration_ticks.saturating_sub(job.progress_ticks) as f64 * 0.1)
                ))
                .fill(if job.status == JobStatus::Running {
                    ACCENT
                } else {
                    MUTED
                }),
        );
        ui.small(format!(
            "Power {:.2} / {:.2} MW · {}",
            job.supplied_power_w as f64 / 1e6,
            job.requested_power_w as f64 / 1e6,
            society::name(&model.society.directory, job.owner)
        ));
        ui.separator();
    }
}

fn duration(seconds: f64) -> String {
    if seconds >= 3600.0 {
        format!("{:.1} h", seconds / 3600.0)
    } else if seconds >= 60.0 {
        format!("{:.1} min", seconds / 60.0)
    } else {
        format!("{seconds:.0} s")
    }
}

#[cfg(test)]
mod tests;
