#!/usr/bin/env python3
import csv
import io
import json
import re
import subprocess
import urllib.parse
from pathlib import Path

DATA = Path(__file__).resolve().parents[1] / "crates/osg-universe/data"
ENDPOINT = "https://simbad.cds.unistra.fr/simbad/sim-tap/sync"
GREEK = dict(zip(
    "alf bet gam del eps zet eta tet iot kap lam mu nu ksi omi pi rho sig tau ups phi chi psi ome".split(),
    "αβγδεζηθικλμνξοπρστυφχψω",
))


def preferred_name(identifiers):
    candidates = []
    for identifier in identifiers.split("|"):
        value = " ".join(identifier.split())
        if value.startswith("NAME "):
            candidates.append((0, value[5:]))
        elif value.startswith("* "):
            name = value[2:]
            match = re.match(r"([a-z]+)(\d*) (.+)$", name)
            if match and match[1] in GREEK:
                name = GREEK[match[1]] + match[2] + " " + match[3]
                candidates.append((1, name))
            else:
                candidates.append((2, name))
        else:
            for rank, prefix in enumerate(("HIP ", "HD ", "GJ ", "Gl "), 3):
                if value.startswith(prefix):
                    candidates.append((rank, value))
    return min(candidates)[1] if candidates else None


def main():
    systems = json.loads((DATA / "inhabited-stars.json").read_text())
    sources = [star["id"] for star in systems if star["id"].startswith("gaia-edr3-")]
    names = {}
    for start in range(0, len(sources), 150):
        identifiers = {
            f"Gaia {release} {source.removeprefix('gaia-edr3-')}": source
            for source in sources[start:start + 150]
            for release in ("EDR3", "DR3")
        }
        values = ",".join(f"'{identifier}'" for identifier in identifiers)
        query = (
            "SELECT ident.id, ids.ids FROM ident JOIN ids ON ident.oidref=ids.oidref "
            f"WHERE ident.id IN ({values})"
        )
        parameters = urllib.parse.urlencode({
            "REQUEST": "doQuery", "LANG": "ADQL", "FORMAT": "csv",
            "MAXREC": "10000", "QUERY": query,
        })
        response = subprocess.run(
            ["curl", "--silent", "--show-error", "--fail", "--max-time", "90",
             "--data-binary", "@-", ENDPOINT], input=parameters,
            check=True, capture_output=True, text=True,
        )
        rows = csv.DictReader(io.StringIO(response.stdout))
        for row in rows:
            name = preferred_name(row["ids"])
            if name:
                names[identifiers[row["id"]]] = name
        print(f"{min(start + 150, len(sources))}/{len(sources)}: {len(names)} names", flush=True)
    (DATA / "star-names.json").write_text(json.dumps({
        "source": ENDPOINT,
        "names": names,
    }, ensure_ascii=False, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
