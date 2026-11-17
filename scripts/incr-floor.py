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
at each run's start is recorded. With --interleave (WP-094) the series run
round-robin, one run of each per round, so that their ratios are taken
under the same load; a series item may be `LABEL:KIND:BIN:MORTAR` to
compare binaries (each update chain has its own output and state, and the
edit is set before each update to the value its chain's state does not
hold).
After each update series the output is compared byte for byte with an
incremental-mode full link of the same (edited) inputs (its layout has the
slack a state's layout has, WP-062; a plain link's has none). Files the script writes in DIR are named
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
        for pre in ("mortar: floor: ", "mortar: incremental: "):
            if line.startswith(pre):
                head, _, rest = line[len(pre):].rpartition(": ")
                out[head] = float(rest.split(" ms", 1)[0])
                break
        else:
            pre = None
        if pre:
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


def parse_series(text, mortar, floor):
    """`LABEL[:KIND[:BIN[:MORTAR]]]` items, comma-separated: KIND defaults to
    LABEL; BIN is the linker (plain, incr-full) or the floor driver (copy,
    in-place), MORTAR the linker an update chain's first full link uses."""
    out = []
    for item in text.split(","):
        parts = item.split(":")
        label = parts[0]
        kind = parts[1] if len(parts) > 1 and parts[1] else label
        if kind not in ("plain", "incr-full", "copy", "in-place", "ld", "lld", "mold"):
            sys.exit(f"incr-floor: unknown series kind {kind}")
        update = kind in ("copy", "in-place")
        b = parts[2] if len(parts) > 2 and parts[2] else (floor if update else mortar)
        m = parts[3] if len(parts) > 3 and parts[3] else mortar
        out.append({"label": label, "kind": kind, "bin": b, "mortar": m})
    return out


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
    ap.add_argument("--extra", default="", help="words added to every mortar link line (space-separated)")
    ap.add_argument("--quiet", type=float, default=0.0,
                    help="before each run, wait until the 1-minute load is under this")
    ap.add_argument("--out-dir", default="",
                    help="WP-105: where the outputs and states go (default: DIR); the link still runs in DIR")
    ap.add_argument("--interleave", action="store_true",
                    help="round-robin: each round runs every series once (WP-094)")
    o = ap.parse_args()
    words = [w for w in open(o.args).read().split("\n") if w]
    extra = [w for w in o.extra.split(" ") if w]
    efile, eoff, eold, enew = o.edit.split(":")
    efile = os.path.join(o.dir, efile)
    eoff = int(eoff, 0)
    eold, enew = bytes.fromhex(eold), bytes.fromhex(enew)
    cur = {"edited": open(efile, "rb").read()[eoff:eoff + len(enew)] == enew}
    start_edited = cur["edited"]

    odir = o.out_dir or o.dir

    def line(out):
        out = os.path.join(odir, out)
        return [out if w == "OUTPUT" else w for w in words] + extra

    def set_input(edited):
        if cur["edited"] == edited:
            return
        if edited:
            edit(efile, eoff, eold, enew)
        else:
            edit(efile, eoff, enew, eold)
        cur["edited"] = edited

    series = parse_series(o.series, o.mortar, o.floor)
    results = {"dir": o.dir, "fs": subprocess.run(["stat", "-f", "-c", "%T", o.dir],
                                                  capture_output=True, text=True).stdout.strip(),
               "interleaved": o.interleave}
    runs = {s["label"]: [] for s in series}
    chain = {}  # label -> the input's value its state was written from
    loads = {s["label"]: uptime() for s in series}

    def one(s, i):
        label, kind = s["label"], s["kind"]
        if kind in ("plain", "incr-full"):
            more = ["--time-phases"] + (["--incremental=full"] if kind == "incr-full" else [])
            cmd = [s["bin"]] + line(f"floor.{label}") + more
        elif kind in ("copy", "in-place"):
            if label not in chain:
                w, p, err, st = timed([s["mortar"]] + line(f"floor.{label}") + ["--incremental=full"], o.dir)
                if st:
                    sys.exit(f"incr-floor: the full link failed:\n{err}")
                chain[label] = cur["edited"]
            set_input(not chain[label])
            chain[label] = cur["edited"]
            cmd = [s["bin"], kind, "--"] + line(f"floor.{label}") + ["--incremental"]
        else:
            tool = {"ld": "ld.bfd", "lld": "ld.lld", "mold": "mold"}[kind]
            out = f"floor.{label}.{i}"
            cmd = [tool] + (["--no-fork"] if kind == "mold" else []) + [w for w in line(out) if w not in extra]
        if o.quiet:
            while load1() >= o.quiet:
                time.sleep(10)
        before = load1()
        wall, peak, err, st = timed(cmd, o.dir)
        if st:
            sys.exit(f"incr-floor: {label} run {i} failed ({st}):\n{err}")
        if kind in ("ld", "lld", "mold"):
            os.remove(os.path.join(odir, f"floor.{label}.{i}"))
        if i == 0:
            return
        runs[label].append({"wall": wall, "peak_kib": peak, "phases": phases(err), "load": before,
                            "first": err.splitlines()[0] if err else ""})

    if o.interleave:
        for i in range(o.runs + 1):
            for s in series:
                one(s, i)
    else:
        for s in series:
            loads[s["label"]] = uptime()
            for i in range(o.runs + 1):
                one(s, i)
    for s in series:
        label, kind = s["label"], s["kind"]
        rs = runs[label]
        r = {"kind": kind, "bin": s["bin"], "uptime_before": loads[label], "uptime_after": uptime(),
             "wall": summary([x["wall"] for x in rs]),
             "peak_kib": summary([x["peak_kib"] for x in rs]),
             "load": summary([x["load"] for x in rs])}
        names = []
        for x in rs:
            for k in x["phases"]:
                if k not in names:
                    names.append(k)
        r["phases"] = {k: summary([x["phases"].get(k, 0.0) for x in rs]) for k in names}
        if kind in ("copy", "in-place"):
            r["first"] = rs[-1]["first"]
            set_input(chain[label])
            w, p, err, st = timed([s["mortar"]] + line("floor.check") + ["--incremental=full"], o.dir)
            if st:
                sys.exit(f"incr-floor: the reference link failed:\n{err}")
            same = open(os.path.join(odir, f"floor.{label}"), "rb").read() == \
                open(os.path.join(odir, "floor.check"), "rb").read()
            r["identical_to_full"] = same
            if not same:
                print(f"incr-floor: {label}: the update differs from the incremental-mode full link", flush=True)
        results[label] = r
        ph = " ".join(f"{k}={v['median']:.1f}" for k, v in r["phases"].items())
        print(f"{label}: wall median {r['wall']['median'] * 1000:.1f} ms "
              f"[{r['wall']['min'] * 1000:.1f}-{r['wall']['max'] * 1000:.1f}], "
              f"peak median {r['peak_kib']['median'] / 1024:.1f} MiB "
              f"[{r['peak_kib']['min'] / 1024:.1f}-{r['peak_kib']['max'] / 1024:.1f}] "
              f"n={o.runs} | load at each run {r['load']['min']:.1f}-{r['load']['max']:.1f} | {r['uptime_before']}", flush=True)
        if ph:
            print(f"  phases (median ms): {ph}", flush=True)
        if "identical_to_full" in r:
            print(f"  {r['first']}; identical to an incremental-mode full link: {r['identical_to_full']}", flush=True)
    if o.json:
        with open(o.json, "w") as f:
            json.dump(results, f, indent=1)
    set_input(start_edited)


if __name__ == "__main__":
    main()
