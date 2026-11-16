#!/usr/bin/env python3
"""The measured floor of an incremental update (WP-061 stage B, spec/incr.md
section 1.5 "Go/no-go rule"; the record is spec/landed/incr/WP-061b.md).

    incr-floor.py --dir DIR --args FILE --edit FILE:OFFSET:OLD:NEW \
        --mortar BIN --floor BIN [--runs 7] [--series a,b,...] [--json OUT]

DIR is the link's working directory; FILE holds the link line one word per
line, the output named `OUTPUT`. The edit is a same-size patch of the object
FILE at OFFSET (OLD and NEW in hex), written through a temporary file and a
rename as a compiler writes its output; successive update runs toggle it
(OLD -> NEW, NEW -> OLD), so each update starts from the state the previous
one wrote. BIN `--floor` is the `incr_floor` example (`mortar::floor`).

Series (each one warm-up run, then RUNS measured runs; wall time by the
clock around the process, peak RSS by `/usr/bin/time -f %M`, `uptime` before
the series):
  plain       mortar, a plain full link, `--time-phases`
  incr-full   mortar `--incremental=full`, `--time-phases`
  copy        the update by copy (IN-47), phase lines from `incr_floor`
  in-place    the update in place (IN-46)
  ld lld mold GNU ld, ld.lld, mold: full links, each to a fresh path
With --quiet L each run waits until the 1-minute load is under L; the load
at each run's start is recorded.
After each update series the output is compared byte for byte with a plain
link of the same (edited) inputs. Files the script writes in DIR are named
`floor.*`; it removes the ones it creates for the other linkers.
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import time


def uptime():
    return subprocess.run(["uptime"], capture_output=True, text=True).stdout.strip()


def load1():
    return float(open("/proc/loadavg").read().split()[0])


def timed(cmd, cwd):
    """Runs cmd under /usr/bin/time: (wall s, peak KiB, stderr, status)."""
    t0 = time.perf_counter()
    p = subprocess.run(["/usr/bin/time", "-f", "FLOORTIME %M"] + cmd, cwd=cwd,
                       capture_output=True, text=True)
    wall = time.perf_counter() - t0
    peak = 0
    lines = []
    for line in p.stderr.splitlines():
        if line.startswith("FLOORTIME "):
            peak = int(line.split()[1])
        else:
            lines.append(line)
    return wall, peak, "\n".join(lines), p.returncode


def phases(text):
    """`... time-phases: NN name: wall X ms, ...` (mortar) or `NN name: wall
    X ms, ...` (incr_floor): {name: ms}."""
    out = {}
    for line in text.splitlines():
        if line.startswith("mortar: floor: "):
            head, _, rest = line[len("mortar: floor: "):].rpartition(": ")
            out[head] = float(rest.split(" ms", 1)[0])
            continue
        if "time-phases: " in line:
            line = line.split("time-phases: ", 1)[1]
        head, sep, rest = line.partition(": wall ")
        if not sep:
            continue
        try:
            out[head.strip()] = float(rest.split(" ms", 1)[0])
        except ValueError:
            pass
    return out


def edit(path, off, old, new):
    d = bytearray(open(path, "rb").read())
    if d[off:off + len(old)] != old:
        sys.exit(f"incr-floor: {path}+{off:#x} holds {d[off:off + len(old)].hex()}, not {old.hex()}")
    d[off:off + len(new)] = new
    tmp = path + ".floor-tmp"
    with open(tmp, "wb") as f:
        f.write(d)
    os.chmod(tmp, os.stat(path).st_mode)
    os.rename(tmp, path)


def summary(xs):
    return {"median": statistics.median(xs), "min": min(xs), "max": max(xs), "n": len(xs)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True)
    ap.add_argument("--args", required=True)
    ap.add_argument("--edit", required=True)
    ap.add_argument("--mortar", required=True)
    ap.add_argument("--floor", required=True)
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--series", default="plain,incr-full,copy,in-place,ld,lld,mold")
    ap.add_argument("--json")
    ap.add_argument("--quiet", type=float, default=0.0,
                    help="before each run, wait until the 1-minute load is under this")
    o = ap.parse_args()
    words = [w for w in open(o.args).read().split("\n") if w]
    efile, eoff, eold, enew = o.edit.split(":")
    efile = os.path.join(o.dir, efile)
    eoff = int(eoff, 0)
    eold, enew = bytes.fromhex(eold), bytes.fromhex(enew)
    state = {"edited": open(efile, "rb").read()[eoff:eoff + len(enew)] == enew}

    def line(out):
        return [out if w == "OUTPUT" else w for w in words]

    def toggle():
        if state["edited"]:
            edit(efile, eoff, enew, eold)
        else:
            edit(efile, eoff, eold, enew)
        state["edited"] = not state["edited"]

    results = {"dir": o.dir, "fs": subprocess.run(["stat", "-f", "-c", "%T", o.dir],
                                                  capture_output=True, text=True).stdout.strip()}
    for name in o.series.split(","):
        runs = []
        load = uptime()
        for i in range(o.runs + 1):
            if name in ("plain", "incr-full"):
                extra = ["--time-phases"] + (["--incremental=full"] if name == "incr-full" else [])
                out = "floor.plain" if name == "plain" else "floor.incr-full"
                cmd = [o.mortar] + line(out) + extra
            elif name in ("copy", "in-place"):
                if i == 0:
                    # The state the series starts from: a full link of the
                    # current inputs (the warm-up run is the first update).
                    w, p, err, st = timed([o.mortar] + line("floor.incr") + ["--incremental=full"], o.dir)
                    if st:
                        sys.exit(f"incr-floor: the full link failed:\n{err}")
                toggle()
                cmd = [o.floor, name, "--"] + line("floor.incr") + ["--incremental"]
            else:
                tool = {"ld": "ld.bfd", "lld": "ld.lld", "mold": "mold"}[name]
                out = f"floor.{name}.{i}"
                cmd = [tool] + (["--no-fork"] if name == "mold" else []) + line(out)
            if o.quiet:
                while load1() >= o.quiet:
                    time.sleep(10)
            before = load1()
            wall, peak, err, st = timed(cmd, o.dir)
            if st:
                sys.exit(f"incr-floor: {name} run {i} failed ({st}):\n{err}")
            if name in ("ld", "lld", "mold"):
                os.remove(os.path.join(o.dir, out))
            if i == 0:
                continue
            runs.append({"wall": wall, "peak_kib": peak, "phases": phases(err), "load": before,
                         "first": err.splitlines()[0] if err else ""})
        r = {"uptime_before": load, "uptime_after": uptime(),
             "wall": summary([x["wall"] for x in runs]),
             "peak_kib": summary([x["peak_kib"] for x in runs]),
             "load": summary([x["load"] for x in runs])}
        names = []
        for x in runs:
            for k in x["phases"]:
                if k not in names:
                    names.append(k)
        r["phases"] = {k: summary([x["phases"].get(k, 0.0) for x in runs]) for k in names}
        if name in ("copy", "in-place"):
            r["first"] = runs[-1]["first"]
            w, p, err, st = timed([o.mortar] + line("floor.check"), o.dir)
            same = open(os.path.join(o.dir, "floor.incr"), "rb").read() == \
                open(os.path.join(o.dir, "floor.check"), "rb").read()
            r["identical_to_plain"] = same
            if not same:
                sys.exit(f"incr-floor: {name}: the update differs from the plain link")
        results[name] = r
        ph = " ".join(f"{k}={v['median']:.1f}" for k, v in r["phases"].items())
        print(f"{name}: wall median {r['wall']['median'] * 1000:.1f} ms "
              f"[{r['wall']['min'] * 1000:.1f}-{r['wall']['max'] * 1000:.1f}], "
              f"peak median {r['peak_kib']['median'] / 1024:.1f} MiB "
              f"[{r['peak_kib']['min'] / 1024:.1f}-{r['peak_kib']['max'] / 1024:.1f}] "
              f"n={o.runs} | load at each run {r['load']['min']:.1f}-{r['load']['max']:.1f} | {load}", flush=True)
        if ph:
            print(f"  phases (median ms): {ph}", flush=True)
        if "identical_to_plain" in r:
            print(f"  {r['first']}; identical to a plain link: {r['identical_to_plain']}", flush=True)
    if o.json:
        with open(o.json, "w") as f:
            json.dump(results, f, indent=1)
    if state["edited"]:
        toggle()


if __name__ == "__main__":
    main()
