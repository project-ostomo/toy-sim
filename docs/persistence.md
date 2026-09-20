# Persistence and the game calendar

The server saves versioned world records to SQLite. Tables hold checkpoint
metadata and named binary sections. A transaction publishes one complete
generation, using WAL mode and FULL synchronization. Three generations are
retained, with checksums for each section and the complete manifest.

The default configuration is:

```toml
[persistence]
enabled = true
path = "world.sqlite"
interval_seconds = 900
```

The path is relative to the server configuration directory. A process holds an
exclusive file lock for the lifetime of the world, so a second server cannot
open the same database. New worlds commit an initial checkpoint before the server
announces readiness. Existing worlds restore the latest checkpoint before clients
can connect. Restoring uses saved ship designs and programs; the original ship
source file is not needed.

Snapshot capture runs between simulation updates. One background worker performs
the SQLite writes, with at most one periodic checkpoint outstanding. Logs report
capture time, write time, byte count and generation separately. The interval uses
wall time and can be increased when a large world needs longer capture pauses.

Ctrl-C, SIGTERM, and the debug launcher's stdin lifetime signal request graceful
shutdown. The server takes a final checkpoint and waits for its commit. On Unix,
SIGUSR1 requests an extra checkpoint while the server keeps running. A process
abort or machine failure loses changes since the last committed checkpoint.

The newest generation is authoritative. A checksum, format or reference error
stops startup with a diagnostic; it does not silently roll the world back. Older
generations remain available for an operator to inspect and recover explicitly.
Preserve the database and its WAL when diagnosing a failed recovery. Saving errors
are reported to the server loop and cause shutdown rather than continued unsaved
operation.

## Saved state

The world record includes stable identities, political affiliations and standing
overrides, asset owners and access grants, information groups and sensor tracks,
ship designs and resources, physical poses and motion, damage and thermal state,
docking relationships, committed slip transit, and the simulation clock. Ship programs
are stored once per content hash, alongside each computer's explicit persistent
data. Runtime execution stacks restart on recovery, including computers suspended
inside a callback. Only explicitly committed durable guest data survives. The
host navigation queue, current stage, autopilot toggle, per-stage estimates, route
fuel budget, original and spent itinerary risk, and slip charging work survive.
Slip direction, speed, retained velocity, distance travelled, fuel accounting,
and both departure and beacon-loss error samples survive without resampling.
The active arrival estimate is cleared
until guidance reports a fresh value. Actuator commands restart from their boot
defaults while the computer comes online. A trapped program still
uses the normal fault/reset behavior. The durable guest API is
described in [Ship controller ABI](ship-abi.md).

Public route previews and background worker continuations are transient. Recovery
cancels those workers and clears their result cache, even when restoring the same
world epoch. Clients request previews again. A saved queue that is still planning
retains its requested destinations and the server recomputes its complete route;
this also works while autopilot is paused or the flight computer is booting.

Gas balances are saved for their actual owner: a player, organization or
sovereignty. Available and spent gas retain their exact integer values, together
with the allocation cursor used to share scarce gas fairly between computers.
Capture requires every reservation to be settled, including reservations of zero
gas. A checkpoint cannot contain outstanding debits. Restore validates all billing
principals and requires an account for every ship with a computer before replacing
the world; it does not replenish a saved balance from the starting allocation.

Queued slip orders preserve their typed destination references. An active charge
also retains its concrete candidate, start tick and accumulated energy. A ship
already in transit restores the galactic endpoint frozen at departure.

Missiles save their ordinary ship hardware and integer fuel, physical pose,
lifetime clock, steering command, guidance flag, target contact reference, parent
UUID and per-parent handle. Launcher cooldowns, callback rotation and the next
unused handle are saved with the parent. Handles are never reassigned after a
restart. A target contact may have expired; recovery does not require it to remain
visible or turn it into a privileged position lookup.

A destroyed parent with guided missiles retains its shared computer on the same
stable identity. Its owner, information group, program, committed durable bytes
and paid gas balance survive recovery. The parent restores as destroyed and
inactive in physics; only its retained missile computer remains available. Each
missile body restores without a second WASM instance. As with ordinary computers,
a suspended native continuation cold boots after recovery.

Before capture or restore, the server rejects missing parents, missile parents
that are themselves missiles, duplicate or reused handles, and guided missiles
without a retained parent computer where one is required. Restore also checks
that a guided parent's saved program exports `missile_tick`. Invalid launcher
clocks, handle counters and steering values stop recovery before world mutation.

Industry state is saved with each facility. Jobs retain their UUID, creator and
output owner, capability, exact input and output specifications, total processing
energy, stored output energy and integer progress. Shipyard jobs also retain the
complete blueprint, including its program. Recovery validates that program before
replacing any world entities.

Cargo includes resource quantities and packaged part kits. Reservations remain
inside the same physical inventory and count toward its mass and volume. Their
totals must equal the sum of pending jobs' inputs exactly. Capture and recovery
reject orphan reservations, unavailable items, overflowing counts, invalid job
owners and output specifications, and jobs requiring an absent installed
capability. Damaged or unpowered modules can retain suspended jobs. Job UUIDs
cannot duplicate one another or another world identity.

Snapshots occur between simulation updates, so they cannot interrupt completion
between consuming ingredients, creating the output and removing the job. A
checkpoint before completion retains the job and its reserved stock; a checkpoint
after completion retains the output and no pending job. Integer progress
determines the processing energy already paid, while the facility's saved battery
contains the remaining energy. Recovery resumes that work without charging the
completed steps again. Finished ships use the ordinary saved ship record and
docking relationship. Construction produces a cold hull; recovery does not grant
fuel, coolant or battery charge.

Each starter mine saves its output type, integer units per second, fractional
remainder and last recipient UUID. A prior recipient may have departed, so the
cursor does not require a live inventory reference. Mine production advances on
simulation ticks; restarting does not produce material for the downtime. The
facility's starter-grant flag is durable, preventing recovery from repeating the
initial stock grant.

NPC organization directors are ordinary ECS entities with stable organization
IDs. Their checkpoints retain the officer, home system, primary facility, asset
assignments, planning cadence, decision and action counters, bounded tool results,
and the exact pending LLM request. The saved director-program digest remains part
of that request's billing scope. Unsupported director versions stop recovery
before world mutation.

The separate [LLM spending ledger](llm.md) retains provider request identities
across world recovery. A restored pending plan polls or resubmits the same scoped
ID and payload; a request already dispatched is never billed again merely
because the server restarted. A completed decision and its game actions are
applied together within one exclusive simulation step, so checkpoints preserve
their action counters alongside the resulting orders, inventories and jobs.

Physical defense assignments and freight duties are saved with their ships.
Defense threat observations are transient and must be acquired again after
recovery. Freight stages, endpoints, item quantities, pause reasons and next-check
times survive recovery. Captured ships and revoked officer permissions do not
make the saved world invalid: each behavior checks current authority before
acting. Historical asset and endpoint IDs may remain after destruction or
despawning; ordinary lookups then report that the object is unavailable. Invalid
account or organization references, scalar bounds and oversized planning context
are rejected before replacing world entities.

Saved programs must implement the current ABI 32. Restore validates each
program's content hash, imports and API-version export before replacing world
entities. An unsupported saved program stops startup with an error.

Authentication configuration stays outside the database. Back it up together with
the world; restoring a world without the matching account credentials does not
grant access to its assets.

## Universe definition changes

The current named `world` section has version 9. SQLite’s table schema and the
outer checkpoint container retain their existing format. Earlier world sections
are rejected before ECS state is replaced; there is no automatic migration or
creation of a replacement database.

A world record stores a BLAKE3 fingerprint over organization lore and the shared
universe fingerprint. The universe fingerprint covers astronomical inputs,
authored definitions, and generator revision without generating system bodies.
Restore compares it against the loaded universe before mutating the world.
The ship-resource catalogue and persistent references are validated separately.

The lazy universe and natural-capture travel format require a new saved world.
Startup reports an incompatible world section or a changed catalogue and asks
for an explicit new database. Preserve the previous state and
choose a new `--state-dir` for the debug launcher, or another `[persistence].path`
for a dedicated server. A normal restart with unchanged definitions restores the
same world. Moving or deleting the original ship blueprint file remains safe
because the ship design itself is stored in the checkpoint.

Snapshots retain actual equipment, transponder settings, inventories, charging
progress, committed slip trajectories, sampled errors, and itinerary risk.
Directory emitter markers, public inhabited membership, and active celestial
entities are rebuilt from restored objects. A restart never runs the initial
infrastructure placement recipe or replenishes destroyed installations.
Destroyed objects retain whether a physical wreck exists. A ship lost during
slip does not gain a wreck or activate a system when restored.

## Local debug worlds

`osg-debug` retains its world and credentials under
`$XDG_STATE_HOME/openspacegame/debug`, or `~/.local/state/openspacegame/debug` when that variable
is unset. `--state-dir PATH` selects another saved world. `--ephemeral` creates a
disposable world and removes it when the launcher exits normally. These options
cannot be combined.

The directory contains `identity.toml`, `server.toml` and `world.sqlite`. Secret
files use private permissions. A launcher lock prevents concurrent sessions from
overwriting the same identity/configuration. A missing identity in an existing
world is an error; the launcher does not generate a new owner silently.

The `--ship PATH` option chooses the starting design for a new world. An existing
world retains its saved design. To experiment with a different initial ship, use
a separate state directory or an ephemeral launch. The debug reset command starts
a fresh scenario and requests a checkpoint of that reset world.

## Calendar

In-game UTC is real UTC plus exactly 146097 days. This advances the Gregorian year
by 400 and preserves month, day and weekday, including leap days. It is separate
from elapsed simulation time and continues independently of accelerated simulation
and server downtime. The wire representation uses signed milliseconds, which
comfortably includes the year 2426. The bottom HUD shows the calendar and exposes
elapsed simulation time in its tooltip.
