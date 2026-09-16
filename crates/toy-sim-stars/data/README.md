# Star catalogue data

This directory holds the star catalogue that `toy-sim-stars` embeds with `include_bytes!`, plus the files that describe how it was produced. The format and tools are documented in [docs/gaia-catalogue.md](../../../docs/gaia-catalogue.md).

## Files

| File | Git-ignored | Description |
| --- | --- | --- |
| `gaia-dr3-earth-million.stars` | no | Binary `TOYSTAR` catalogue with 1,000,000 records. `StarCatalogue::embedded()` decodes it. |
| `gaia-dr3-earth-million.stars.json` | no | Metadata written by the importer: format version, namespace, frame, units, origin, parallax SNR threshold, calibration note, input files and row counts. |
| `gaia-dr3-earth-million.adql` | no | The ADQL query used for the download. |
| `gaia-dr3-earth-million.csv` | yes | Downloaded source rows. `.gitignore` excludes `*.csv` and `*.csv.gz` in this directory. The build does not need this file. |

The recorded metadata for the embedded catalogue:

- frame: ICRS Cartesian, fixed J2016.0
- units: integer micrometres, origin `[0, 0, 0]`
- minimum parallax SNR: 10
- calibration: G magnitude used as visual magnitude, approximate RGB from temperature, spectral class or BP−RP, no extinction correction
- input rows 1,000,000, stars 1,000,000, rejected 0, duplicates 0

The query selects the 1,000,000 brightest Gaia DR3 sources with `phot_g_mean_mag < 12` and `parallax_over_error > 10`, ordered by G magnitude and then source ID:

```sql
SELECT TOP 1000000 source_id,ra,dec,parallax,parallax_error,phot_g_mean_mag,bp_rp
FROM gaiadr3.gaia_source
WHERE phot_g_mean_mag < 12 AND parallax_over_error > 10
ORDER BY phot_g_mean_mag ASC, source_id ASC
```

## Regenerating

Run both commands from the repository root:

```sh
python3 tools/download_gaia_earth.py
python3 tools/import_gaia.py crates/toy-sim-stars/data/gaia-dr3-earth-million.stars \
    crates/toy-sim-stars/data/gaia-dr3-earth-million.csv
```

The download script submits an asynchronous job to the ESA Gaia TAP service at `https://gea.esac.esa.int/tap-server/tap`. It saves the job URL next to the output with a `.job` suffix, rewrites the `.adql` file, and refuses a result with the wrong row count. To resume a job, pass `--job <url>`.

Rebuild after replacing the `.stars` file, because it is embedded in every binary that depends on `toy-sim-stars`. The test `embedded_million_star_catalogue_is_queryable_without_a_runtime_file` expects exactly 1,000,000 stars and between 6,000 and 7,000 stars brighter than magnitude 6 from the origin. Update that test if you embed a different selection.
