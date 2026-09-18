# Equipment catalogue

The catalogue is authored in `crates/toy-sim-ships/data/catalogue.toml`. Equipment types define behaviour; individual part records supply performance parameters, dimensions, mass, material capacity, and model references. Visual scaling does not implicitly change simulation parameters.

## Models

`assets/models/parts/parts-catalogue.blend` contains a separate editable scene for each new model and a `Parts catalogue` overview. The library uses octagonal attachment frames, grey Whipple shielding, titanium supports, dark thermal surfaces, and recessed optical elements. The existing fuselage and micropulse drive sources remain separate.

Run `tools/art/catalogue.py` inside Blender with `runpy.run_path(absolute_path, run_name="__main__")` to regenerate the library. The script exports one GLB per family and writes `catalogue-models.json` with bounds and dimensions. Models use metres, a centred origin, and +Z exhaust. Scaled sizes share their family GLB. Frame, truss, and adapter geometry supplies the corresponding structural catalogue parts.

## Propulsion and fuel

Micropulse drives consume complete manufactured charges. Their electricity recovery exists only during a burn. Smaller drives have lower specific impulse, so equal manoeuvres can cost more charge mass. The charge plant consumes reactor fuel and bulk repair material; charge energy utilization is deliberately much worse than sustained reactor utilization. Manufacturing coefficients describe abstract game inventories, not a physical device design.

Thermal engines contain their own propulsion reactor. Hydrogen variants offer greater exhaust velocity with bulky storage; water variants trade performance for dense, convenient propellant. Propellant and reactor fuel are consumed together, and exhausted reactor fuel must fit in spent-fuel storage. A shortage reduces thrust. Prompt waste and a decaying heat reservoir remain after a burn. An excessively hot cooling sink inhibits firing.

Electric engines consume their specified propellant and electricity. Catalogue validation checks exhaust kinetic power against electrical input. Resistojets likewise use explicit water or hydrogen supplies. The small auxiliary generator burns a separate conventional fuel and is intended for emergency power.

## Tank containment

Tank allocation reserves gross internal volume. The selected resource supplies a storage class and coefficients for usable volume and containment mass. Initial fill applies to usable volume. Containment contributes dry mass, centre of mass, and inertia even when a tank is empty. The editor shows gross allocation, usable volume, and containment mass.

Hydrogen's low density and containment overhead make its tanks bulky. Storage has no refrigeration demand or passive fuel loss.

## Reactors and industry

Compact, high-temperature, and breeder reactors store core heat independently of ship heat. Fuel releases thermal energy, with a fraction retained in a decay reservoir. A finite heat-transfer coefficient moves energy from the core to the ship's cooling sink. Electrical conversion uses actual core temperature, capped by the authored operating temperature, and the sink temperature. Converter quality scales the Carnot bound.

Reactor demand is controlled through the existing generator interface. A full battery limits new fuel consumption. High sink or core temperatures cause shutdown, while accumulated core heat can still damage a shut-down reactor. Decay heat continues after shutdown and while docked.

Breeders consume fertile feedstock and accumulate bred material while burning reactor fuel. A separate automatic processor converts bred material to usable fuel, retaining processing losses as spent material. Exhausted spent fuel cannot be turned back into full-energy fuel. Output storage and available electricity bound processing. The charge plant is another automatic processor; it fabricates drive charges only when all inputs and output capacity are available.

Fuel utilization, breeding rates, converter quality, and operating temperatures remain explicit authored parameters. The system does not attempt a neutron transport or detailed chemical processing simulation.

## Cooling

Shields retain the existing radiating envelope and consumable reserve behaviour. Auxiliary radiators use emitting area and emissivity to remove stored heat. Emergency coolers expel water above an authored hull-heat threshold. Heat sinks add storage before hull damage starts.

For equipment coupled to the hull reservoir, the coolant temperature is an abstract mapping of stored heat fraction. It is not the average temperature of the hull. Active shields instead supply their radiating temperature. Cooling is capped by available stored energy, and damaged equipment does not operate.

Docked ships transfer accumulated heat to their host. Their reactors cease fission but continue releasing stored and decay heat. Docked radiators do not radiate directly to space from inside the host.

## Utilities

Installed command modules provide the electrical requirement for the ship computer. Powered sensor modules extend the existing sensor picture and use its fusion and uncertainty rules. Active and passive arrays currently differ by catalogue characteristics; there is no separate active-emission detection model. IFF remains part of ship identity and host state.

Powered beacon equipment enables navigation broadcasts. Docking ports and hangars create bays in the existing docking system, which removes docked ships from active flight physics. Bay occupancy survives hardware reconstruction.

Cargo handlers and power couplers serve docked ships belonging to the same owner. The docked ship must explicitly request cargo and/or power service using `SetDockServices`. Transfers respect authored rate limits and destination capacity. Cargo resupply targets allocated tanks. Service preferences are included in telemetry and cleared after leaving the dock.

Crew quarters establish crew capacity and population. Life support consumes supplies and electricity, and reports the supported fraction. There is no crew death simulation. Workshops consume repair material and electricity to restore hull integrity; they do not fabricate replacement ships.

## Weapons

Railguns use the existing projectile simulation. Lasers consume electricity and deposit beam energy into the first intersected shield or hull within range. They use the normal aiming interface, have no ammunition requirement, and create no projectile body. Point-defence naming identifies a fast-slewing laser family; autonomous projectile interception remains a script policy. Missiles are deferred until ships can launch other ships.

## Example ships

Generate the examples with:

```sh
cargo run -p toy-sim-ships --example write_catalogue_examples --offline
```

Open any of these in the ship editor and select **Launch sim**:

- `assets/ships/micropulse-patrol.ship`: charge-fuelled patrol craft with a reactor and railgun.
- `assets/ships/ntr-utility.ship`: water thermal propulsion with docking and cargo handling.
- `assets/ships/breeder-freighter.ship`: breeder power, fuel processing, electric propulsion, and cargo storage.

The default server scenario starts two micropulse patrol ships 1 km apart; the second aims and fires at the player after booting. The editor Starter button creates the larger micropulse demonstrator. The examples provide initial tank allocations and batteries. Reactor startup, limited cooling, power demand, and resource exhaustion remain part of their operation. Parameters are prototype balance values.
