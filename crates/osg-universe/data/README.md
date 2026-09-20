# Inhabited stellar anchors

`inhabited-stars.json` contains 2,990 selected stellar systems within 125 light-years
of Sol. The ten authored systems bring the inhabited map to 3,000 systems. This
selection describes settled space; it does not limit the background star catalogue
or assert that the remaining nearby stars do not exist.

The input is the `gaia-edr3-gcns-250ly-v2` catalogue from the worldbuilding history
simulation. That catalogue contains 139,021 Gaia EDR3 Catalogue of Nearby Stars
sources within a 250-light-year radius, plus Sol. The importer selects from the
18,031 non-Sol sources inside a 125-light-year radius. This distinction between
radius and diameter matters: the inhabited map is about 250 light-years across.

Run the importer with the original worldbuilding catalogue to reproduce both the
data and its checksum metadata:

```sh
python3 tools/import_inhabited_stars.py \
  ~/ASSISTANT/worldbuilding/openspacegame-history-sim/static/catalog/sector-v1.json.gz
```

The importer groups nearby components within 0.03 light-years of a selected
primary, preserves named and very nearby systems, and samples the remaining
settlement anchors with a SHA-256-derived weighted key. The key favors more
luminous sources without removing dim stars. Records are sorted by catalogue
identity, so source input order does not change the output. It retains 48 candidate
companions in 46 selected systems. The grouping is a simulation assumption about
possible multiple systems; it does not establish that the components are bound or
provide an observed orbital solution.

The source catalogue uses Galactic Cartesian coordinates. The game background
catalogue uses ICRS Cartesian coordinates. The importer rotates positions with the
transpose of the same ICRS-to-Galactic rotation used by the source builder. Sol
remains the origin. Coordinate rounding and the astrometric uncertainties of the
source remain; generated planetary orbits do not add observational precision.

`inhabited-stars.metadata.json` records the source version, composite source
checksum, compressed input checksum, output checksum, selection counts, and
coordinate frames. The data file preserves release-qualified Gaia identities.
Names are display labels, and occasional duplicate labels are disambiguated with
the catalogue identity when the map is built.

## Underlying sources

The worldbuilding source catalogue records these inputs:

- [Gaia EDR3 Catalogue of Nearby Stars, table 1c](https://cdsarc.cds.unistra.fr/ftp/J/A+A/649/A6/table1c.dat.gz),
  SHA-256 `299f7c15025780df96d5f73fc299e89c81b76fe3de2231981d92ffde11f20ab1`.
- [Gaia EDR3 Hipparcos best-neighbour crossmatch](https://cdn.gea.esac.esa.int/Gaia/gedr3/cross_match/hipparcos2_best_neighbour/Hipparcos2BestNeighbour.csv.gz),
  SHA-256 `23a74b073ad78c6294d8248fc04be95be2cf7ac646162493eb7322c698eb3a9c`.
- [CDS V/53A Bright Star Catalogue](https://cdsarc.cds.unistra.fr/ftp/cats/V/53A/catalog.dat.gz),
  SHA-256 `6b1e6469e3c98bbc70377297812b12000f1c047cbf572bbda8e6a265700c2c91`.
- [IAU Working Group on Star Names catalogue](https://exopla.net/star-names/modern-iau-star-names/),
  source snapshot SHA-256 `1bb8e33837efe24e153bddd202fb9855b86318d7b61fcbc3a064b3b82ae52795`.

The composite source checksum is
`e95815932b057af919d89ede7b682ba45ea4b0fbff87383a9eff748aa9e262e1`.
Luminosities and temperatures are approximate photometric transformations in that
source, rather than independently fitted stellar parameters. Some familiar labels
refer to a measured component of a wider named system. The source's habitability,
resource, and planet-count priors are excluded from this import. The game's
planetary generation supplies its own explicit simulation parameters.
