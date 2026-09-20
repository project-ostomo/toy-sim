# Inhabited space in 2426

The starting map contains 3,000 inhabited, wormhole-connected systems in a sphere
125 light-years in radius around Sol. The calendar remains real UTC plus 400
years. The political geography follows the worldbuilding chronology and politics
handoff; the specific regional boundaries and local sovereignty names are authored
choices for this prototype.

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

The ten existing authored systems keep their names and identities. Sol is moved to
the coordinate origin. Helion is placed in Helion Commonwealth, about 91
light-years from Sol. The other authored systems anchor their respective outer
regions, including Elysium in Nova Partenia and Havoc in Concord Free State. Their
original chain of connections remains as a named corridor through the wider map.
These nine bilateral links are retained infrastructure, rather than a claim that
all other stars lie along that chain.

The generated starting allocation contains 369 USE systems, 2,111 systems across
the six LFS member states, and 520 independent systems. Nova Partenia has 62 of
the independent systems. The network contains 6,104 gate pairs, with a maximum
shortest path of 32 gate hops between any two systems.

## How the network is built

Each region first expands from its authored anchor to progressively more distant
settlements. A new settlement attaches to the nearest existing settlement in its
sovereignty that has capacity for another connection. The shared spatial hash
supplies these searches. The resulting branches preserve the economic history of
expensive sublight mouth-seed delivery.

The LFS then adds nearby redundant connections until its ordinary settlements
have roughly four routes each. Independent regions add more modest local
redundancy. A small number of mature USE core crosslinks and postwar connections
between blocs complete the network. Every system has at most six connections.
This bound controls the size of the initial authored infrastructure; it is not a
physical law that forbids future construction.

The result is a connected graph with a visibly sparse USE trunk structure and a
more redundant LFS mesh. Link kinds describe that construction history: Trunk,
Regional, Mesh, or Concord. They do not carry transit permissions. A government
that wants to stop a ship must physically guard or disable its infrastructure.
Ordinary gate geometry, speed, and exclusion rules apply independently of the
political map.

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
cargo-transfer and navigation APIs. Defense uses sensors and weapons; a hostile
standing cannot deny passage through a gate by itself.

When explicitly enabled, asynchronous LLM directors receive their organization's
lore, objectives and permitted observations, and issue high-level orders through
a bounded tool interface. Radio-capable ships run chatter firmware through the
same metered LLM service available to other ship programs. See
[language-model calls](llm.md) for provider setup and the shared spending cap.

## Data and verification

The [stellar anchor provenance](../crates/osg-universe/data/README.md) describes
the measured catalogue selection and its coordinate conversion. Named fictional
anchors are separate from catalogue observations. Generated map identities and
random streams depend on stable system names or catalogue identifiers, and the
generation version is explicit.

Tests check connectivity, unique names and identifiers, the size of the inhabited
region, sovereign representation, Nova Partenia's independent status, the
connection bound, retention of the original corridor, and the different network
densities of the USE and LFS. A coordinate check places Sirius in the correct ICRS
sky direction, catching accidental reuse of unrotated Galactic coordinates.
