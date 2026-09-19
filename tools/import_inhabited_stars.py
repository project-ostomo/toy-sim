#!/usr/bin/env python3
import argparse
import gzip
import hashlib
import itertools
import json
import math
from pathlib import Path

RADIUS_LY = 125.0
GROUP_RADIUS_LY = 0.03
CELL_LY = 0.05
SYSTEM_COUNT = 2990
LY_IN_AU = 63241.077
ICRS_TO_GALACTIC = (
    (-0.0548755604, -0.8734370902, -0.4838350155),
    (0.4941094279, -0.4448296300, 0.7469822445),
    (-0.8676661490, -0.1980763734, 0.4559837762),
)
OUTPUT = Path(__file__).resolve().parents[1] / "crates/toy-sim-universe/data/inhabited-stars.json"


def group_components(rows):
    cells = {}
    groups = []
    for star in sorted(rows, key=lambda row: (-row["luminosity"], row["id"])):
        position = tuple(star[axis] for axis in "xyz")
        cell = tuple(math.floor(value / CELL_LY) for value in position)
        neighbours = itertools.product(*(range(value - 1, value + 2) for value in cell))
        close = []
        for neighbour in neighbours:
            for index in cells.get(neighbour, []):
                primary = groups[index][0]
                distance_squared = sum((star[axis] - primary[axis]) ** 2 for axis in "xyz")
                if distance_squared < GROUP_RADIUS_LY ** 2:
                    close.append(index)
        if close:
            groups[min(close)].append(star)
        else:
            index = len(groups)
            groups.append([star])
            cells.setdefault(cell, []).append(index)
    return groups


def settlement_priority(group):
    star = group[0]
    digest = hashlib.sha256(("toy-sim settlement v1:" + star["id"]).encode()).digest()
    uniform = int.from_bytes(digest[:8], "little") / 2 ** 64
    weight = 1 + 3 * min(star["luminosity"], 1) + 2 * bool(star.get("name"))
    return -math.log(max(uniform, 1e-18)) / weight


def export_group(group):
    primary = group[0]
    companions = []
    for companion in group[1:4]:
        separation = math.sqrt(sum((primary[axis] - companion[axis]) ** 2 for axis in "xyz"))
        companions.append({
            "id": companion["id"],
            "luminosity_solar": companion["luminosity"],
            "temperature_k": companion["temperature"],
            "separation_au": separation * LY_IN_AU,
        })
    return {
        "id": primary["id"],
        "name": primary.get("name") or primary.get("designation") or primary["id"],
        "position_ly": [
            round(sum(ICRS_TO_GALACTIC[row][column] * primary[axis]
                      for row, axis in enumerate("xyz")), 12)
            for column in range(3)
        ],
        "luminosity_solar": primary["luminosity"],
        "temperature_k": primary["temperature"],
        "companions": companions,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("catalogue", type=Path)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    with gzip.open(args.catalogue, "rt") as handle:
        catalogue = json.load(handle)
    nearby = [star for star in catalogue["stars"]
              if star["id"] != "sol" and sum(star[axis] ** 2 for axis in "xyz") <= RADIUS_LY ** 2]
    groups = group_components(nearby)
    prominent = [group for group in groups if group[0].get("name")
                 or sum(group[0][axis] ** 2 for axis in "xyz") < 15.0 ** 2]
    reserved = {group[0]["id"] for group in prominent}
    remaining = [group for group in groups if group[0]["id"] not in reserved]
    chosen = prominent + sorted(remaining, key=settlement_priority)[:SYSTEM_COUNT - len(prominent)]
    chosen.sort(key=lambda group: group[0]["id"])
    systems = [export_group(group) for group in chosen]
    assert len(systems) == SYSTEM_COUNT
    assert len({system["id"] for system in systems}) == SYSTEM_COUNT
    assert all(len(group) <= 4 for group in chosen), "unhandled stellar components"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w") as handle:
        json.dump(systems, handle, separators=(",", ":"))
        handle.write("\n")
    metadata = {
        "import_version": 1,
        "source_version": catalogue["version"],
        "source_composite_sha256": catalogue["checksum"],
        "source_file_sha256": hashlib.sha256(args.catalogue.read_bytes()).hexdigest(),
        "output_sha256": hashlib.sha256(args.output.read_bytes()).hexdigest(),
        "source_coordinate_frame": "Galactic Cartesian, Sol origin",
        "coordinate_frame": "ICRS Cartesian, Sol origin, inherited J2016 astrometry",
        "radius_ly": RADIUS_LY,
        "group_radius_ly": GROUP_RADIUS_LY,
        "source_nearby_stars": len(nearby),
        "selected_systems": len(systems),
        "selected_companions": sum(len(system["companions"]) for system in systems),
    }
    args.output.with_suffix(".metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Imported {len(systems)} systems from {len(nearby)} nearby catalogue stars")
    print(catalogue["source"])


if __name__ == "__main__":
    main()
