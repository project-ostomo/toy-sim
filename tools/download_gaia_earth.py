"""Download the brightest Gaia DR3 sources with parallax SNR > 10."""
import argparse
import csv
import gzip
import pathlib
import shutil
import time
import urllib.parse
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--count', type=int, default=1_000_000)
parser.add_argument('--output', type=pathlib.Path, default=pathlib.Path('crates/osg-stars/data/gaia-dr3-earth-million.csv'))
parser.add_argument('--job', help='Resume retrieval of an existing ESA asynchronous job URL')
args = parser.parse_args()
if args.count <= 0:
    parser.error('count must be positive')
base = 'https://gea.esac.esa.int/tap-server/tap'
q = f'SELECT TOP {args.count} source_id,ra,dec,parallax,parallax_error,phot_g_mean_mag,bp_rp FROM gaiadr3.gaia_source WHERE phot_g_mean_mag < 12 AND parallax_over_error > 10 ORDER BY phot_g_mean_mag ASC, source_id ASC'
args.output.parent.mkdir(parents=True, exist_ok=True)
if args.job:
    job = args.job
else:
    body = urllib.parse.urlencode({'REQUEST':'doQuery','LANG':'ADQL','FORMAT':'csv','QUERY':q,'MAXREC':str(args.count),'PHASE':'RUN'}).encode()
    with urllib.request.urlopen(urllib.request.Request(base+'/async', data=body), timeout=60) as r:
        job = r.url
print('Job', job, flush=True)
args.output.with_suffix('.job').write_text(job)
for _ in range(360):
    with urllib.request.urlopen(job+'/phase', timeout=30) as r:
        phase = r.read().decode().strip()
    print('Phase', phase, flush=True)
    if phase == 'COMPLETED':
        break
    if phase in ('ERROR','ABORTED'):
        raise RuntimeError(urllib.request.urlopen(job+'/error').read().decode())
    time.sleep(5)
else:
    raise RuntimeError('Query timed out; use --job with the saved URL to resume')
part = pathlib.Path(str(args.output)+'.part')
with urllib.request.urlopen(job+'/results/result', timeout=180) as response, part.open('wb') as f:
    shutil.copyfileobj(response, f)
with part.open('rb') as f:
    compressed = f.read(2) == b'\x1f\x8b'
if compressed:
    unpacked = pathlib.Path(str(part)+'.csv')
    with gzip.open(part,'rb') as source, unpacked.open('wb') as destination:
        shutil.copyfileobj(source,destination)
    unpacked.replace(part)
with part.open(newline='') as f:
    rows = csv.DictReader(f)
    if not {'source_id','ra','dec','phot_g_mean_mag'}.issubset(rows.fieldnames or []):
        raise RuntimeError('Unexpected result columns')
    count = sum(1 for _ in rows)
if count != args.count:
    raise RuntimeError(f'Expected {args.count} rows, got {count}; refusing truncated download')
part.replace(args.output)
args.output.with_suffix('.adql').write_text(q+'\n')
print('Downloaded',count,'rows,',args.output.stat().st_size,'bytes',flush=True)
