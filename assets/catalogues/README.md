# Gaia DR3 catalogue for views near Earth

The source download, `gaia-dr3-earth-million.csv`, contains 1,000,000 real ESA Gaia DR3 sources, ordered
by apparent G magnitude, with parallax signal-to-noise above 10. Downloaded
2026-09-07. All million rows have usable 3D positions; the flat runtime catalogue is 76 MB and spans G 1.94–10.80. The exact query is in `gaia-dr3-earth-million.adql`.

The repository includes the compact `.stars` runtime catalogue and its provenance
JSON. The raw CSV is ignored by Git and is only needed when rebuilding the
catalogue; the commands below download it again.

This is a brightness-selected catalogue, not an arbitrary TOP sample. The parallax
quality requirement excludes sources without reliable distances. Gaia itself is
incomplete at the very bright end, so this is not a complete inventory of all
bright stars visible from Earth. No additional Hipparcos/Tycho stars are merged yet.

Endpoint: https://gea.esac.esa.int/tap-server/tap/async

Refresh and rebuild:

```sh
python3 tools/download_gaia_earth.py --count 1000000
python3 tools/import_gaia.py assets/catalogues/gaia-dr3-earth-million.stars assets/catalogues/gaia-dr3-earth-million.csv
```

The downloader streams the response and verifies the exact requested row count
before replacing the CSV. The converter atomically replaces its output and writes a provenance JSON file. The default
scenario disables synthetic background stars, keeps its authored planetary system,
and uses this flat binary catalogue. **The default magnitude limit is 6**; the catalogue
size does not determine how many stars are rendered. The renderer cap is 150,000.

The catalogue is loaded once into RAM and indexed in luminosity buckets using
KD-trees. For reproducible query timings without Bevy or a window:

```sh
cargo run -p toy-sim-stars --release --example query -- assets/catalogues/gaia-dr3-earth-million.stars
```

Positions use fixed ICRS Cartesian axes at J2016.0, observer origin (0,0,0), and
high-SNR inverse parallaxes. Our fictional initial system is near that origin;
this is a near-Earth star view, not a reconstruction of the Solar System. G is
approximated as visual magnitude, BP-RP supplies approximate colours, and extinction
is omitted. The companion `.stars.json` records these choices.

This work has made use of data from the European Space Agency (ESA) mission Gaia
(https://www.cosmos.esa.int/gaia), processed by the Gaia Data Processing and Analysis
Consortium (DPAC, https://www.cosmos.esa.int/web/gaia/dpac/consortium). Funding for the
DPAC has been provided by national institutions, in particular the institutions
participating in the Gaia Multilateral Agreement.

References: Gaia Collaboration et al. (2016), A&A 595, A1;
Gaia Collaboration et al. (2023), A&A 674, A1.
