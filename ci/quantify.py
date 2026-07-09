#!/usr/bin/env python3
"""Quantify every event tree in a model; write merged JSON results.

Usage: quantify.py <model-dir> <out.json> [--engine PATH]
"""
import glob
import json
import os
import subprocess
import sys

import yaml


def main() -> int:
    model_dir, out_path = sys.argv[1], sys.argv[2]
    engine = os.environ.get("CANOPY_BIN", "engine/target/release/canopy")
    if "--engine" in sys.argv:
        engine = sys.argv[sys.argv.index("--engine") + 1]

    et_ids = []
    for p in sorted(glob.glob(os.path.join(model_dir, "event-trees/*.yaml"))):
        et = yaml.safe_load(open(p)).get("event_tree", {})
        if "id" in et:
            et_ids.append(et["id"])

    results = {}
    for et_id in et_ids:
        proc = subprocess.run(
            [engine, model_dir, et_id, "--json"],
            capture_output=True, text=True,
        )
        if proc.returncode != 0:
            print(f"ERROR quantifying {et_id}:\n{proc.stderr}", file=sys.stderr)
            return 1
        results[et_id] = json.loads(proc.stdout)

    with open(out_path, "w") as f:
        json.dump(results, f, indent=2, sort_keys=True)
    print(f"quantified {len(results)} event tree(s) -> {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
