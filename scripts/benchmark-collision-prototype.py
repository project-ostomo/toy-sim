#!/usr/bin/env python3
"""Run the headless collision experiment serially; preserve every sample summary."""
import argparse
import csv
import itertools
import os
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--ticks", type=int, default=300)
    parser.add_argument("--warmup", type=int, default=70)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--binary", type=Path, help="use a previously built executable")
    parser.add_argument("--allocations", action="store_true")
    parser.add_argument("--workloads", nargs="+", choices=["1", "16", "scripted"], default=["1", "16", "scripted"])
    parser.add_argument("--steps", nargs="+", type=int, choices=[100, 10, 5], default=[100, 10, 5])
    parser.add_argument("--precisions", nargs="+", choices=["f64", "f32"], default=["f64", "f32"])
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    os.chdir(root)
    feature = "collision-prototype-allocations" if args.allocations else "collision-prototype"
    if not args.skip_build and args.binary is None:
        subprocess.run([
            "cargo", "build", "--release", "-p", "osg-server", "--features", feature,
            "--example", "collision_prototype", "--offline",
        ], check=True)
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve() if args.binary else root / "target/release/examples/collision_prototype"
    common = [str(binary), "--ticks", str(args.ticks), "--warmup", str(args.warmup)]
    jobs = []
    if not args.allocations:
        for repeat, players in itertools.product(range(args.repeats), [int(w) for w in args.workloads if w != "scripted"]):
            jobs.append((f"baseline-{players}-{repeat}", ["--baseline", "--ships", str(players)]))
    for repeat, workload, precision, cached, step in itertools.product(
        range(args.repeats), args.workloads, args.precisions, [True, False], args.steps
    ):
        options = ["--scripted"] if workload == "scripted" else ["--ships", workload]
        options += ["--step-ms", str(step)]
        if precision == "f32":
            options += ["--f32"]
        if not cached:
            options += ["--fresh"]
        jobs.append((f"{workload}-{precision}-{cached}-{step}-{repeat}", options))
    for index, (name, options) in enumerate(jobs):
        destination = args.output / f"{name}.csv"
        if destination.exists():
            continue
        started = time.monotonic()
        result = subprocess.run(common + options, text=True, capture_output=True)
        (args.output / f"{name}.log").write_text(result.stderr)
        result.check_returncode()
        lines = result.stdout.splitlines()
        header = next(i for i, line in enumerate(lines) if line.startswith("workload,"))
        rows = list(csv.reader(lines[header:header + 2]))
        assert len(rows) == 2 and len(rows[0]) == len(rows[1]), result.stdout
        destination.write_text("\n".join(lines[header:header + 2]) + "\n")
        print(f"{index + 1}/{len(jobs)} {name} {time.monotonic() - started:.1f}s", flush=True)


if __name__ == "__main__":
    main()
