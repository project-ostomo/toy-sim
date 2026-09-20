# Inhabited space in 2426

The starting scenario places navigation installations at 3,000 settlement
locations in a sphere 125 light-years in radius around Sol. These locations are
a recipe for creating a new world. Current public inhabitation is derived from
the equipment and broadcasts of the objects that actually exist.

The full universe contains 1,001,760 system roots from the Gaia catalogue,
enriched nearby astronomical records and ten authored systems. All procedural
systems use the same deterministic generator. Systems outside the initial
settlement region are available for inspection, natural capture and construction.
The calendar remains real UTC plus 400 years. The political geography follows
the worldbuilding chronology and politics handoff; regional boundaries and local
sovereignty names are authored choices for this prototype.

The USE holds Sol, the central 52-light-year sphere, and an outward corridor of
older trunk infrastructure. Six LFS member sovereignties occupy most of the outer
territory: Helion Commonwealth, Aurora Compact, Meridian League, St Raphael
Commonwealth, Vesper Freeports, and Lyra Research Compact. Each is sovereign; the
League is their defense and arbitration association. Three independent states
complete the starting political map: Nova Partenia, Concord Free State, and
Terminus Protectorate.

Nova Partenia is permanently neutral. It provides a territorial base for the Holy
See, rather than a homeland for every Catholic in inhabited space. Its independent
status is distinct from Catholic communities in LFS member states such as St
Raphael Commonwealth. Political affiliation and treaty neutrality describe claims
and institutions. They grant no automatic protection against weapons.

The ten authored systems have explicit stable keys. Sol lies at
the coordinate origin. Helion is placed in Helion Commonwealth, about 91
light-years from Sol. The other authored systems anchor their respective outer
regions, including Elysium in Nova Partenia and Havoc in Concord Free State. Their
positions anchor the starting scenario's geography. Travel uses slip trajectories
with natural capture and ordinary propulsion.

Jurisdiction metadata describes the starting territorial claims. It does not
decide whether a system currently appears as inhabited or grant automatic access
to its facilities.

## Public inhabitation

A functioning directory transmitter advertises its host while that host's
transponder is lit. Each system whose gravitational influence contains the host
appears in the shared public inhabited set. This behavior comes from equipment;
there is no special station category. A ship's ordinary transponder alone does
not establish public inhabitation.

Building the first broadcasting installation adds a system. Destroying its last
transmitter, disabling power or the transponder, or moving its last broadcaster
outside the influence boundary removes it. Additional broadcasters keep it
listed while any qualifying transmitter remains. An installation in overlapping
influence regions advertises each affected system; one in interstellar space
advertises no system.

Clients synchronize this global set with the server. It represents currently
broadcast public knowledge. A system containing only ships or dark installations
does not appear in the set. Such objects still exist and can keep server physics
active. Crew count and political labels are not inputs to directory membership.

The navigation map derives sovereignty from the same broadcasting installations.
Each installation contributes one vote to its legal owner's organization; a
player-owned installation uses that player's organization. An organization must
own more than half of the broadcasting installations in a system to claim it.
The map displays that organization's sovereignty. Ties and other results without
a strict majority are unclaimed. Separate organizations do not pool their votes.
Unaffiliated installations count toward the total but cast no organization vote.
Ownership transfers, affiliation changes, power loss and destruction update the
claim. Advertised transponder factions cannot change legal ownership.

Navigation beacons provide authenticated guidance independently of directory
visibility. Access to that service uses the `Navigate` permission. A public
listing does not grant access, and a known private beacon can provide guidance
to authorized ships. Natural celestial exclusion spheres determine capture.

Loading a checkpoint restores surviving installations and recomputes directory
membership. It does not rerun the new-world placement recipe or recreate
destroyed infrastructure and consumed stock.

## Generation and activity

Client and server share the stellar inputs, authored definitions, generator
revision, stable identities and orbital epoch. The connection checks a universe
fingerprint. Celestial definitions and ephemerides are evaluated independently
by each process. A client can inspect or render a distant system without asking
the server to activate it or stream its body definitions.

The compact catalogue contains enough information for spatial queries, distant
star rendering and conservative influence bounds. Detailed planetary systems
are generated only when resolved. Each process retains up to 128 idle
definitions in its cache; consumers can keep definitions they are using.
Eviction changes neither identities nor regenerated content.

Server activity follows actual objects inside gravitational influence regions.
Ships outside slip, stations, dark installations, wrecks and other physical
objects keep overlapping systems active. Slipping ships alone do not. Queries
for navigation, inspection and slip intersections may resolve dormant systems
without starting their periodic simulation. On capture, the necessary celestial
state is available before ordinary physics resumes. Celestial positions are
evaluated at the current epoch when a system becomes active again.

The initial navigation installations therefore keep their occupied systems
active. This follows from their physical presence. The same rules apply to a
new installation at any other catalogue location.

## Organizations and physical populations

The starting world includes 108 organizations with authored histories, cultures,
doctrines, goals and relationships. Their profiles live in the
[USE](../crates/osg-universe/data/organizations-union.json),
[LFS](../crates/osg-universe/data/organizations-league.json) and
[independent](../crates/osg-universe/data/organizations-independent.json)
rosters. Organization membership sits below sovereignty and above individual
accounts. Friendly, neutral and hostile standings affect client identification
and NPC decisions.

Organizations receive physical facilities and vessels suited to their roles:
freighters, patrols, research ships or broadcasters. Their inventories, losses,
orders and ownership survive checkpoints. Logistics uses ordinary production,
cargo-transfer and navigation APIs. Defense uses sensors and weapons. Facility
access and navigation guidance permissions apply independently of territorial
claims.

When explicitly enabled, asynchronous LLM directors receive their organization's
lore, objectives and permitted observations, and issue high-level orders through
a bounded tool interface. Radio-capable ships run chatter firmware through the
same metered LLM service available to other ship programs. See
[language-model calls](llm.md) for provider setup and the shared spending cap.

## Data and verification

The [stellar anchor provenance](../crates/osg-universe/data/README.md) describes
the measured catalogue selection and its coordinate conversion. Named fictional
anchors have explicit identities. Procedural random streams depend on catalogue
identity and stable local body paths; display names do not determine identity.
The generator revision is included in the universe fingerprint. Exact source-ID
aliases reconcile overlapping catalogue inputs, and known companions belong to
their owning systems. See [the catalogue format](gaia-catalogue.md).

Tests check lazy full-catalogue loading, stable identities through renaming and
cache eviction, conservative companion bounds, and reproducible orbital motion.
Directory tests cover equipment and transponder changes, multiple emitters and
overlapping influence regions. A coordinate check places Sirius in the correct
ICRS sky direction. The initial settlement tests retain unique names and
identifiers and the authored jurisdictions.

The shared resolver lives in
[universe.rs](../crates/osg-universe/src/universe.rs). Server activity is managed
in [orrery/activity.rs](../crates/osg-server/src/sim/orrery/activity.rs), and public
membership is built in
[infrastructure/navigation.rs](../crates/osg-server/src/sim/infrastructure/navigation.rs).
Player-facing risk, fuel and capture rules are documented in
[stations and navigation](stations-navigation.md).
