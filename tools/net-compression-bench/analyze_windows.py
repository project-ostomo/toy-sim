#!/usr/bin/env python3
import csv
import statistics
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent
RESULTS = ROOT / 'results'
rows = list(csv.DictReader((RESULTS / 'windows-measurements.csv').open()))
groups = defaultdict(list)
for row in rows:
    groups[row['scenario'], row['codec']].append(row)
summary = []
for (scenario, codec), group in groups.items():
    data = {'scenario': scenario, 'codec': codec, 'trials': len(group)}
    for key in rows[0]:
        if key not in {'scenario', 'codec', 'repeat'}:
            data[key] = statistics.median(float(row[key]) for row in group)
    data['main_mbps'] = data['wire_bytes'] * 80 / 1e6
    data['total_mbps'] = (data['wire_bytes'] + data['asset_wire_bytes_per_tick']) * 80 / 1e6
    data['server_us'] = data['serialize_us'] + data['compress_cpu_us'] + data['asset_cpu_us_per_tick']
    data['cores_per_1000_clients'] = data['server_us'] / 100
    data['encoder_gib_per_1000_clients'] = data['encoder_bytes'] * 1000 / 2**30 if data['encoder_bytes'] else ''
    summary.append(data)
with (RESULTS / 'windows-summary.csv').open('w') as f:
    writer = csv.DictWriter(f, fieldnames=summary[0].keys())
    writer.writeheader()
    writer.writerows(summary)

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
plt.rcParams.update({'font.size': 10})
fig, axes = plt.subplots(2, 3, figsize=(13, 7.6))
for col, scenario in enumerate(['coast128', 'battle1024', 'battle8192']):
    scenario_rows = [r for r in summary if r['scenario'] == scenario]
    for level, color in [(-1, '#208fbc'), (1, '#e18825'), (3, '#8262b2')]:
        selected = [r for r in scenario_rows if r['codec'].startswith(f'zstd{level}-') and not r['codec'].endswith('ldm')]
        selected.sort(key=lambda r: int(r['codec'].split('-w')[1]))
        windows = [2**int(r['codec'].split('-w')[1])/1024 for r in selected]
        axes[0,col].plot(windows, [r['main_mbps'] for r in selected], 'o-', color=color, label=f'Zstd {level}')
        axes[1,col].plot(windows, [r['compress_cpu_us']/1000 for r in selected], 'o-', color=color)
    lz4 = next(r for r in scenario_rows if r['codec'] == 'lz4-linked')
    axes[0,col].axhline(lz4['main_mbps'], color='#555', linestyle='--', label='LZ4')
    axes[1,col].axhline(lz4['compress_cpu_us']/1000, color='#555', linestyle='--')
    axes[0,col].set_title(scenario)
    for row in range(2):
        axes[row,col].set_xscale('log', base=2)
        axes[row,col].grid(alpha=.2)
        axes[row,col].set_xticks([64,256,1024,4096,16384],['64 KiB','256 KiB','1 MiB','4 MiB','16 MiB'],rotation=20)
    axes[1,col].set_xlabel('Streaming history window')
axes[0,0].set_ylabel('Wire bandwidth at 10 Hz (Mbit/s)')
axes[1,0].set_ylabel('Compression CPU (ms/snapshot)')
axes[0,2].legend()
fig.suptitle('Complete postcard snapshots: larger Zstd windows have a workload-dependent payoff\nRyzen 9 5900XT, one core, median of 3–6 trials; synthetic data')
fig.tight_layout()
fig.savefig(RESULTS / 'windows.png', dpi=160)
fig.savefig(RESULTS / 'windows.svg')
