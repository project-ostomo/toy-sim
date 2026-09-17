# Assets

This directory is the Bevy asset root for both GUI applications. `toy-sim` and `toy-ship-editor` set `AssetPlugin::file_path` to `<crate>/../../assets`, resolved at compile time. Both applications always read from this directory, whatever the current working directory is.

For step-by-step editing instructions, see [docs/asset-workflow.md](../docs/asset-workflow.md).

## Contents

| Path | Loaded by | Purpose |
| --- | --- | --- |
| [universe.toml](universe.toml) | `toy-sim` at startup | Lists the star system files that make up the universe. |
| [stars/helion.star.toml](stars/helion.star.toml) | `toy-sim`, through `universe.toml` and several tests | The Helion system: one star, the planet Helion I Neris with an atmosphere, and five airless moons. |
| [stars/sol.star.toml](stars/sol.star.toml) | Nothing at present | A Sun and eight planets. It is not listed in `universe.toml`. |
| [ships/starter.ship](ships/starter.ship) | `toy-sim --ship`, the editor's Open button, an editor test | The armed starter design saved in the binary `.ship` format. |
| [models/dummy.glb](models/dummy.glb) | Nothing in the current source | A glTF binary file with no references. |
| [models/parts/](models/parts/README.md) | Parts whose catalogue entry sets `model` | Location for optional part models. |

## universe.toml

```toml
systems = ["stars/helion.star.toml"]
```

`systems` must be a non-empty list of non-empty paths relative to this directory. Unknown keys are rejected. Each path is loaded as a star system asset.

## Star system files (`*.star.toml`)

The `.star.toml` extension selects the TOML loader for `OrreryCfg` ([orrery_cfg.rs](../crates/toy-sim-universe/src/orrery_cfg.rs)). Unknown top-level keys are rejected.

Top level:

- `name`: system name, unique across the universe.
- `position_um`: fixed galactic anchor as three integer micrometre coordinates. Decimal strings are accepted so values beyond the TOML integer range survive. Defaults to the origin.
- `bodies`: array of body tables.

Body fields:

| Field | Default | Notes |
| --- | --- | --- |
| `name` | required | Unique across all loaded systems. |
| `class` | `planet` | `star` (requires `lumens`) or `planet`. |
| `parent` | none | Required for every body except the star. |
| `mass` | 0 | Kilograms, or a string with `kg`, `massEarth`/`mEarth`, `massSol`/`mSol`/`massSun`. Must be positive. |
| `radius` | 0 | Metres, or a string with `m`, `km`, `au`, `ly`, `pc`. Must be positive. |
| `semi_major` | 0 | Distance units as above. Zero fixes the body to its parent. |
| `period` | computed | Seconds, or a string with `s`, `h`, `d`, `yr`. If zero while `semi_major` is non-zero, it is computed from Kepler's third law. |
| `eccentricity` | 0 | Must lie in [0, 1). |
| `inclination`, `ascending_node`, `arg_of_pericenter`, `mean_anomaly` | 0 | Radians. |
| `epoch` | 0 | MJD. |
| `rotation_period` | 0 | Time units as above. |
| `obliquity`, `eq_ascend_node`, `rotation_epoch` | 0 | Rotation parameters. |
| `surface_color` | `[0.4, 0.4, 0.4]` | Components in [0, 1]. |
| `spectral_class` | none | One of `O B A F G K M`. It sets the star colour. |
| `atmosphere` | none | Table; omit for an airless body. |

Each system must contain exactly one star. The star has no parent and a zero semi-major axis, and its `lumens` must be positive and finite.

Atmosphere tables use metres, kg/m³, kelvin, J/(kg·K) and optical coefficients in m⁻¹: `height`, `surface_density`, `scale_height`, `temperature`, `specific_gas_constant`, `heat_capacity_ratio`, `rayleigh_scattering` (RGB), `mie_scattering`, `mie_absorption`, `mie_scale_height`, `mie_asymmetry`, `ground_albedo` (RGB). See [helion.star.toml](stars/helion.star.toml) for a complete example.

`sol.star.toml` writes its orbital angles with degree-like values (for example `inclination = 7.00487`). The solver reads these fields as radians. Convert the angles before you add that file to `universe.toml`.

## Ship files

`.ship` files are CBOR-encoded blueprints. They are documented in [docs/ships.md](../docs/ships.md#blueprint-files-ship). To regenerate `ships/starter.ship` from code:

```sh
cargo run -p toy-sim-ships --example write_starter -- assets/ships/starter.ship
```

## Assets compiled into binaries

Some data is embedded at build time and is not part of this directory. Editing it requires a rebuild:

- the part and resource catalogue: [crates/toy-sim-ships/data/catalogue.toml](../crates/toy-sim-ships/data/catalogue.toml)
- the standard firmware: [crates/toy-sim-ships/data/example-controller.wasm](../crates/toy-sim-ships/data/example-controller.wasm)
- the star catalogue: [crates/toy-sim-stars/data/](../crates/toy-sim-stars/data/README.md)
- the MFD font: [crates/toy-sim-ship-view/data/fonts/](../crates/toy-sim-ship-view/data/fonts/README.md)
- the WGSL shaders in [crates/toy-sim-ship-view/src](../crates/toy-sim-ship-view/src)
