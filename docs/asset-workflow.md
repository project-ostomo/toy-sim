# AI-assisted part models and effects

Investigation: 2026-09-11. This is a proposed authoring workflow, not an installed
asset pipeline or a visual-quality benchmark of generation services.

## Recommendation

Use an AI coding assistant to author Blender Python generators for the mechanical
part kit, and Rust/Bevy code for runtime effects. Use image generation for concept
references and selected textures. Evaluate image-to-3D services as optional draft
sources after comparing one actual part against the scripted workflow.

The important production loop is: brief → generate → render → inspect → revise →
export → inspect in the editor. Inspecting the result matters as much as writing
the generation script.

## What already fits this repository

- There are nine predefined part types in
  `crates/osg-ships/data/catalogue.toml`.
- A part can already set `model = "models/parts/engine.glb"`.
  Both desktop applications load GLB scene 0.
- Exported assets use metres, are centred on their part origin, and fit the
  catalogue dimensions, which are multiples of 0.1 m.
- Thrust is along engine-local −Z, so a conventional nozzle and exhaust face +Z
  in the exported model. Blender's export axis conversion must be checked.
- Model geometry does not change the authoritative box occupancy, mass, inertia
  or capabilities. A visually hollow truss still occupies its whole defined box.
- Most catalogue parts use 1 × 1 × 1 m boxes; the engine is 1 × 1 × 2 m. Coolant tanks and heat sinks each occupy a 1 m cube.
- Presentation lives in `crates/osg-ship-view`. Effects belong there, not in
  the authoritative ship library or player WASM.
- Blender was not found on this environment's PATH during the investigation.
  No installations or paid generation jobs were run.

## Authoring approaches

| Approach | Suggested role | Main tradeoff |
| --- | --- | --- |
| Assistant-written Blender Python | Primary mechanical models and shared kit pieces | Precise and reproducible; appearance still needs visual iteration |
| Interactive Blender through MCP | Manual/assistant collaboration on an open scene | Convenient scene inspection; preserve a reproducible source after exploration |
| Concept image → image-to-3D → Blender cleanup | Optional unusual shapes or draft silhouettes | Generated topology, mounting surfaces and dimensions need checking |
| Assistant-written Bevy effects | Engine plumes, sparks and shield impacts | Must be evaluated in the actual renderer |

Blender supports background execution and Python scripts; no MCP bridge is
required for a generate/render/export loop.
[Blender command-line documentation](https://docs.blender.org/manual/en/3.0/advanced/command_line/arguments.html)

The community Blender MCP integration can inspect scenes, manipulate objects and
materials, and execute Python in an interactive Blender session. It is optional
and is not an official Blender product.
[Blender MCP](https://github.com/ahujasid/blender-mcp)

Meshy offers asynchronous text/image-to-3D jobs and GLB downloads, plus an
assistant-facing MCP integration. Tripo also documents image-to-3D with textured
GLB output. These are viable integrations, not evidence that either produces
better assets for this specific kit. Test one part before selecting a service.
[Meshy API](https://docs.meshy.ai/en/api/quick-start),
[Meshy AI integration](https://docs.meshy.ai/en/api/ai),
[Tripo image-to-model API](https://developers.tripo3d.ai/en/docs/generation-image-to-model)

## Repeatable production loop

1. **Define the visual language.** Choose a common mounting frame, panel treatment,
   bevel size, metal/ceramic palette, functional accent colours, and label style.
   Agree on a small contact sheet before building the inventory.
2. **Write a part brief.** Include catalogue ID, exact exported bounds, mounting
   faces, orientation, silhouette and effect attachment points. Gameplay values
   come from the catalogue; scripts should read dimensions rather than duplicate
   them in a second authoritative file.
3. **Generate editable geometry.** Use reusable functions for frames, cylinders,
   nozzles, vents, plates and fasteners. Authoring parameters are developer tools:
   exported catalogue pieces remain fixed, not player-configurable.
4. **Render a review sheet.** Front/side/back plus three-quarter views, both solid
   shaded and wireframe, with a bounding-box overlay. The assistant inspects
   these images and makes targeted changes.
5. **Export a GLB.** Apply geometry modifiers and transforms deliberately; keep
   exportable metallic/roughness materials. Bake unsupported procedural surface
   detail to textures. Do not assume the Blender shader graph is portable.
   [Blender glTF material documentation](https://docs.blender.org/manual/en/4.0/addons/import_export/scene_gltf2.html)
6. **Validate the exported asset.** Check finite vertices, expected bounds/origin,
   orientation, normals, triangle counts, material/primitive counts, textures and
   required named effect sockets. Re-importing the GLB catches export mistakes.
7. **Review in Bevy.** Check close-up and actual editor viewing distance, assembled
   beside neighbouring parts, and with simulation lighting/exposure/bloom.
   Also inspect a fleet: draw calls, texture duplication and overdraw can matter
   more than the triangle count of an isolated asset.

Initial budgets to experiment with: approximately 500–3000 triangles for an
ordinary module, one or two material primitives, and a shared material palette or
texture atlas. These are starting targets, not measured renderer limits. Avoid
exporting every bolt as its own render entity.

Suggested source layout, if implemented:

```text
art/parts/                 # Source briefs, references, .blend files
tools/art/                 # Blender generators, export and validation scripts
assets/models/parts/       # Runtime GLBs
assets/textures/parts/     # Shared textures, if not embedded
assets/textures/fx/        # Particle masks / flipbook images when needed
apps/osg-asset-preview/    # Optional Bevy helper for repeatable model/FX review
```

Keep source scripts, accepted references and generated runtime assets together in
version control. Record generator/service version and source provenance so assets
can be reproduced or replaced. Pin the Blender version for the generation scripts.

## Effects workflow

Runtime particles are separate assets/programs from the exported part meshes.
Start with an emissive plume mesh or simple shader for the engine, and add a
particle system when individual moving particles contribute to the effect.

Hanabi is a GPU particle plugin; its compatibility table lists Hanabi 0.19 with
Bevy 0.19. Its effect API exposes finite particle capacity and simulation space.
This makes it a reasonable candidate to prototype, rather than building a custom
particle renderer.
[Hanabi](https://github.com/djeedai/bevy_hanabi),
[EffectAsset API](https://docs.rs/bevy_hanabi/latest/bevy_hanabi/struct.EffectAsset.html)

Proposed division:

| Effect | Initial implementation |
| --- | --- |
| Steady engine exhaust | Emissive tapered mesh with procedural variation |
| Short-lived sparks / damage burst | GPU particles |
| Shield impact ripple | Mesh/material shader |
| Status lights / hot nozzle | Emissive material parameters |
| More complicated animated textures | Baked flipbook only when simpler shaders are inadequate |

Have the assistant author effect code and parameter curves, then capture the same
short preview sequence on each iteration: off, partial power, full power, shutdown,
rotation, camera movement and floating-origin rebase.

Add named GLB sockets such as `fx_exhaust`; resolving those sockets after scene
loading would be new presentation code. Drive intensity from actual hardware
output, not requested throttle, so lack of propellant/power and module failures
look correct. Render smoothly between the 10 Hz status samples.

Keep these effects cosmetic and local to the client/monitor. The server sends
hardware state and events, not particle positions. Emit effects only for relevant
visible ships and use shared assets and explicit particle budgets.

For the first plume use ship-local simulation space. Detached trails and particles
need a deliberate floating-origin strategy; moving an emitter does not imply that
particles already stored in GPU buffers have been rebased. This needs a targeted
test before adopting long-lived world-space trails.

## First milestone

Create one engine, one cargo module and one sensor from the existing catalogue.
Together they exercise rotational surfaces, box construction, thin details,
material consistency and nozzle orientation. Add one controllable exhaust effect.

Review those three assembled together and in a many-ship scene, then build the
remaining six existing types from the same kit. New sizes, decorative variants,
and new gameplay capabilities should be separate subsequent decisions.

A useful engine brief:

> Create a 1 × 1 × 2 metre electric engine within the exact catalogue box. Use
> the shared mounting frame, ceramic casing, dark nozzle and restrained accent
> markings. In exported Bevy coordinates, put the nozzle opening on +Z and the
> mounting interface on −Z. Add an fx_exhaust socket at the opening. Aim for
> 2000 triangles and two material primitives. Export scene 0 with a centred
> origin and produce front, side, rear and three-quarter review renders.

This milestone establishes whether the visual style and tooling work before
spending effort on dozens of assets.
