# Persistence and the game calendar

The server saves one world payload per SQLite checkpoint. A transaction publishes
one complete generation, using WAL mode and FULL synchronization. Three
generations are retained. SQLite `PRAGMA user_version` stores the shared `GAME_VERSION`,
which also controls connection and firmware compatibility.

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

The newest generation is authoritative. Decoding or reconstruction errors stop
startup. Older generations remain available for explicit recovery.
Preserve the database and its WAL when diagnosing a failed recovery. Saving errors
are reported to the server loop and cause shutdown rather than continued unsaved
operation.

## Saved state

The server's `SocietyState` contains social records, economy and gas accounts.
Indexed Rust collections serialize canonical records and rebuild secondary
indexes when restored. Saves are trusted: loading does not check checksums,
duplicate keys, references, balances or other domain invariants. Malformed saves
have unspecified behavior. Derived asset indexes live in a separate ECS resource and
rebuild through normal publication. Gas is serialized inside `SocietyState`.

Authoritative maps, sets and declaration histories use persistent `imbl`
collections. Cloning the state shares their storage; writes copy the affected
paths. Queued actions mutate a draft and publish it only on success. Physical
component replacements are prepared alongside the draft before publication.
Snapshots serialize values, without sharing pointers or secondary indexes.

The checkpoint remains one atomic SQLite payload. SQLite does not serve live
society queries. This schema uses game version 63 and does not migrate earlier
checkpoints.

The world record includes stable identities, political affiliations and standing
overrides, asset owners and access grants,
ship designs and resources, physical poses and motion, damage and thermal state,
docking relationships, committed slip transit, and the simulation clock. Ship programs
are stored once per content hash, alongside each computer's explicit persistent
data. Runtime execution stacks restart on recovery, including computers suspended
inside a callback. Only explicitly committed durable guest data survives. The
host navigation queue, current stage, autopilot toggle, per-stage estimates, route
fuel budget, original and spent itinerary risk, and slip charging work survive.
Slip direction, nominal aim, accumulated variance, speed, retained velocity,
distance travelled, fuel accounting, and beacon-loss state survive. Future
disturbances are drawn during flight; saves contain only realized motion.
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
Each computer system debits grants, executes computers, then refunds unused gas
and records actual spending before returning. Flight computers execute in parallel;
display computers retain their serial scheduling and share the physical tick cap.
Checkpoints run outside these systems, after refunds. Restoring preserves saved
gas balances and allocation cursors.

Queued slip orders preserve their typed destination references. An active charge
also retains its concrete candidate, start tick and accumulated energy. A ship
already in transit restores the galactic endpoint frozen at departure.

Slip history saves coalesced swept spans and short transition impulses, including
passage timestamps, inertial drift, visual seeds and opaque effect identities.
Each deposited portion expires after 300 simulation seconds. Restoring preserves
its remaining lifetime; the source ship does not need to survive.

Industry state is saved with each facility. Jobs retain their UUID, creator and
output owner, capability, exact input and output specifications, total processing
energy, stored output energy and integer progress. Shipyard jobs also retain the
complete blueprint, including its program. Recovery compiles these blueprints to
reconstruct the runtime designs.

Cargo includes resource quantities and packaged part kits. Reservations remain
inside the same physical inventory and count toward its mass and volume. Their
totals reflect pending jobs' inputs. Damaged or unpowered modules can retain
suspended jobs.

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

Saved programs are instantiated through the normal WASM runtime. Unsupported
imports or a mismatched `game_version` export cause instantiation to fail.

Authentication configuration stays outside the database. Back it up together with
the world; restoring a world without the matching account credentials does not
grant access to its assets.

Sensor snapshots and contact handles are transient and rebuilt from restored geometry. Saved contact-dependent orders are blocked with autopilot disabled because their handles cannot survive a restart. Public beacon and celestial references remain durable.

## Universe definition changes

An incompatible `GAME_VERSION` stops startup. There are no separate database,
checkpoint, or world-section versions and no automatic migration.

A changed catalogue is assumed compatible with the saved world. To start a new world,
preserve the previous state and
choose a new `--state-dir` for the debug launcher, or another `[persistence].path`
for a dedicated server. A normal restart with unchanged definitions restores the
same world. Moving or deleting the original ship blueprint file remains safe
because the ship design itself is stored in the checkpoint.

Snapshots retain actual equipment, transponder settings, inventories, charging
progress, committed slip trajectories, realized drift, and itinerary risk.
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
