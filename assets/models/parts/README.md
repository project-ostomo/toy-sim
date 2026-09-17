# Part models

This directory is the intended location for optional 3D models of ship parts. No part in the bundled catalogue uses a model at present, so every part renders as a coloured box.

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
- **Size.** The model is not scaled. One unit is one metre. The part box spans `dimensions[i] × 0.1` metres on each axis.
- **Engine direction.** Engines push along part-local −Z, and the exhaust plume points along +Z from `plume.origin_m`.
- **Weapons.** Barrel visuals are added separately from the weapon definition ([weapon.rs](../../../crates/toy-sim-ship-view/src/weapon.rs)). Muzzles fire along the barrel's −Z after yaw and pitch.
- **Scene.** Only the first scene, `Scene0`, is used.

## What a model does not change

Models are visual only. Mass, inertia, collision geometry, exposed area, connectivity and overlap checks all use the box given by `dimensions` ([design.rs](../../../crates/toy-sim-ships/src/design.rs), [collision/mod.rs](../../../crates/toy-sim-server/src/sim/physics/collision/mod.rs)). Explosion fragments are also boxes coloured with `color`.

The catalogue is compiled into the binaries with `include_str!`. After editing it, rebuild both applications. See [docs/asset-workflow.md](../../../docs/asset-workflow.md) for the full editing workflow.
