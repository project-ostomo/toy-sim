# Cleanup handoff

Implementation is largely complete; integration verification remains. All subagents
are stopped. The workspace includes extensive earlier gameplay changes: preserve
them and do not reset the working tree. No commit has been made.

## Implemented

- Removed LLM and NPC systems, including scripted NPCs and their firmware APIs.
- Zstd runs directly within async transport tasks, with no `spawn_blocking` or
  explicit scheduler yields. The user considers each compression call fast enough.
- Protocol uses length-prefixed postcard messages, without sections or per-frame
  versions. Persistence stores one world payload without sections.
- Canonical `GAME_VERSION = 46` lives in `osg-ship-api` and is reexported by
  `osg-model`; handshake, save compatibility, and firmware use it.
- Removed redundant ship schema/catalogue compatibility versions; regenerated
  nine bundled `.ship` assets. Rebuilt four bundled WASM assets using unversioned
  `ship` imports and the `game_version` export.
- Removed trusted server-data semantic validation and the unused presentation
  `DeviceReading` duplicate. Retained decoding and untrusted-input validation.
- Removed layout-dependent clicking tests, all 11 ignored tests, C/TS bindings
  and their regex generator, DEVLOG, and historical documentation/experiments.
- Added shared tick constants and updated current documentation.

## Remaining work

1. Finish integration checks, fixing regressions caused by this cleanup:
   ```sh
   cargo check --workspace --all-targets --all-features --offline
   cargo test --workspace --lib --all-features --offline --no-fail-fast
   cargo test -p osg-ships --offline
   cargo test -p osg-server persistence --offline
   ```
   Root checks may still be running after interruption: sessions `45830` (check)
   and `21586` (tests). Logs are `/tmp/osg-cleanup-workspace-check.log` and
   `/tmp/osg-cleanup-workspace-tests.log`. Latest check was waiting on Cargo's
   lock; tests were compiling the client. Avoid launching duplicate builds.
2. Confirm latest network tests pass after removing explicit compression yields;
   the workspace library suite includes them. Earlier six network tests passed.
3. Run `cargo fmt --all` and `git diff --check` after final fixes; scan for obsolete
   APIs, version constants, ignored tests, and stale documentation references.
4. Report actual validation results and the intentional save-format incompatibility.

## Verification context

- 54 ship-WASM tests, 9 client tests without default features, and 7 surviving
  client slip tests passed. Earlier persistence storage/checkpoint tests passed.
- Initial persistence world failures came from stale bundled firmware; rebuilding
  fixed a targeted restoration test. Full persistence rerun remains unconfirmed.
- Integration compilation caught obsolete `GasLedger::prepay` tests. Those tests
  have now been deleted by the NPC agent; rerun compilation to verify.
- Ship tests initially had an obsolete serialized field-count assertion; it was
  updated after removing appearance metadata. Full rerun remains unconfirmed.
- Some server tests failed before this cleanup, including
  `starting_orbit_allows_slip_departure_without_a_clearance_burn` (no route).
  Investigate failures rather than assuming every failure is preexisting.
- Gaia catalogue binary encoding version `2` remains: it describes the static
  asset encoding. Industry catalogue content hashes also remain for change
  detection. These are distinct from game compatibility versions.

Use native `apply_patch` for manual edits. No compatibility shims, live desktop
interaction, or restored ignored tests. Do not resume agents without authorization.
