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

Settlement populations use independent BLAKE3-derived ChaCha20 streams. The USE's
older industrial core has substantially larger populations than most outer
settlements. The values are simulation parameters, not measurements from the star
survey. Sol begins with 36 billion inhabitants across its planetary and orbital
settlements.

## Data and verification

The [stellar anchor provenance](../crates/toy-sim-universe/data/README.md) describes
the measured catalogue selection and its coordinate conversion. Named fictional
anchors are separate from catalogue observations. Generated map identities and
random streams depend on stable system names or catalogue identifiers, and the
generation version is explicit.

Tests check connectivity, unique names and identifiers, the size of the inhabited
region, sovereign representation, Nova Partenia's independent status, the
connection bound, retention of the original corridor, and the different network
densities of the USE and LFS. A coordinate check places Sirius in the correct ICRS
sky direction, catching accidental reuse of unrotated Galactic coordinates.
