# Stations, navigation and inventory

Build both the server and debug launcher with `cargo build -p osg-server -p osg-debug`, then run `cargo run`. The default Peregrine expedition patrol has a 4 m micropulse drive, two lasers, a 250 MW thermal fission reactor, batteries and a fitted 500 MW slipdrive. The smaller water-NTR patrol remains available as `assets/ships/ntr-patrol.ship`. A hostile patrol starts 1 km away, with Neris Anchorage nearby.

Interstellar travel uses committed slip trajectories that end at the first natural exclusion sphere they intersect. The initial scenario places navigation installations throughout its 3,000 settlement locations. Every catalogue system is a possible natural destination, including systems absent from the public inhabited directory.

## Client controls

- Select a ship in Overview to align, approach or keep range. These actions replace the navigation queue. Keep range continues until interrupted; approach and align finish when their conditions are met. Mark target and Start firing are separate weapons actions. Stop firing retains the mark; Unmark target clears it and stops firing. Navigation never enables firing.
- Celestial selections offer **Slip to** in Selected Item and the Overview context menu. It queues a natural capture at that body and is disabled while the ship intersects a celestial exclusion sphere. Hold Shift to append it to the queue.
- Select a public installation to approach or dock when it offers docking service. Hold Shift to append these orders.
- Open Navigation map from the left toolbar. Search or select a system to preview a route, then use Set destination or Add waypoint. Drag empty map space to pan and scroll to zoom. Celestial positions and definitions come from the client's shared catalogue; the server supplies the current inhabited directory and gameplay infrastructure.
- The browser, search and Fit view show inhabited systems plus active or preview route stops and the ship's current system. Uninhabited intermediate stops appear while their route is displayed. Slip legs use their planned failure probability: green through 100 ppm, yellow through 1,000 ppm, orange through 10,000 ppm, and red above that; gray means unknown. Map labels and preview rows show the numerical risk.
- Set Maximum ship-destruction risk in ppm for the complete itinerary. The decimal input and logarithmic slider control the same value. Route previews show estimated loss, the selected maximum, beacon assumptions, exotic fuel and the conventional fuel budget. The default is 100 ppm. Change the preference and request a new route to apply it.
- Fuel allowance limits estimated use of each remaining propulsion resource. Required fuel and available balances appear in the route preview and Navigation, with an exhaustion warning and estimated shortfalls. Partial estimates are labelled. Departure clearance, charging and matching destination motion contribute to travel estimates.
- Open Navigation to inspect, remove or move pending commands earlier. Pause and resume preserve the queue. Clear queue cancels it.
- The location HUD lists remaining route stages with an approximate cumulative ETA for each timed stage. The active estimate updates during flight and slip charging. Continuous commands and unknown estimates are labelled explicitly. Numbered waypoint markers include docking destinations, slip targets, galactic coordinates and relative positions; dotted lines connect the route. Offscreen waypoints use edge markers. Distances of 0.1 light-years or more display in ly.
- Look at is available for ships within 100 km in the subscribed picture. Escape returns to the controlled ship.
- Docking changes the view to a private hangar. Right-drag empty space to spin the camera around the ship; scroll to zoom. Undock is available in the location display. Stored ships have no active physics body.
- Inventory has Cargo hold and Consumables tabs. Cargo uses integer icon stacks; consumables show installed tanks, battery charge and shield reserve. Resources without tank capacity are omitted. Select a cargo stack and another controlled, co-docked ship to transfer an integer quantity. Tanks never draw from cargo and their contents cannot be transferred.

Double-click empty scene space to align the controlled ship along the camera ray. Right-drag orbits the camera with a 40 ms exponential angular response; the mouse wheel zooms.

## Risk and natural capture

The selected risk is an allowance for estimated slip loss across the complete
itinerary. The planner composes the individual leg probabilities, chooses speeds
and intermediate captures, and preserves the remaining allowance through automatic
replanning. It reports the estimate separately from the requested maximum.

| Maximum risk | Equivalent chance of loss |
| --- | --- |
| 1 ppm | 1 in 1,000,000 |
| 10 ppm | 1 in 100,000 |
| 100 ppm, the default | 1 in 10,000 |
| 1,000 ppm | 1 in 1,000 |
| 10,000 ppm | 1% |

The control accepts decimal values from 0 through 1,000,000 ppm. Zero excludes
slip legs with nonzero modelled loss; 1,000,000 ppm removes the probability
restriction. The estimate covers slip capture, with the displayed beacon
availability assumptions. Combat and ordinary piloting hazards are outside this
estimate. A missed intended capture counts as a loss in route estimates, even
though a subsequent unplanned natural capture could rescue the ship.

Every physical celestial body contributes an exclusion sphere:

```text
R = 0.008 AU × (body mass / solar mass)^(1/3)
```

The exclusion regions are the union of those spheres. Barycentres contribute no
additional mass or sphere. Engagement requires clearing the regions with the
whole ship. Once engaged, the ship follows its committed direction at the chosen
speed. The first natural exclusion intersection ends slip. Bodies move during
flight, and dormant systems participate in capture queries. Physical collision
destroys the ship; a body whose surface extends outside its exclusion sphere is
an unsafe target.

Arrival retains the ship's galactic velocity. The next stage may require a burn
to match the destination's motion, clear an exclusion sphere, or travel around a
region that would intercept the onward trajectory. Docking and movement deep
inside a gravitational well use ordinary propulsion. Cancelling or pausing a
queue cannot manually disengage an ongoing slip. A ship that captures nothing
before exhausting its exotic fuel is lost.

### Dispersion and beacon guidance

Angular error has two independent Gaussian components. With speed `v` in
light-years per second, each component's standard deviation is:

```text
sigma(v, B) = max(1.0743925808301219e-8, 0.00009549549877340154 × v²) / B
B = 1 for blind travel; 36 with authenticated navigation guidance
```

For an isolated target of radius `R` at distance `D`, using the same distance
units, the modelled capture probability is:

```text
P(capture) = 1 − exp(−R² / (2 × D² × sigma²))
loss in ppm = (1 − P(capture)) × 1,000,000
```

The hard floor means that sufficiently slow travel gains no further precision.
A blind jump aimed at a solar exclusion sphere 10 light-years away has a maximum
capture probability of 50%. At the default 100 ppm budget, the maximum solar
single-leg ranges are approximately 2.74 light-years blind and 98.8 light-years
with guidance. Several legs must divide the itinerary's total risk allowance.

The speed coefficient calibrates a guided 10-light-year solar capture to about
five minutes at 100 ppm, excluding charging and conventional burns. The maximum
selected speed is 1 light-year per second. A blind 10-light-year trip cannot gain
100 ppm reliability merely by taking longer; it needs suitable intermediate
captures or a different risk allowance.

The ppm allowance constrains route planning. The controller chooses the actual
aim, and the server executes it without correcting it toward the planned target
or vetoing it for excessive risk. A buggy controller can miss every capture
sphere and exhaust its fuel. Risk estimates assume the controller follows the
planned trajectory.

The server checks beacon operation and authorization throughout transit. First
loss adds an independent angular error from the ship's current position, with
deviation `sqrt(sigma_blind² − sigma_assisted²)`. Speed remains fixed, and that
jump remains blind even if the beacon becomes available again. Saved transit
state includes the sampled errors, preventing reloads from rerolling the jump.

### Charging and exotic fuel

Electrical preparation costs 500 kJ per kilogram per light-year and lasts at least ten seconds.
Retargeting updates the required energy while preserving charge already paid. Actual charging time also depends on supplied
power, so a 500 MW drive does not imply that the ship can continuously supply
500 MW. Ordinary motion continues during charging.

Exotic fuel consumption for one uninterrupted slip is:

```text
C(d) = 1e-5 × M × d^1.2 kilograms
M = departure mass in kilograms
d = distance travelled in light-years
```

Inventory records exotic fuel in grams. The host debits increments of cumulative
consumption and retains fractional grams, so accounting is independent of tick
subdivision. Consumption follows actual distance, including flight beyond a
missed target. Each new jump snapshots its own departure mass and begins a new
distance total.

Starter ships receive approximately 1,000 light-years of uninterrupted endurance,
including the reserve's own mass, in a 5 m³ exotic-fuel tank. The default 50% fuel
allowance covers about 561 ly, over twice the distance from Helion to the furthest
initial USE system, leaving fuel for routes through unaided intermediate captures.
Exotic fuel is ordinary cargo supplied through existing transfer logistics;
Neris Anchorage starts with stock. Tank propellants and coolant keep their
existing storage rules. Loading more cargo changes the mass used for the next
jump. Transit trajectories, fuel accounting and remaining risk allowances survive
checkpoints.

## Construction

Ship format 3 stores one root and an attachment tree. Each connection names a parent socket, child plug and quarter-turn roll. Connector strings must match exactly and each node may be used once. The editor shows matching sockets and previews the assembled pose. R changes roll; Delete removes the selected part and its attached descendants.

Catalogue nodes specify a position in metres and an outward cardinal normal. Hull connectors include their diameter, such as `hull_2`; utility mounts use `equipment`. Station structural, docking and directory-transmitter connections use `station_backbone`. The habitat exposes equipment nodes on its fixed hub. Parts can provide their own node list.

Dimensions remain in decimetres for catalogue authoring. They do not define a construction grid. The server voxelizes the assembly on a coarse 1 m grid and merges cells into collision boxes. Explicit cylindrical, habitat and hangar profiles preserve open regions. Habitat spoke sweeps are conservative collision regions; the ring animation runs only on clients and in the editor.

## Station assets

The separate Blender library is [station-catalogue.blend](../assets/models/stations/station-catalogue.blend). Its construction/export script is [stations.py](../tools/art/stations.py). Ship-part artwork remains in its existing Blender files.

| Part | Dimensions | Function |
| --- | --- | --- |
| Station backbone | 32 m diameter × 64 m | Tank volume and equipment attachment nodes |
| Station bulkhead | 32 m diameter × 8 m | Structural end closure |
| Habitat | 200 m diameter × 48 m | Fixed hub, two counterrotating habitation rings, 10,000 crew capacity |
| Docking hangar | 64 m diameter × 96 m | Open mouth, 25 m ship-radius limit, storage ingress |
| Directory transmitter | 48 × 48 × 64 m | Advertises its host through the public inhabited directory when operational and the host transponder is lit |
| Navigation beacon | 4 × 4 × 4 m | Authenticated slip guidance reference |

The habitat ring speed is `sqrt(g / 94 m)` in opposite directions. GLB node extras carry `angular_velocity_rad_s`; the shared rendering plugin evaluates rotation from simulation time. Neris Anchorage combines these modules with a fission reactor, radiator, life support, cargo storage and a ship-class gun.

## Runtime contracts

The host owns `TravelState`, strategic route planning, the remaining risk allowance and presence changes. Standard firmware executes local guidance, target tracking, slip preparation and docking approach. Revision checks reject stale queue edits. Contact guidance uses the fused information group. An unavailable target blocks and retries the command.

Directory and navigation capabilities come from installed, functioning equipment. Any host with an operational directory transmitter and a lit transponder publicly inhabits every system whose gravitational influence contains it. A conventional ship transponder alone does not advertise a system. The same hardware rules apply when a player builds or moves an installation; no separate station category determines membership. The last qualifying broadcaster going dark, leaving or being destroyed removes public membership.

Navigation guidance additionally requires a functioning navigation beacon and `Navigate` access. Public directory membership does not itself grant guidance access. An authorized ship can use a known private beacon without making its system public. Beacons are reference devices: celestial bodies supply the physical capture regions.

USE navigation installations start with no public access grant and grant `Navigate` to the USE sovereignty through the ordinary access policy.

Docking is available from any direction within 100 m surface clearance of the station (centre distance minus both bounding radii), at no more than 10 m/s relative speed. The autopilot approaches from the ship's current side and brakes before requesting capture. It does not fly into a hangar or match its orientation. A docking bay determines access and the size and mass of ships that can dock. After docking, ships enter the host's ECS storage relationship and release its reservation, so the same bay can serve multiple stored ships. Hull, cargo, tanks, hardware and ownership remain on the stored entities. Host mass includes them. Destruction preserves them as wreck inventory. Checkpoints restore stored ships and their host relationships.

Conventional consumable counts are `u64`. A fractional request uses stochastic rounding with the default `rand` RNG; a request for 2.4 units removes 2 or 3 with probabilities 0.6 and 0.4. Continuous supplied demand drives smooth thrust, while actual integer debits determine remaining stock and mass. Exotic fuel uses the cumulative fractional accounting described above. Battery charge and capacity are integer joules. Electrical producers and consumers use stochastic rounding at the accounting boundary; dock power transfers debit and credit the same integer amount. The shield coolant reserve is stored in integer milligrams, with stochastic withdrawals into the continuously simulated shield. Reactor conversion and processing conserve actual integer material counts.

## Verification

```sh
cargo test -p osg-server --lib infrastructure::tests
cargo test -p osg-server --lib default_patrol_encounter
cargo test -p osg-server --lib default_ntr_patrol_reverses_heading
cargo test -p osg-ships --test attachments
cargo test -p osg-ships --test ships
cargo test -p osg-server --test network
```

The docking tests run the stock WASM flight computer through approach and capture, checking station clearance, range and speed boundaries, arbitrary arrival attitude, bay access and compatibility. Travel checks cover moving-body capture, retained velocity, physical collision, fuel exhaustion, fractional accounting and beacon loss. Directory checks cover equipment, transponder state, destruction and overlapping influence regions. Client verification uses headless screenshots of the map, risk controls, inventory, slip transit and hangar.

The cargo layout uses [EVE Online’s inventory screenshots](https://www.eveonline.com/de/news/view/unified-inventory) as a reference for icon stacks, quantity overlays, capacity and filtering.

The shared `StochasticRound` and `StochasticBalance` extension traits provide `.stochastic_round()`, `.withdraw(requested)`, and `.deposit(supplied, capacity)`. They use the default `rand` RNG and keep no fractional remainder. Invalid quantities and capacity violations panic. Physical rates, temperatures, and forces remain floating-point.

Development and release builds use `panic = "abort"` workspace-wide. Invalid outgoing server snapshots panic during encoding before any bytes are written. Socket errors continue to close only the affected connection. Rust's test harness uses unwinding so panic assertions can be tested.

## Economical travel

The standard flight computer minimizes a local time-and-propellant objective, `seconds + seconds_per_kg × kilograms`. It prices one percent of current ship mass as 36 seconds. With constant acceleration and propellant flow, it solves for an economical cruise speed and flies acceleration, coast and braking phases. Feedback corrects lateral error, relative disturbance and changes in available thrust. The terminal envelope also allows for turning. This is a local transfer approximation, not a globally optimal orbital trajectory solver.

The route search compares estimated conventional transfers with charging and
slip transit between natural capture targets. It queries nearby candidates from
the catalogue and resolves detailed systems as needed. A weighted A* search
prioritizes progress toward the destination, including assisted intermediate
systems, and evaluates expensive clearance and transit forecasts lazily. Risk
allocations share one search frontier, with at most 32 slip legs. Requests have
a two-second computation budget, reserving time for exact itinerary validation;
the search can return a feasible route without proving global optimality. The
planner reports when no route meets the allowance within its search budget.
It validates the planned itinerary's geometry, fuel and cumulative risk. Actual
controller aim remains unrestricted by route feasibility or risk forecasts.

Ship Status has been removed. The bottom HUD shows thrust, torque, electrical balance, hull integrity and thermal condition. Consumable depletion turns bars amber at or below 25% and red at or below 10%; heat uses amber from 75% and red from 100% of its displayed scale. Shield temperature uses the model's 6000 K vaporization reference, not a hard collapse temperature. Inventory holds the detailed resource list, transponder switch and computer status. Empty storage for reactor waste is not treated as depleted fuel.

The bottom HUD also shows CPU consumption for the last simulation tick. Hover for the gas counts and reserve balance. Faults replace the meter with a red `FAULTED` label and a reboot countdown; the tooltip contains the fault message. An unpowered fault waits for power. Startup and suspended computers show `BOOTING` and `PAUSED`. A fault clears AP, its queue and targets rather than leaving an obsolete route on screen.

The autopilot panel lists the remaining orders and names their target systems.
Navigation lets the player remove or reorder pending commands. A journey to
Terminus may combine conventional departure clearance, one or more natural
captures and final local approach, depending on the selected risk, available
guidance and resource budget.

The shared formulae are implemented in
[travel/slip.rs](../crates/osg-model/src/travel/slip.rs). The authoritative transit
state and intersection logic live in
[the server slip engine](../crates/osg-server/src/sim/travel/slip.rs), and strategic
search lives in [routing](../crates/osg-server/src/sim/routing/mod.rs).
