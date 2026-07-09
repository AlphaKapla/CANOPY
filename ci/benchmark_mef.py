#!/usr/bin/env python3
"""Benchmark the engine against SCRAM on a directory of MEF fault trees
(e.g. the Aralia suite bundled in SCRAM's input/Aralia).

For each XML file: import to YAML (ci/import_mef.py), quantify P(top) with
our engine (--prob-only), quantify with SCRAM (--bdd --probability), and
compare (rel tol 2e-5, bounded by SCRAM's 6-significant-digit report).
Both engines run under the same timeout and a 4 GiB address-space cap;
per-case failures (timeout/memory) are reported, not fatal — an honest
scalability profile is part of the result.

Usage: benchmark_mef.py <xml-dir> [--timeout 60] [--engine PATH]
"""
import argparse
import glob
import json
import os
import resource
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

REL_TOL = 2e-5
MEM_BYTES = int(os.environ.get("PSA_BENCH_MEM_GIB", 4)) << 30


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (MEM_BYTES, MEM_BYTES))


def run(cmd, timeout):
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, capture_output=True, text=True,
                           timeout=timeout, preexec_fn=limits)
        dt = time.monotonic() - t0
        if p.returncode != 0:
            return None, dt, "crash/oom"
        return p.stdout, dt, None
    except subprocess.TimeoutExpired:
        return None, timeout, "timeout"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("xml_dir")
    ap.add_argument("--timeout", type=int, default=60)
    ap.add_argument("--engine", default=os.environ.get(
        "CANOPY_BIN", "engine/target/release/canopy"))
    a = ap.parse_args()

    files = sorted(glob.glob(os.path.join(a.xml_dir, "*.xml")))
    print(f"{'case':<12} {'BEs':>5} {'gates':>6} | "
          f"{'ours P(top)':>12} {'t(s)':>6} {'nodes':>9} | "
          f"{'SCRAM P(top)':>12} {'t(s)':>6} | verdict")
    print("-" * 96)
    agree = disagree = incomplete = 0

    for f in files:
        name = os.path.splitext(os.path.basename(f))[0]
        d = tempfile.mkdtemp(prefix="psa-bench-")
        try:
            imp = subprocess.run(
                [sys.executable, "ci/import_mef.py", f, d],
                capture_output=True, text=True)
            if imp.returncode != 0:
                print(f"{name:<12} {'—':>5} {'—':>6} | import failed: "
                      f"{imp.stderr.strip().splitlines()[-1]}")
                incomplete += 1
                continue
            nbe = imp.stdout.split(" gates, ")[1].split(" basic")[0]
            ngt = imp.stdout.split("imported ")[1].split(": ")[1].split(
                " gates")[0]

            ours, to, eo = run([a.engine, d, "FT-MAIN", "--json",
                                "--prob-only"], a.timeout)
            rep = tempfile.mktemp(suffix=".xml")
            # -l 1: probability comes from the BDD and is unaffected;
            # this only truncates the report's product listing, which on
            # large trees otherwise reaches gigabytes.
            sout, ts, es = run(["scram", "--bdd", "--probability", "-l", "1",
                                f, "-o", rep], a.timeout)

            po = pn = None
            if ours:
                j = json.loads(ours)
                po, nodes = j["probability"], j["bdd_nodes"]
            ps = None
            if es is None and os.path.exists(rep):
                for sp in ET.parse(rep).getroot().iter("sum-of-products"):
                    ps = float(sp.get("probability"))
            if os.path.exists(rep):
                os.unlink(rep)

            oc = f"{po:.6e}" if po is not None else eo
            sc = f"{ps:.6e}" if ps is not None else (es or "no result")
            nn = f"{nodes}" if po is not None else "—"
            if po is not None and ps is not None:
                ok = abs(po - ps) <= max(1e-12,
                                         REL_TOL * max(abs(po), abs(ps)))
                verdict = "AGREE" if ok else "DISAGREE"
                agree += ok
                disagree += not ok
            else:
                verdict = "incomplete"
                incomplete += 1
            print(f"{name:<12} {nbe:>5} {ngt:>6} | {oc:>12} {to:>6.1f} "
                  f"{nn:>9} | {sc:>12} {ts:>6.1f} | {verdict}")
        finally:
            shutil.rmtree(d, ignore_errors=True)

    print("-" * 96)
    print(f"{agree} agree, {disagree} disagree, {incomplete} incomplete "
          f"(timeout {a.timeout}s, mem cap {MEM_BYTES >> 30} GiB per side)")
    return 1 if disagree else 0


if __name__ == "__main__":
    sys.exit(main())
