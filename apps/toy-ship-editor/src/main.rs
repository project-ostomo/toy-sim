mod devices;
mod previews;
mod ui;
mod viewport;
use bevy::prelude::*;
use std::path::PathBuf;
use toy_sim_ship_wasm::ControllerRuntime;
use toy_sim_ships::*;
use toy_sim_ui::bevy_egui::{EguiGlobalSettings, EguiPrimaryContextPass};
#[derive(Resource)]
pub struct Editor {
    pub ship: ShipBlueprint,
    pub layout: Vec<PreparedPart>,
    pub plug: usize,
    pub ghost_pose: Option<PreparedPart>,
    pub catalogue: Catalogue,
    pub descriptions: Vec<toy_sim_ui::parts::PartDescription>,
    pub catalogue_search: String,
    pub category: toy_sim_ui::parts::Category,
    pub inspector: ui::InspectorTab,
    pub undo: Vec<ShipBlueprint>,
    pub redo: Vec<ShipBlueprint>,
    pub selected: Option<u64>,
    pub place: Option<String>,
    pub placement_tanks: Vec<toy_sim_ships::Tank>,
    pub orientation: u8,
    pub revision: u64,
    pub dirty: bool,
    pub file: String,
    pub wasm_file: String,
    pub status: String,
    pub validation: Result<String, String>,
    pub devices_mode: bool,
    pub preview_thrust: f32,
    pub viewport: toy_sim_ui::egui::Rect,
    pub runtime: ControllerRuntime,
    pub sim_path: String,
    pub child: Option<std::process::Child>,
    pub camera_target: Vec3,
    pub camera_yaw: f32,
    pub camera_pitch: f32,
    pub camera_distance: f32,
    pub pixels_per_point: f32,
    pub ghost: Option<PlacedPart>,
    pub ghost_valid: bool,
}
impl Default for Editor {
    fn default() -> Self {
        let catalogue = Catalogue::builtin();
        let descriptions = catalogue
            .parts
            .iter()
            .map(|part| toy_sim_ui::parts::PartDescription::new(part, &catalogue))
            .collect();
        Self {
            ship: ShipBlueprint::default(),
            layout: vec![],
            plug: 0,
            ghost_pose: None,
            catalogue,
            descriptions,
            catalogue_search: String::new(),
            category: default(),
            inspector: ui::InspectorTab::Part,
            undo: vec![],
            redo: vec![],
            selected: None,
            place: None,
            placement_tanks: Vec::new(),
            orientation: 0,
            revision: 1,
            dirty: false,
            file: "ship.ship".into(),
            wasm_file: String::new(),
            status: "Open Starter to try a complete ship, or place a part to begin.".into(),
            validation: Err("Empty assembly".into()),
            devices_mode: false,
            preview_thrust: 1.,
            viewport: toy_sim_ui::egui::Rect::NOTHING,
            runtime: ControllerRuntime::default(),
            sim_path: String::new(),
            child: None,
            camera_target: Vec3::new(0.5, 0.5, 4.5),
            camera_yaw: 0.5,
            camera_pitch: 0.4,
            camera_distance: 18.,
            pixels_per_point: 1.,
            ghost: None,
            ghost_valid: false,
        }
    }
}
impl Editor {
    pub fn active_part(&self) -> Option<(usize, Option<&PlacedPart>)> {
        let placed = self
            .selected
            .and_then(|id| self.ship.parts.iter().find(|part| part.id == id));
        let (prototype, placed) = if let Some(prototype) = &self.place {
            (prototype, None)
        } else {
            let placed = placed?;
            (&placed.prototype, Some(placed))
        };
        let index = self
            .catalogue
            .parts
            .iter()
            .position(|part| &part.id == prototype)?;
        Some((index, placed))
    }

    pub fn pick_up(&mut self, prototype: String) {
        self.place = Some(prototype);
        self.plug = 0;
        self.placement_tanks.clear();
        self.ghost = None;
        self.inspector = ui::InspectorTab::Part;
    }

    pub fn duplicate_for_placement(&mut self, part: &PlacedPart) {
        self.pick_up(part.prototype.clone());
        self.orientation = part.attachment.as_ref().map_or(0, |a| a.roll);
        self.placement_tanks = part.tanks.clone();
    }

    pub fn cancel_placement(&mut self) {
        self.place = None;
        self.placement_tanks.clear();
        self.ghost = None;
    }

    pub fn replace_ship(&mut self, ship: ShipBlueprint) {
        self.edit(|old| *old = ship);
        self.selected = None;
        self.cancel_placement();
        self.orientation = 0;
        self.frame_ship();
    }

    pub fn frame_ship(&mut self) {
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        for part in &self.layout {
            let (lo, hi) = part.bounds();
            minimum = minimum.min(lo.as_vec3());
            maximum = maximum.max(hi.as_vec3());
        }
        if minimum.is_finite() && maximum.is_finite() {
            self.camera_target = (minimum + maximum) * 0.5;
            self.camera_distance = ((maximum - minimum).length() * 1.6).max(18.);
        }
    }

    pub fn edit(&mut self, f: impl FnOnce(&mut ShipBlueprint)) {
        self.undo.push(self.ship.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        f(&mut self.ship);
        self.changed();
    }
    pub fn changed(&mut self) {
        self.ghost = None;
        if self
            .selected
            .is_some_and(|id| !self.ship.parts.iter().any(|part| part.id == id))
        {
            self.selected = None;
        }
        self.revision += 1;
        self.dirty = true;
        self.validate();
    }
    pub fn validate(&mut self) {
        self.layout = self.ship.layout(&self.catalogue).unwrap_or_default();
        self.validation = self
            .ship
            .compile(&self.catalogue)
            .and_then(|d| {
                self.runtime
                    .validate_program(self.ship.controller_bytes())?;
                Ok(format!(
                    "{} parts · {:.1} kg dry · {:.2} m³ cargo · {:.1} m³ tank space · {:.0} hull",
                    d.parts.len(),
                    d.dry_mass,
                    d.capacity_m3,
                    d.parts
                        .iter()
                        .map(|part| part.definition.tank_volume_m3)
                        .sum::<f64>()
                        .max(0.),
                    d.hull
                ))
            })
            .map_err(|e| format!("{e:#}"));
    }
    pub fn add_part(&mut self, mut part: PlacedPart) {
        let Some(id) = self
            .ship
            .parts
            .iter()
            .map(|p| p.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.status = "Part ID space exhausted".into();
            return;
        };
        part.id = id;
        if self
            .catalogue
            .part(&part.prototype)
            .is_some_and(|p| p.equipment.device_kind().is_some())
        {
            let base = format!("{}_{}", part.prototype, id).replace('-', "_");
            part.alias = base.clone();
            let mut suffix = 1;
            while self.ship.parts.iter().any(|p| p.alias == part.alias) {
                part.alias = format!("{base}_{suffix}");
                suffix += 1;
            }
        } else {
            part.alias.clear();
        }
        self.edit(|s| s.parts.push(part));
        self.selected = Some(id);
    }
    pub fn delete_part(&mut self, id: u64) {
        self.edit(|s| {
            let mut removed = std::collections::BTreeSet::from([id]);
            loop {
                let before = removed.len();
                for part in &s.parts {
                    if part
                        .attachment
                        .as_ref()
                        .is_some_and(|a| removed.contains(&a.parent))
                    {
                        removed.insert(part.id);
                    }
                }
                if before == removed.len() {
                    break;
                }
            }
            s.parts.retain(|p| !removed.contains(&p.id));
            s.avionics
                .excluded_actuators
                .retain(|p| !removed.contains(p));
        });
        self.selected = None;
    }
    pub fn launch(&mut self) -> anyhow::Result<()> {
        self.ship.compile(&self.catalogue)?;
        self.runtime
            .validate_program(self.ship.controller_bytes())?;
        let dir = std::env::temp_dir().join("toy-ship-editor");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!(
            "launch-{}-{}.ship",
            std::process::id(),
            self.revision
        ));
        self.ship.save(&path)?;
        let binary = if self.sim_path.is_empty() {
            std::env::current_exe()?.with_file_name(if cfg!(windows) {
                "toy-sim-debug.exe"
            } else {
                "toy-sim-debug"
            })
        } else {
            PathBuf::from(&self.sim_path)
        };
        anyhow::ensure!(
            binary.is_file(),
            "Build toy-sim-debug and toy-sim-server first, or set its executable path in the inspector"
        );
        self.child = Some(
            std::process::Command::new(binary)
                .arg("--ship")
                .arg(&path)
                .spawn()?,
        );
        Ok(())
    }
}
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--example") {
        let path = args
            .get(2)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("starter.ship"));
        starter(EXAMPLE_CONTROLLER.to_vec()).save(path)?;
        return Ok(());
    }
    if args.get(1).is_some_and(|a| a == "--micropulse-example") {
        let path = args
            .get(2)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("micropulse-demo.ship"));
        micropulse_starter().save(path)?;
        return Ok(());
    }
    if args.get(1).is_some_and(|a| a == "--validate") {
        let s = ShipBlueprint::load(
            args.get(2)
                .ok_or_else(|| anyhow::anyhow!("--validate requires a path"))?,
        )?;
        let d = s.compile(&Catalogue::builtin())?;
        ControllerRuntime::new()?.validate_program(s.controller_bytes())?;
        println!(
            "{}: {} parts, {:.1} kg dry",
            s.name,
            d.parts.len(),
            d.dry_mass
        );
        return Ok(());
    }
    let mut editor = Editor::default();
    if let Some(path) = args.get(1) {
        anyhow::ensure!(
            !path.to_string_lossy().starts_with("--"),
            "unknown editor argument"
        );
        editor.ship = ShipBlueprint::load(path)?;
        editor.file = path.to_string_lossy().into_owned();
        editor.validate();
        editor.frame_ship();
    }
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Ship Editor".into(),
                        resolution: (1400, 900).into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(toy_sim_ui::UiPlugin)
        .add_plugins(toy_sim_ship_view::plume::PlumePlugin)
        .add_plugins(toy_sim_ship_view::mechanisms::MechanismPlugin)
        .add_systems(
            Update,
            |time: Res<Time>, mut clock: ResMut<toy_sim_ship_view::mechanisms::MechanismTime>| {
                clock.0 = time.elapsed_secs_f64();
            },
        )
        .insert_resource(EguiGlobalSettings {
            enable_absorb_bevy_input_system: true,
            auto_create_primary_context: false,
            ..default()
        })
        .insert_resource(editor)
        .insert_resource(ClearColor(Color::srgb(0.035, 0.045, 0.065)))
        .add_systems(
            Startup,
            (
                toy_sim_ship_view::prepare_visuals,
                viewport::setup,
                previews::setup,
            )
                .chain(),
        )
        .add_systems(EguiPrimaryContextPass, ui::editor)
        .add_systems(
            PostUpdate,
            (
                viewport::camera,
                viewport::visuals,
                viewport::ghost,
                toy_sim_ship_view::add_weapon_visuals,
                previews::activity,
                previews::fit_and_isolate,
            )
                .chain()
                .after(toy_sim_ui::bevy_egui::EguiPostUpdateSet::EndPass)
                .before(TransformSystems::Propagate),
        )
        .run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tank_edits_survive_undo_and_duplicate_placement_resets_on_pickup() {
        let mut editor = Editor::default();
        let mut ship = starter(EXAMPLE_CONTROLLER.to_vec());
        ship.parts.truncate(1);
        ship.parts[0].prototype = "fuselage_8m".into();
        ship.parts[0].tanks = vec![Tank {
            resource: "fuel".into(),
            volume_m3: 50.,
            initial_fill: 0.5,
        }];
        editor.replace_ship(ship);
        let installed = editor.ship.parts[0].clone();
        editor.duplicate_for_placement(&installed);
        assert_eq!(editor.placement_tanks, installed.tanks);
        editor.edit(|ship| ship.parts[0].tanks[0].volume_m3 = 100.);
        editor.ship = editor.undo.pop().unwrap();
        editor.changed();
        assert_eq!(editor.ship.parts[0].tanks, installed.tanks);
        assert!(editor.validation.is_ok());
        editor.pick_up("fuselage_8m".into());
        assert!(editor.placement_tanks.is_empty());
    }

    #[test]
    fn catalogue_pickup_preserves_selection_without_editing_the_blueprint() {
        let mut editor = Editor::default();
        editor.replace_ship(starter(EXAMPLE_CONTROLLER.to_vec()));
        let selected = editor.ship.parts[0].id;
        editor.selected = Some(selected);
        editor.dirty = false;
        let before = editor.ship.to_bytes().unwrap();
        let revision = editor.revision;

        editor.pick_up("battery".into());
        let (index, installed) = editor.active_part().unwrap();
        assert_eq!(editor.catalogue.parts[index].id, "battery");
        assert!(installed.is_none());
        assert_eq!(editor.selected, Some(selected));
        assert_eq!(editor.revision, revision);
        assert!(!editor.dirty);
        assert_eq!(editor.ship.to_bytes().unwrap(), before);

        editor.cancel_placement();
        assert_eq!(editor.active_part().unwrap().1.unwrap().id, selected);
        editor.pick_up("engine".into());
        editor.replace_ship(ShipBlueprint::default());
        assert!(editor.selected.is_none());
        assert!(editor.place.is_none());
        assert!(editor.active_part().is_none());
    }

    #[test]
    fn bundled_armed_starter_validates_against_current_catalogue_and_abi() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/ships/starter.ship");
        let ship = ShipBlueprint::load(path).unwrap();
        let design = ship.compile(&Catalogue::builtin()).unwrap();
        assert_eq!(design.weapon_specs.len(), 2);
        ControllerRuntime::new()
            .unwrap()
            .validate_program(ship.controller_bytes())
            .unwrap();
    }

    #[test]
    fn assembly_deletion_and_undo_restore_self_contained_design() {
        let mut e = Editor::default();
        e.edit(|s| *s = starter(EXAMPLE_CONTROLLER.to_vec()));
        assert!(e.validation.is_ok(), "{:?}", e.validation);
        let before = e.ship.to_bytes().unwrap();
        e.delete_part(9);
        assert!(e.ship.parts.iter().all(|p| p.id != 9));
        e.ship = e.undo.pop().unwrap();
        e.changed();
        assert_eq!(e.ship.to_bytes().unwrap(), before);
        assert!(e.validation.is_ok(), "{:?}", e.validation);
        let path =
            std::env::temp_dir().join(format!("toy-ship-roundtrip-{}.ship", std::process::id()));
        e.ship.save(&path).unwrap();
        let reopened = ShipBlueprint::load(&path).unwrap();
        assert_eq!(reopened.to_bytes().unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }
}
