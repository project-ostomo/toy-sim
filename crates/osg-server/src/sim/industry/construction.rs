use super::*;
use crate::blueprint_uploads::BlueprintUploads;
use osg_ships::ShipBlueprint;

pub fn prepare_work(
    work: &ServiceWork,
    allow_catalogue: bool,
    uploads: &BlueprintUploads,
    manufacturing: &ManufacturingCatalogue,
    catalogue: &Catalogue,
    runtime: &mut osg_ship_wasm::ControllerRuntime,
) -> Result<WorkPlan> {
    match work {
        ServiceWork::Recipe { recipe, batches } => {
            let recipe = manufacturing
                .0
                .recipes
                .iter()
                .find(|entry| entry.id == *recipe)
                .context("recipe unavailable")?;
            WorkPlan::recipe(recipe, *batches)
        }
        ServiceWork::Ship { blueprint_hash } => {
            let builtin =
                allow_catalogue
                    .then(|| {
                        manufacturing.0.blueprints.iter().find(|entry| {
                            blake3::hash(&entry.blueprint).as_bytes() == blueprint_hash
                        })
                    })
                    .flatten();
            let bytes: Arc<[u8]> = if let Some(blueprint) = builtin {
                blueprint.blueprint.clone().into()
            } else {
                Arc::from(uploads.get(*blueprint_hash)?.as_ref().as_ref())
            };
            prepare_blueprint(bytes, catalogue, runtime)
        }
    }
}

pub fn prepare_blueprint(
    bytes: Arc<[u8]>,
    catalogue: &Catalogue,
    runtime: &mut osg_ship_wasm::ControllerRuntime,
) -> Result<WorkPlan> {
    ensure!(
        bytes.len() <= osg_ships::MAX_FILE,
        "ship blueprint too large"
    );
    let blueprint = ShipBlueprint::from_bytes(&bytes)?;
    let design = blueprint.compile(catalogue)?;
    runtime.validate_program(blueprint.controller_bytes())?;
    let requirements = osg_ships::industry::construction_requirements(&design, catalogue)?;
    Ok(WorkPlan {
        name: blueprint.name.clone(),
        capability: IndustryCapability::Shipyard,
        inputs: requirements.inputs,
        output: WorkOutput::Ship(bytes),
        duration_ticks: requirements.duration_ticks,
        energy_j: requirements.energy_j,
        stored_energy_j: 0,
        required_radius_m: design.radius,
        construction: Some(ConstructionDesign {
            blueprint: Arc::new(blueprint),
            design: Arc::new(design),
        }),
    })
}

pub fn dry_mass(work: &WorkPlan, catalogue: &Catalogue) -> Result<f64> {
    Ok(osg_ships::industry::stack_mass_mg(&work.inputs, catalogue)? as f64 / 1_000_000.)
}
