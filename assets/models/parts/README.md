# Part models

This directory contains optional 3D models of ship parts. The fuselage section and end use GLB models; other bundled parts use coloured boxes, with separate barrel geometry for weapons.

## Fuselage kit

| Catalogue part | Model | Dimensions |
| --- | --- | --- |
| 8m fuselage section | `fuselage-8m.glb` | 8 × 8 × 16 m |
| 8m fuselage end | `fuselage-end-8m.glb` | 8 × 8 × 2 m |
| 4m fuselage section | `fuselage-8m.glb`, scale 0.5 | 4 × 4 × 8 m |
| 2m fuselage section | `fuselage-8m.glb`, scale 0.25 | 2 × 2 × 4 m |
| 4m fuselage end | `fuselage-end-8m.glb`, scale 0.5 | 4 × 4 × 1 m |
| 2m fuselage end | `fuselage-end-8m.glb`, scale 0.25 | 2 × 2 × 0.5 m |

All fuselage variants are structural parts. Their mass and hull ratings are provisional gameplay values; the 8m parts provide 500 m³ of tank space for the section and 40 m³ for the end. Tank capacity scales with volume, while shell mass and hull ratings scale with area. The section repeats every 16 m along local Z. The end has its mounting face at +Z and its closed face at −Z; rotate it 180 degrees around X or Y for the opposite end.

`fuselage-08m-16m.blend` contains the editable section, end, and assembly preview scenes. The outer Whipple sheets use constant base colour, metalness and roughness, with no surface texture. The spacing between shield layers is actual geometry. Each GLB contains one mesh with three material primitives.

To regenerate the end geometry and export both parts from the saved Blender source:

```sh
blender --background assets/models/parts/fuselage-08m-16m.blend --python tools/art/fuselage.py
```

The script preserves the section geometry, applies modifiers to temporary export copies, and triangulates them before exporting normals and tangents. Axis conversion is disabled deliberately so the Blender Z axis remains the game's part-local Z axis. The editable source stays separate from the joined export meshes.

## Micropulse engine asset

`micropulse-engine-8m.blend` contains the editable `Micropulse drive 8m` scene. Its 8 × 8 × 12 m envelope has a mounting face at Z = −6 m and an exhaust opening at Z = +6 m. The octagonal housing contains the machinery behind an aft shadow shield. The exposed section has a reaction chamber, throat coil, four expanding nozzle coils, eight structural supports, and coolant/feed lines.

`micropulse-engine-8m.glb` is the joined export with five material primitives. The catalogue uses this model for micropulse engines at scales 1, 0.5, 0.25, and 0.125, giving widths of 8m, 4m, 2m, and 1m. All consume complete micropulse charges and provide thrust, electrical recovery, and absorbed heat; see [ships.md](../../../docs/ships.md). To regenerate it in an interactive Blender session, execute `tools/art/micropulse_engine.py` with its absolute path supplied as `__file__`. The script rebuilds its own scene and saves the engine source without overwriting the fuselage source.

## How a part selects a model

A part definition in [crates/toy-sim-ships/data/catalogue.toml](../../../crates/toy-sim-ships/data/catalogue.toml) may set an optional `model` string:

```toml
[[parts]]
id = "engine"
title = "Electric propellant engine"
dimensions = [10,10,20]
model = "models/parts/engine.glb"
# ...
```

When `model` is set, the simulator ([toy-sim-ship-view/src/lib.rs](../../../crates/toy-sim-ship-view/src/lib.rs)) and the editor viewport ([viewport.rs](../../../apps/toy-ship-editor/src/viewport.rs)) load `<model>#Scene0` through Bevy's `AssetServer`. The path is relative to the `assets/` directory. When `model` is absent, they use a cuboid mesh sized to `dimensions × 0.1 m` and a material coloured by `color`.

## Conventions the code expects

- **Origin and axes.** The scene is spawned at the part's centre, with the part's placement rotation applied. Author the model centred on its origin in part-local axes.
- **Size.** One model unit is one metre before applying the optional positive, finite `model_scale` (default 1.0). Scaling is uniform. For example, `model = "models/parts/micropulse-engine-8m.glb"` with `model_scale = 0.5` produces a 4 × 4 × 6 m visual; author `dimensions = [40, 40, 60]` for its matching part box. Dimensions, mass, performance, plume origins and plume sizes remain explicit catalogue values. The scale applies only to the imported model child in the shared renderer, including editor previews and client views.
- **Engine direction.** Engines push along part-local −Z, and the exhaust plume points along +Z from `plume.origin_m`.
- **Weapons.** Assembly views, catalogue thumbnails and placement previews all add barrel visuals separately from the weapon definition ([weapon.rs](../../../crates/toy-sim-ship-view/src/weapon.rs)). Muzzles fire along the barrel's −Z after yaw and pitch.
- **Scene.** Only the first scene, `Scene0`, is used.

## What a model does not change

Models are visual only. Mass, inertia, collision geometry, exposed area, connectivity and overlap checks all use the box given by `dimensions` ([design.rs](../../../crates/toy-sim-ships/src/design.rs), [collision/mod.rs](../../../crates/toy-sim-server/src/sim/physics/collision/mod.rs)). Explosion fragments are also boxes coloured with `color`.

The catalogue is compiled into the binaries with `include_str!`. After editing it, rebuild both applications. See [docs/asset-workflow.md](../../../docs/asset-workflow.md) for the full editing workflow.

## Equipment library

`parts-catalogue.blend` contains the complete equipment model library and an overview scene. `tools/art/catalogue.py` rebuilds its scenes and exports the GLBs. `catalogue-models.json` records each model's intended dimensions and measured bounds. Scaled catalogue entries reuse these models. See [the equipment catalogue](../../../docs/parts-catalogue.md) for gameplay and example ships.
