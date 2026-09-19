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
capture time, write time, byte count and generation separately. The interval can
be increased when a large world needs longer capture pauses. It is wall time,
so paused debug worlds still save.

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
docking relationships, gates, slip transit, and the simulation clock. Ship programs
are stored once per content hash, alongside each computer's explicit persistent
data. Runtime execution stacks restart on recovery. The host navigation queue, current
stage, autopilot toggle and slip charging work survive; actuator commands restart
from their boot defaults while the computer comes online. A trapped program still
uses the normal fault/reset behavior. The durable guest API is
described in [Ship controller ABI](ship-abi.md).

Authentication configuration stays outside the database. Back it up together with
the world; restoring a world without the matching account credentials does not
grant access to its assets.

## Local debug worlds

`toy-sim-debug` retains its world and credentials under
`$XDG_STATE_HOME/toy-sim/debug`, or `~/.local/state/toy-sim/debug` when that variable
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
from elapsed simulation time and continues through pauses, accelerated simulation,
and server downtime. The wire representation uses signed milliseconds, which
comfortably includes the year 2426. The bottom HUD shows the calendar and exposes
elapsed simulation time in its tooltip.
