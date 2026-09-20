# Assets

This directory is the Bevy asset root for the client and ship editor, resolved
relative to their crate paths at compile time. The server embeds the bundled
celestial definitions and streams immutable system and ship assets to clients
by content hash. Rebuild after editing embedded definitions.

For step-by-step editing instructions, see [docs/asset-workflow.md](../docs/asset-workflow.md).

## Contents

| Path | Loaded by | Purpose |
| --- | --- | --- |
| [universe.toml](universe.toml) | `osg-universe` | Lists the ten authored system definitions used alongside the inhabited catalogue. |
| [stars/helion.star.toml](stars/helion.star.toml) | Server and streamed client orrery | Helion, ocean world Neris, and five airless moons. |
| [stars/sol.star.toml](stars/sol.star.toml) | Server and streamed client orrery | The Sun and eight planets, with physical appearance metadata. |
| [ships/starter.ship](ships/starter.ship) | The editor's Open button and design tools | A starter blueprint in the binary `.ship` format. |
| [models/dummy.glb](models/dummy.glb) | Nothing in the current source | A glTF binary file with no references. |
| [models/parts/](models/parts/README.md) | Parts whose catalogue entry sets `model` | Location for optional part models. |

## universe.toml

```toml
systems = ["stars/helion.star.toml"]
```

`systems` must be a non-empty list of non-empty paths relative to this directory.
Unknown keys are rejected. The bundled manifest contains ten entries; the
inhabited map adds catalogue systems. See the
[generation guide](../crates/osg-universe/GENERATION.md).

## Star system files (`*.star.toml`)

Star system files deserialize as `OrreryCfg`
([orrery_cfg.rs](../crates/osg-universe/src/orrery_cfg.rs)). Unknown top-level
keys are rejected.

Top level:

- `name`: system name, unique across the universe.
- `position_um`: fixed galactic anchor as three integer micrometre coordinates. Decimal strings are accepted so values beyond the TOML integer range survive. Defaults to the origin.
- `bodies`: array of body tables.

Body fields:

| Field | Default | Notes |
| --- | --- | --- |
| `name` | required | Unique across all loaded systems. |
| `class` | `planet` | `star` (requires `lumens`), `planet`, or virtual `barycenter`. |
| `parent` | none | Required for every body except the system root. |
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
| `planet` | none | Physical surface recipe, including seed, climate, oceans, relief, clouds, and biosphere. |
| `stellar` | none | Stellar class, effective temperature, and age. |

Each system has one fixed root and at least one star. Multiple stars may orbit
virtual barycenters. Stellar `lumens` must be positive and finite. Virtual bodies
carry orbital mass and parent relationships but have no rendered or collidable
surface.

Atmosphere tables use metres, kg/m³, kelvin, J/(kg·K) and optical coefficients in m⁻¹: `height`, `surface_density`, `scale_height`, `temperature`, `specific_gas_constant`, `heat_capacity_ratio`, `rayleigh_scattering` (RGB), `mie_scattering`, `mie_absorption`, `mie_scale_height`, `mie_asymmetry`, `ground_albedo` (RGB). See [helion.star.toml](stars/helion.star.toml) for a complete example.

All bundled orbital angles, including Sol's, use radians. Surface recipes are
baked deterministically on the client; they do not require stored texture files.

## Ship files

`.ship` files are CBOR-encoded blueprints. They are documented in [docs/ships.md](../docs/ships.md#blueprint-files-ship). To regenerate `ships/starter.ship` from code:

```sh
cargo run -p osg-ships --example write_starter -- assets/ships/starter.ship
```

## Assets compiled into binaries

Some data is embedded at build time and is not part of this directory. Editing it requires a rebuild:

- the part and resource catalogue: [crates/osg-ships/data/catalogue.toml](../crates/osg-ships/data/catalogue.toml)
- the standard firmware: [crates/osg-ships/data/example-controller.wasm](../crates/osg-ships/data/example-controller.wasm)
- the star catalogue: [crates/osg-stars/data/](../crates/osg-stars/data/README.md)
- the MFD font: [crates/osg-ui/data/fonts/](../crates/osg-ui/data/fonts/README.md)
- the WGSL shaders in [crates/osg-ship-view/src](../crates/osg-ship-view/src)
