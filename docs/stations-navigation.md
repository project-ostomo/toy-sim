# Stations, navigation and inventory

Build both the server and debug launcher with `cargo build -p toy-sim-server -p toy-sim-debug`, then run `cargo run`. The default patrol uses a water NTR and a cartridge autocannon. Its attitude actuator and stock controller are tested to reverse its heading within three seconds. A hostile patrol starts 1 km away. Neris Anchorage is nearby, and ten gate mouths connect Helion, Sol, Vesper and Aurora.

## Client controls

- Select a ship in Overview to align, approach, keep range or engage. These actions replace the current navigation/combat queue. Keep range and engage continue until interrupted; approach and align finish when their conditions are met.
- Select a station beacon to approach or dock, or a gate to approach and jump. Hold Shift to append these orders.
- Open Gate Network from the left toolbar. Click a system to preview a route, then use Set destination or Add waypoint. Drag empty map space to pan and scroll to zoom. Only wormhole-connected systems are shown. The graph selects gate hops; the flight computer performs the local flying and validates usable gates through world queries.
- Open Navigation to inspect, remove or move pending commands earlier. Pause and resume preserve the queue. Clear queue cancels it.
- The location HUD shows the active action, queue position and slip ETA. Gold beacon markers identify queued destinations, including an edge marker when the destination is off screen.
- Look at is available for ships within 100 km in the subscribed picture. Escape returns to the controlled ship.
- Docking changes the view to a private hangar. Drag empty space to spin the camera around the ship; scroll to zoom. Undock is available in the location display. Stored ships have no active physics body.
- Inventory has Cargo hold and Consumables tabs. Cargo uses integer icon stacks; consumables use capacity bars. Select a cargo stack and another controlled, co-docked ship to transfer an integer quantity. Tanks never draw from cargo and their contents cannot be transferred.

## Construction

Ship format 3 stores one root and an attachment tree. Each connection names a parent socket, child plug and quarter-turn roll. Connector strings must match exactly and each node may be used once. The editor shows matching sockets and previews the assembled pose. R changes roll; Delete removes the selected part and its attached descendants.

Catalogue nodes specify a position in metres and an outward cardinal normal. Hull connectors include their diameter, such as `hull_2`; utility mounts use `equipment`. Station structural, docking and beacon connections use `station_backbone`. The habitat exposes equipment nodes on its fixed hub. Parts can provide their own node list.

Dimensions remain in decimetres for catalogue authoring. They do not define a construction grid. The server voxelizes the assembly on a coarse 1 m grid and merges cells into collision boxes. Explicit cylindrical, habitat and hangar profiles preserve open regions. Habitat spoke sweeps are conservative collision regions; the ring animation runs only on clients and in the editor.

## Station assets

The separate Blender library is [station-catalogue.blend](../assets/models/stations/station-catalogue.blend). Its construction/export script is [stations.py](../tools/art/stations.py). Ship-part artwork remains in its existing Blender files.

| Part | Dimensions | Function |
| --- | --- | --- |
| Station backbone | 32 m diameter × 64 m | Tank volume and equipment attachment nodes |
| Station bulkhead | 32 m diameter × 8 m | Structural end closure |
| Habitat | 200 m diameter × 48 m | Fixed hub, two counterrotating habitation rings, 10,000 crew capacity |
| Docking hangar | 64 m diameter × 96 m | Open mouth, 25 m ship-radius limit, storage ingress |
| Subspace beacon | 48 × 48 × 64 m | Long-range station destination |
| Gate frame | 512 m envelope | Fixed sparse frame surrounding a 220 m radius spherical aperture |

The habitat ring speed is `sqrt(g / 94 m)` in opposite directions. GLB node extras carry `angular_velocity_rad_s`; the shared rendering plugin evaluates rotation from simulation time. Neris Anchorage combines these modules with a fission reactor, radiator, life support, cargo storage and a ship-class gun.

## Runtime contracts

The host owns `TravelState` and presence changes. Standard firmware owns guidance, target tracking, local approach, gate routing and docking alignment. Revision checks reject stale queue edits. Contact guidance uses the fused information group and does not reveal hidden entity state. An unavailable target blocks and retries the command.

Gates are prescribed navigation anchors without rigid-body dynamics. Their local circular ephemerides keep them in a reachable traffic frame; collisions and player forces cannot displace them. Entry is omnidirectional, speed-limited relative to the mouth, and revalidated after a one-tick dwell.

A docking bay is an entry interface. After docking, ships enter the host's ECS storage relationship and release its reservation, so the same bay can serve multiple stored ships. Hull, cargo, tanks, hardware and ownership remain on the stored entities. Host mass includes them. Destruction preserves them as wreck inventory. Persistence remains outside this implementation.

Consumable counts are `u64`. A fractional request uses stochastic rounding with the default `rand` RNG; a request for 2.4 units removes 2 or 3 with probabilities 0.6 and 0.4. Continuous supplied demand drives smooth thrust and power, while actual integer debits determine remaining stock and mass. Reactor conversion and processing conserve actual integer material counts.

## Verification

```sh
cargo test -p toy-sim-server --lib infrastructure::tests
cargo test -p toy-sim-server --lib default_patrol_encounter
cargo test -p toy-sim-server --lib default_ntr_patrol_reverses_heading
cargo test -p toy-sim-ships --test attachments
cargo test -p toy-sim-ships --test ships
cargo test -p toy-sim-server --test network
```

The docking tests run the stock WASM flight computer through approach and docking. Other tests cover storage, cargo isolation, gate access, coarse hull clearance, conventional firing, integer consumption, and attachment graph validity. Client presentation should also be inspected with screenshots of the map, both inventory tabs, gates, habitat rotation, slip transit and the hangar.

The cargo layout uses [EVE Online’s inventory screenshots](https://www.eveonline.com/de/news/view/unified-inventory) as a reference for icon stacks, quantity overlays, capacity and filtering.
