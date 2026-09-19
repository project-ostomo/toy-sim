# Industry and cargo

Stations obtain industrial capabilities from installed parts. Refineries process
ore and recover fuel, fuel plants prepare consumables, fabricators produce
ammunition and packaged equipment, and shipyards assemble blueprints. A warehouse
adds ordinary cargo capacity. The station uses its normal electrical and thermal
systems to operate these modules.

Open Industry from the client toolbar to select an authorized facility. Production
shows the recipe, ingredients, products, duration and electrical cost. Jobs shows
progress and the reason for a pause. Management works across the universe; moving
physical cargo requires inventories at the same dock.

## Inventory and Hangar

Cargo holds contain integer quantities of resources and packaged part kits.
Consumables occupy installed tanks. Inventory follows the focused ship and has
Cargo hold and Consumables tabs. It only displays installed resource capacities.
Hangar opens the current dock's accessible storage and docked ships. Open cargo
on another ship to inspect its hold in a separate window, or activate it to switch
ships. Hangar pages contain authorized ships at that dock, independently of the
global facility directory and fleet telemetry limits.

Industry has a Storage tab for the selected facility's inputs and finished goods.
Drag a stack directly between cargo windows to transfer its available quantity.
Hold Shift while dropping to choose a quantity first. Reserved stock stays in its
source. Invalid destinations show the reason, including permissions, distance or
insufficient capacity. Remote industry management does not move cargo remotely.

Drag cargo fuel onto its matching bar in the ship's Consumables tab to refill up
to the tank's remaining capacity. The Fill from own cargo button uses the ship's
hold. Shield coolant replenishes the installed shield reserve the same way. Dock
power supplies electricity separately. Installed consumables cannot be dragged
out of tanks.

Spent and bred reactor fuel remain in installed product reservoirs until unloaded.
They appear in the Cargo tab under Reactor products. Drag a product into a cargo
window to unload it; Shift-drop allows a partial quantity. A ship without a
cargo hold can unload directly into its dock's warehouse. Both inventories require
cargo-transfer permission and physical colocation; unloading into the ship's own
hold is also possible. A full destination leaves the reservoir unchanged.
Operational fuel and propellant cannot be unloaded from consumable tanks.

Refineries accept the resulting cargo. Recovered reactor fuel can then be loaded
through Refill. Onboard fuel processors continue to use their installed reservoirs.

Industrial metals, electronics, reactor construction material and chemical
feedstock use milligram quantities so that manufacturing small parts does not
create or discard fractional kilograms. The UI displays their mass. Other
resources retain their catalogue units, including kilograms of propellant and
individual ammunition rounds.

Starting a job reserves its ingredients in the facility's cargo hold. Reserved
items still have mass and occupy space. Transfers, refills and other jobs cannot
spend them. Cancelling releases the reservation. Completion consumes the complete
bill and inserts all products together; a full output hold pauses the job until
space becomes available.

## Power and materials

Each industrial module supplies a limited number of lanes with a rated electrical
power. Jobs advance in simulation ticks and consume an exact integer share of
their total energy at each step. Missing power or a damaged module pauses work.
Processing energy becomes heat except for the portion stored in charged products.

Recipes conserve material mass, including waste and other products. Equipment kits
contain dry equipment. Reactor alloys and ceramics are structural materials;
fissile fuel requires its own feedstock. Micropulse charges use fissile material
much less efficiently than reactor fuel. A spent-fuel batch recovers two kilograms
of usable reactor fuel from ten kilograms of spent material and leaves eight
kilograms of radioactive waste. Repeated recovery therefore has a finite fuel
yield.

Chemical propellant production pays for the energy stored in the propellant.
Autocannon ammunition consumes this prepared propellant. A packaged Kite missile
includes its dry assembly, avionics, propellant and a charged battery; its recipe
pays for all of them.

## Ship construction

The Shipyard tab accepts a catalogue blueprint or an imported ship design. Its
bill includes every installed part kit, distributed avionics and tank containment.
Assembly needs an operational shipyard and a suitable docking aperture.

A completed ship appears in the station's docked inventory. It starts with empty
tanks, an empty battery and no deployed shield coolant. Select it from the hangar,
refill its tanks, enable dock power and undock when it is ready. Blueprint starting
fill settings do not supply free fuel to a manufactured ship.

Industry permission permits production for the facility's owner. Assigning a new
ship to a different owner also requires permission to withdraw the facility's
cargo and authority over that recipient. The output owner is recorded when the
job starts. These permissions represent access to machinery and storage; laws,
territorial claims and gate restrictions depend on physical in-game enforcement.

## Starting scenario

Neris Anchorage has a refinery, fuel plant, fabricator, shipyard, warehouse and
dock power. The starting player can manage production and use its warehouse. Its
initial supplies include the complete assembly bill for a Kestrel service launch,
industrial materials, ore, water and reactor fuel.

A short first job is Repair material: one kilogram of industrial metals becomes
one kilogram of repair material in one second, consuming one megajoule. The
Kestrel service launch takes 110 seconds and 110 megajoules to assemble. These
durations assume continuous electrical supply and an available module.

Starter mines are explicit resource sources with fixed production rates. They
load authorized, physically colocated cargo inventories and retain their fractional
production remainder and recipient cursor across snapshots. Extraction gameplay
is deferred.

## Persistence and publication

Server snapshots contain inventories, reservations, job progress, output ownership,
blueprints and mine state. Restoration validates their relationships before
replacing the live world. A restored job continues from its recorded progress;
completed construction must not produce a second ship on a later restart.

Clients subscribe to authorized directory pages and selected inventory details.
The server rechecks access on publication. Private inventories are not public
world telemetry. Catalogue data is cached by revision, and cargo updates include
reserved quantities. Publication limits apply to complete inventory records;
omitted inventories are identified explicitly so the client can clear stale
details and ask the player to select fewer inventories.
