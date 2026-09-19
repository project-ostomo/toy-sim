# Stations, navigation and inventory

Build both the server and debug launcher with `cargo build -p toy-sim-server -p toy-sim-debug`, then run `cargo run`. The default Peregrine expedition patrol has a 4 m micropulse drive, two lasers, a 250 MW thermal fission reactor, batteries and a fitted 100 MW slipdrive. The smaller water-NTR patrol remains available as `assets/ships/ntr-patrol.ship`. A hostile patrol starts 1 km away, with Neris Anchorage nearby.

The ten-system chain is Helion → Sol → Vesper → Aurora → Lyra → Cinder → Meridian → Havoc → Elysium → Terminus. Its nine connections have eighteen mouths. Intermediate systems place their mouths 60 Mm apart, each with a 10 Mm exclusion radius, so each gate hop requires a substantial transfer. If two enabled macromouth exclusion volumes overlap, both mouths lose stability and become disabled. Slip apertures must also stay outside enabled mouth exclusions; ordinary sublight traffic can enter them.

## Client controls

- Select a ship in Overview to align, approach or keep range. These actions replace the navigation queue. Keep range continues until interrupted; approach and align finish when their conditions are met. Mark target and Start firing are separate weapons actions. Stop firing retains the mark; Unmark target clears it and stops firing. Navigation never enables firing.
- Select a station beacon to approach or dock, or a gate to approach and jump. Hold Shift to append these orders.
- Open Gate Network from the left toolbar. Click a system to preview a route, then use Set destination or Add waypoint. Drag empty map space to pan and scroll to zoom. Only wormhole-connected systems are shown. The flight computer queues gate, sublight and slip orders, performs local guidance and validates usable apertures through world queries.
- Gate Network's Fuel priority slider trades travel time for propulsion fuel, with the equivalent minutes per tonne shown below it. Set destination again to apply a changed preference to an existing route. The current route's required fuel and available tank balances appear in Gate Network and Navigation, with a red exhaustion warning and estimated shortfall for each deficient resource. Partial estimates are labelled. The location HUD also flags exhaustion risk. Slip preparation preserves motion, and staging points move with their gates. Fuel estimates include matching destination motion after arrival.
- Open Navigation to inspect, remove or move pending commands earlier. Pause and resume preserve the queue. Clear queue cancels it.
- The location HUD lists every remaining route stage without scrolling, with an approximate cumulative ETA for each timed stage. The active estimate updates during flight and slip charging. Continuous commands and unknown estimates are labelled explicitly. Amber numbered waypoint markers include gates, docking destinations, slip arrivals, galactic coordinates and relative positions; dotted lines connect the route. Offscreen waypoints use edge markers. Distances of 0.1 light-years or more display in ly. Floating windows can overlap the bottom HUD.
- Look at is available for ships within 100 km in the subscribed picture. Escape returns to the controlled ship.
- Docking changes the view to a private hangar. Right-drag empty space to spin the camera around the ship; scroll to zoom. Undock is available in the location display. Stored ships have no active physics body.
- Inventory has Cargo hold and Consumables tabs. Cargo uses integer icon stacks; consumables show installed tanks, battery charge and shield reserve. Resources without tank capacity are omitted. Select a cargo stack and another controlled, co-docked ship to transfer an integer quantity. Tanks never draw from cargo and their contents cannot be transferred.

Double-click empty scene space to align the controlled ship along the camera ray. Right-drag orbits the camera with a 40 ms exponential angular response; the mouse wheel zooms.

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

The host owns `TravelState` and presence changes. Standard firmware owns guidance, target tracking, local approach, gate routing and docking approach. Revision checks reject stale queue edits. Contact guidance uses the fused information group and does not reveal hidden entity state. An unavailable target blocks and retries the command.

Gates are prescribed navigation anchors without rigid-body dynamics. Their local circular ephemerides keep them in a reachable traffic frame; collisions and player forces cannot displace them. Entry is automatic and omnidirectional. Crossing the aperture at up to 100 m/s relative to the mouth transfers the object; higher speeds destroy it. Outward exits place the whole object beyond the paired aperture.

Docking is available from any direction within 100 m surface clearance of the station (centre distance minus both bounding radii), at no more than 10 m/s relative speed. The autopilot approaches from the ship’s current side and brakes before requesting capture. It does not fly into a hangar or match its orientation. A docking bay determines access and the size and mass of ships that can dock. After docking, ships enter the host's ECS storage relationship and release its reservation, so the same bay can serve multiple stored ships. Hull, cargo, tanks, hardware and ownership remain on the stored entities. Host mass includes them. Destruction preserves them as wreck inventory. Persistence remains outside this implementation.

Consumable counts are `u64`. A fractional request uses stochastic rounding with the default `rand` RNG; a request for 2.4 units removes 2 or 3 with probabilities 0.6 and 0.4. Continuous supplied demand drives smooth thrust, while actual integer debits determine remaining stock and mass. Battery charge and capacity are integer joules. Electrical producers and consumers use stochastic rounding at the accounting boundary; dock power transfers debit and credit the same integer amount. The shield coolant reserve is stored in integer milligrams, with stochastic withdrawals into the continuously simulated shield. Reactor conversion and processing conserve actual integer material counts.

## Verification

```sh
cargo test -p toy-sim-server --lib infrastructure::tests
cargo test -p toy-sim-server --lib default_patrol_encounter
cargo test -p toy-sim-server --lib default_ntr_patrol_reverses_heading
cargo test -p toy-sim-ships --test attachments
cargo test -p toy-sim-ships --test ships
cargo test -p toy-sim-server --test network
```

The docking tests run the stock WASM flight computer from the default spawn through approach and capture, checking that it stays outside the station’s collision envelope. They also check range and speed boundaries, arbitrary arrival attitude, bay access and compatibility. Other tests cover storage, cargo isolation, gate access, coarse hull clearance, conventional firing, integer consumption, and attachment graph validity. Client presentation should also be inspected with screenshots of the map, both inventory tabs, gates, habitat rotation, slip transit and the hangar.

The cargo layout uses [EVE Online’s inventory screenshots](https://www.eveonline.com/de/news/view/unified-inventory) as a reference for icon stacks, quantity overlays, capacity and filtering.

The shared `StochasticRound` and `StochasticBalance` extension traits provide `.stochastic_round()`, `.withdraw(requested)`, and `.deposit(supplied, capacity)`. They use the default `rand` RNG and keep no fractional remainder. Invalid quantities and capacity violations panic. Physical rates, temperatures, and forces remain floating-point.

Development and release builds use `panic = "abort"` workspace-wide. Invalid outgoing server snapshots panic during encoding before any bytes are written. Socket errors continue to close only the affected connection. Rust's test harness uses unwinding so panic assertions can be tested.

## Economical travel

The standard flight computer minimizes a local time-and-propellant objective, `seconds + seconds_per_kg × kilograms`. It prices one percent of current ship mass as 36 seconds. With constant acceleration and propellant flow, it solves for an economical cruise speed and flies acceleration, coast and braking phases. Feedback corrects lateral error, relative disturbance and changes in available thrust. The terminal envelope also allows for turning. This is a local transfer approximation, not a globally optimal orbital trajectory solver.

The route graph compares estimated seconds for these sublight transfers against slip charging and transit. Nodes include gate mouths and admissible staging points outside their exclusions. The map submits destinations rather than explicit gate queues. Amber connections show queued gate crossings and dashed cyan links show queued slip transits; the gate-only alternative remains visible. The server validates actual departures independently of the planning query.

Ship Status has been removed. The bottom HUD shows thrust, torque, electrical balance, hull integrity and thermal condition. Consumable depletion turns bars amber at or below 25% and red at or below 10%; heat uses amber from 75% and red from 100% of its displayed scale. Shield temperature uses the model's 6000 K vaporization reference, not a hard collapse temperature. Inventory holds the detailed resource list, transponder switch and computer status. Empty storage for reactor waste is not treated as depleted fuel.

The bottom HUD also shows CPU consumption for the last simulation tick. Hover for the gas counts and reserve balance. Faults replace the meter with a red `FAULTED` label and a reboot countdown; the tooltip contains the fault message. An unpowered fault waits for power. Startup and suspended computers show `BOOTING` and `PAUSED`. A fault clears AP, its queue and targets rather than leaving an obsolete route on screen.

The top-left autopilot panel lists the remaining orders with gate crossings in amber and slip transits in cyan. Destination labels name the system, including when its beacon is named for the system on the other side of a gate. Navigation lets the player remove or reorder those same orders. For the default Terminus trip, the computer first uses the nearby Sol gate, transfers outside the mouth exclusion, then slips to Terminus and finishes the local approach.
