#!/usr/bin/env python3
"""Sequential wormhole-network crawl helper.

Pops the next system from queue.txt, pushes its unvisited gate neighbours
(nearest to Sol first), and prints a dossier used to write SYSTEMS-LORE.md.
Visited systems are the indices recorded in SYSTEM-NAMES.txt.

Usage:
    python3 crawl.py init   # seed queue.txt with the closest system to Sol
    python3 crawl.py        # process the next system from the queue
"""

import json
import math
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent          # .../SYSTEM-NAMING
REPO = ROOT.parent                              # OpenSpaceGame repository root
DUMP = Path("/tmp/opencode/inhabited-map-dump.json")
QUEUE = ROOT / "queue.txt"
NAMES = ROOT / "SYSTEM-NAMES.txt"
LORE = ROOT / "SYSTEMS-LORE.md"


def load():
    if not DUMP.exists():
        subprocess.run(
            ["cargo", "run", "--release", "-p", "osg-universe", "--example", "map_dump"],
            cwd=REPO,
            check=True,
            capture_output=True,
        )
    data = json.loads(DUMP.read_text())
    systems = data["systems"]
    adjacency = {}
    for link in data["links"]:
        adjacency.setdefault(link["a"], []).append((link["b"], link["kind"]))
        adjacency.setdefault(link["b"], []).append((link["a"], link["kind"]))
    return systems, adjacency


def visited(names_file: Path):
    if not names_file.exists():
        return set()
    return {int(line.split("|")[0]) for line in names_file.read_text().splitlines() if line.strip()}


AUTHORED_POSITIONS = {
    "Sol": [0.0, 0.0, 0.0],
    "Helion system": [90.0, 10.0, 5.0],
    "Aurora system": [40.0, 83.0, 40.0],
    "Meridian system": [-55.0, 65.0, 45.0],
    "Cinder system": [30.0, -68.0, 60.0],
    "Vesper system": [-30.0, -85.0, -15.0],
    "Lyra system": [20.0, 20.0, -100.0],
    "Elysium system": [-75.0, 25.0, -20.0],
    "Havoc system": [-75.0, -40.0, 60.0],
    "Terminus system": [-70.0, -20.0, -70.0],
}


def distance_ly(system):
    position = system["position_ly"] or AUTHORED_POSITIONS.get(system["name"])
    if not position:
        return math.inf
    return math.dist(position, [0.0, 0.0, 0.0])


def spectral_class(temperature_k):
    if temperature_k is None:
        return "?"
    for limit, label in [
        (30000, "O"), (10000, "B"), (7500, "A"),
        (6000, "F"), (5200, "G"), (3700, "K"),
    ]:
        if temperature_k >= limit:
            return label
    return "M"


def pop_queue():
    lines = QUEUE.read_text().splitlines()
    if not lines:
        return None, []
    return lines[0].strip(), lines[1:]


def push_queue(rest, entries):
    QUEUE.write_text("\n".join([*(f"{i}|{n}" for i, n in entries), *rest]) + "\n")


def parse_entry(line):
    index, _name = line.split("|", 1)
    return int(index), _name


def record_name(index, name, date=None):
    with NAMES.open("a") as file:
        file.write(f"{index}|{name}|{date or ''}\n")


def main():
    systems, adjacency = load()

    if sys.argv[1:] and sys.argv[1] == "init":
        ranked = sorted(
            (s for s in systems if s["name"] != "Sol" and s["position_ly"] and adjacency.get(s["index"])),
            key=distance_ly,
        )
        closest = ranked[0]
        QUEUE.write_text(f"{closest['index']}|{closest['name']}\n")
        NAMES.touch()
        print(f"queue initialized with {closest['name']} ({distance_ly(closest):.2f} ly)")
        return

    seen = visited(NAMES)
    line, rest = pop_queue()
    if line is None:
        print("queue empty")
        return
    index, current_name = parse_entry(line)
    system = systems[index]
    if index in seen:
        push_queue(rest, [])
        print(f"{system['name']} already named; skipped")
        return

    neighbors = adjacency.get(index, [])
    fresh = [(other, kind) for other, kind in neighbors if other not in seen]
    fresh.sort(key=lambda pair: distance_ly(systems[pair[0]]))
    # Keep already-queued systems out of the new batch; duplicates deeper in the
    # stack are harmless (they are skipped on pop).
    pending = {parse_entry(l)[0] for l in rest}
    push_queue(rest, [(other, systems[other]["name"]) for other, _ in fresh if other not in pending])

    print(f"=== popped #{index}: {system['name']}")
    print(f"catalogue_id : {system['catalogue_id']}")
    print(f"sovereignty  : {system['sovereignty']} ({system['alignment']})")
    print(f"distance     : {distance_ly(system):.2f} ly from Sol")
    print(f"star         : {spectral_class(system['temperature_k'])}-class, "
          f"L={system['luminosity_solar']}, T={system['temperature_k']} K, "
          f"companions={system['companions']}")
    if NAMES.exists():
        tail = NAMES.read_text().splitlines()[-3:]
        print(f"chronology   : last named -> " + " | ".join(tail))
    for star in system.get("stellar", []):
        print(f"star         : {star['name']} — {star['kind']}, {star['spectral_class']}, "
              f"{star['mass_solar']:.2f} Mo, T={star['temperature_k']} K")
    for planet in system.get("planets", []):
        if planet["mass_earth"] < 0.01 and not planet["biosphere"] and planet["ocean_fraction"] == 0:
            continue
        notable = []
        if planet["atmosphere"]:
            notable.append("atmo")
        if planet["ocean_fraction"] > 0:
            notable.append(f"ocean{planet['ocean_fraction']:.2f}")
        if planet["biosphere"]:
            notable.append("BIOSPHERE")
        if planet["tidally_locked"]:
            notable.append("locked")
        if planet["moons"]:
            notable.append(f"{planet['moons']}moons")
        print(f"  planet     : {planet['name'][:30]:<30} {planet['kind']:<9} "
              f"a={planet['semi_major_au']:.3f} P={planet['period_days']:.1f}d "
              f"M={planet['mass_earth']:.3f}Me T={planet['temperature_k']:.0f}K "
              f"{' '.join(notable)}")
    planets = system.get("planets", [])
    habitable = [
        p for p in planets
        if p["kind"] not in ("GasGiant", "IceGiant")
        and (p["atmosphere"] and 240 < p["temperature_k"] < 320 or p["biosphere"])
    ]
    hints = []
    if habitable:
        hints.append("HABITABLE:" + ",".join(p["name"][:22] for p in habitable))
    if len(neighbors) >= 4:
        hints.append("hub")
    print(f"gate links   : {len(neighbors)}   [{(' '.join(hints)) or 'no special hooks'}]")
    for other, kind in sorted(neighbors, key=lambda p: distance_ly(systems[p[0]])):
        print(f"  [{'-> new' if other not in seen else '   seen'}] {kind:8s} "
              f"#{other} {systems[other]['name']} ({distance_ly(systems[other]):.2f} ly)")
    print(f"queue depth  : {len(QUEUE.read_text().splitlines())}")
    print("next: write lore, then record the name with: "
          f"python3 {sys.argv[0]} name {index} '<NAME>'")


if __name__ == "__main__":
    if sys.argv[1:] and sys.argv[1] == "name":
        index, name = int(sys.argv[2]), sys.argv[3]
        used = {line.split("|")[1] for line in NAMES.read_text().splitlines() if line.strip()} if NAMES.exists() else set()
        if name in used:
            print(f"WARNING: name '{name}' already used")
        record_name(index, name, sys.argv[4] if len(sys.argv) > 4 else None)
        print(f"recorded #{index} as {name}")
    else:
        main()
