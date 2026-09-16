#!/usr/bin/env python3
"""Summarize repeated compression measurements and draw bandwidth comparisons."""
import csv
from collections import defaultdict
from pathlib import Path
from statistics import median

root = Path(__file__).resolve().parent
results = root / 'results'
groups = defaultdict(list)
with (results / 'measurements.csv').open() as source:
    for row in csv.DictReader(source):
        groups[(row['scenario'], row['format'], row['codec'])].append(row)

fields = ['first_wire_bytes', 'raw_bytes', 'wire_bytes', 'serialize_us',
          'compress_us', 'decompress_us', 'parse_us', 'server_us',
          'server_p95_us', 'client_us', 'asset_wire_bytes']
summary = []
for (scenario, form, codec), rows in groups.items():
    row = {'scenario': scenario, 'format': form, 'codec': codec, 'runs': len(rows)}
    row.update({key: round(median(float(r[key]) for r in rows), 2) for key in fields})
    row['main_mbps_at_10hz'] = round(row['wire_bytes'] * 10 * 8 / 1e6, 4)
    summary.append(row)

with (results / 'summary.csv').open('w') as output:
    writer = csv.DictWriter(output, fieldnames=list(summary[0]))
    writer.writeheader()
    writer.writerows(summary)

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

methods = [('snapshot', 'lz4-linked'), ('snapshot', 'zstd1-w23'),
           ('snapshot', 'zstd3-w23'), ('compact', 'zstd3-w23'),
           ('delta', 'raw'), ('delta', 'zstd1-w23')]
labels = ['CBOR maps + LZ4', 'CBOR maps + Zstd 1', 'CBOR maps + Zstd 3',
          'CBOR arrays + Zstd 3', 'Delta, uncompressed', 'Delta + Zstd 1']
colors = ['#8aa3b7', '#4477aa', '#225588', '#228866', '#ddaa33', '#bb6633']
fig, axes = plt.subplots(1, 3, figsize=(13, 4.4), layout='constrained')
for ax, scenario, title in zip(axes, ['coast1024', 'battle1024', 'battle8192'],
                             ['1,024 coasting objects', '1,024 battle objects', '8,192 battle objects']):
    values = [next(r['main_mbps_at_10hz'] for r in summary
                   if (r['scenario'], r['format'], r['codec']) == (scenario, *method))
              for method in methods]
    ax.barh(labels, values, color=colors)
    ax.invert_yaxis()
    ax.set_title(title)
    ax.set_xlabel('Main stream payload (Mbit/s at 10 Hz)')
    ax.grid(axis='x', alpha=.2)
    ax.set_axisbelow(True)
    ax.set_xlim(0, max(values)*1.2)
    for i, value in enumerate(values):
        ax.text(value + max(values)*.02, i, f'{value:.2f}', va='center', fontsize=9)
fig.suptitle('Synthetic lossless state streaming — median of three runs')
fig.savefig(results / 'bandwidth.svg')
fig.savefig(results / 'bandwidth.png', dpi=150)
