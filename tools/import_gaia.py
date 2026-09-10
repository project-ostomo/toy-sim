#!/usr/bin/env python3
"""Gaia CSV/CSV.gz -> portable flat .stars catalogue (no database or dependencies)."""
import argparse
import csv
import gzip
import json
import math
import pathlib
import struct

PARSEC_UM = 3.085677581491367e22
SOLAR_LUMENS = 3.6e28
HEADER = struct.Struct('<8sIIQIIQ')
MAGIC = b'TOYSTAR\0'
RECORD_BYTES = 76

def number(row, key):
    try:
        v = float(row.get(key, ''))
        return v if math.isfinite(v) else None
    except (ValueError, TypeError):
        return None

# Deliberately approximate display palette; no claim of calibrated spectral RGB.
PALETTE = [(30000, (.60,.72,1)), (15000, (.70,.80,1)), (8500, (.86,.90,1)),
           (6500, (1,.97,.92)), (5500, (1,.90,.76)), (4200, (1,.75,.51)), (3000, (1,.56,.30))]
def colour(temp, spectral, bp_rp):
    if temp is None and spectral and spectral[0].upper() in 'OBAFGKM':
        temp = PALETTE['OBAFGKM'.index(spectral[0].upper())][0]
    if temp is None and bp_rp is not None:
        # Coarse observed-colour fallback, not a temperature inference.
        anchors = [(-.4, 30000), (-.2, 15000), (0., 8500), (.4, 6500), (.8, 5500), (1.5, 4200), (3., 3000)]
        temp = anchors[-1][1]
        for (x0,t0),(x1,t1) in zip(anchors, anchors[1:]):
            if bp_rp <= x1:
                f = min(1., max(0., (bp_rp-x0)/(x1-x0)))
                temp = t0 + f*(t1-t0)
                break
    if temp is None:
        return (1., 1., 1.)
    rgb = PALETTE[-1][1]
    for (t0,c0),(t1,c1) in zip(PALETTE, PALETTE[1:]):
        if temp >= t1:
            f = min(1., max(0., (temp-t1)/(t0-t1)))
            rgb = tuple(b+(a-b)*f for a,b in zip(c0,c1))
            break
    rgb = tuple(v/12.92 if v <= .04045 else ((v+.055)/1.055)**2.4 for v in rgb)
    y = sum(v*w for v,w in zip(rgb, (.2126,.7152,.0722)))
    return tuple(v/y for v in rgb)

def record(row, origin, min_snr):
    source_id = int(row['source_id'])
    if not 0 < source_id < 2**64:
        raise ValueError('source_id must be a positive u64')
    if any(row.get(key, '').lower() in ('true', '1') for key in ('in_qso_candidates', 'in_galaxy_candidates')):
        return None
    ra, dec = number(row, 'ra'), number(row, 'dec')
    g = number(row, 'phot_g_mean_mag')
    distance = next((v for key in ('distance_pc', 'r_med_geo')
                     if (v := number(row, key)) is not None and v > 0), None)
    if distance is None:
        p, pe = number(row, 'parallax'), number(row, 'parallax_error')
        if p is not None and pe is not None and p > 0 and pe > 0 and p/pe >= min_snr:
            distance = 1000/p
    if ra is None or dec is None or g is None or distance is None or not (0 <= ra < 360 and -90 <= dec <= 90):
        return None
    r, d = math.radians(ra), math.radians(dec)
    xyz = tuple(o + round(distance*PARSEC_UM*v) for o,v in zip(origin,
                (math.cos(d)*math.cos(r), math.cos(d)*math.sin(r), math.sin(d))))
    try:
        absolute_mag = g - 5*math.log10(distance/10)
        luminosity = SOLAR_LUMENS*10**((4.83-absolute_mag)/2.5)
    except OverflowError:
        return None
    if not all(-2**126 <= v < 2**126 for v in xyz) or not math.isfinite(luminosity) or luminosity <= 0:
        return None
    rgb = colour(number(row,'teff_gspphot'), row.get('spectraltype_esphs'), number(row,'bp_rp'))
    return (struct.pack('<Q', source_id)
            + b''.join(v.to_bytes(16, 'little', signed=True) for v in xyz)
            + struct.pack('<d3f', luminosity, *rgb))

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=pathlib.Path)
    parser.add_argument('csv', nargs='+', type=pathlib.Path)
    parser.add_argument('--origin-um', nargs=3, type=int, default=[0,0,0])
    parser.add_argument('--min-parallax-snr', type=float, default=10.)
    parser.add_argument('--limit', type=int, help='Maximum input rows, for sample conversions')
    args = parser.parse_args()
    if not math.isfinite(args.min_parallax_snr) or args.min_parallax_snr <= 0 or (args.limit is not None and args.limit <= 0):
        parser.error('SNR and limit must be positive')
    if args.output.resolve() in [p.resolve() for p in args.csv]:
        parser.error('output must differ from input')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + '.part')
    seen = set()
    count = processed = rejected = duplicates = 0
    try:
        with temporary.open('wb') as output:
            output.write(HEADER.pack(MAGIC, 1, RECORD_BYTES, 0, 1, 1, 1))
            for path in args.csv:
                opener = gzip.open if path.suffix == '.gz' else open
                with opener(path, 'rt', newline='') as source:
                    rows = csv.DictReader(line for line in source if not line.startswith('#'))
                    if not rows.fieldnames or not {'source_id','ra','dec','phot_g_mean_mag'}.issubset(rows.fieldnames):
                        raise ValueError(f'missing Gaia CSV columns: {path}')
                    for line, row in enumerate(rows, 1):
                        if args.limit is not None and processed >= args.limit:
                            break
                        processed += 1
                        try:
                            ident = int(row['source_id'])
                            if ident in seen:
                                duplicates += 1
                                continue
                            seen.add(ident)
                            data = record(row, args.origin_um, args.min_parallax_snr)
                        except Exception as error:
                            raise ValueError(f'{path}: data row {line}: {error}') from error
                        if data is None:
                            rejected += 1
                        else:
                            output.write(data)
                            count += 1
                        if processed % 100000 == 0:
                            print(f'{processed} input rows, {count} stars', flush=True)
            output.seek(0)
            output.write(HEADER.pack(MAGIC, 1, RECORD_BYTES, count, 1, 1, 1))
        temporary.replace(args.output)
    finally:
        temporary.unlink(missing_ok=True)
    metadata = {
        'format_version': 1, 'namespace': 'Gaia DR3 (1)',
        'frame': 'ICRS Cartesian, fixed J2016.0', 'units': 'integer micrometres',
        'origin_um': args.origin_um, 'min_parallax_snr': args.min_parallax_snr,
        'calibration': 'G-as-visual; approximate temperature/class/BP-RP RGB; no extinction',
        'input_files': [str(p) for p in args.csv], 'input_rows': processed,
        'stars': count, 'rejected': rejected, 'duplicates': duplicates,
    }
    args.output.with_suffix(args.output.suffix + '.json').write_text(json.dumps(metadata, indent=2) + '\n')
    print(f'Wrote {count} stars ({args.output.stat().st_size} bytes) to {args.output}')

if __name__ == '__main__':
    main()
