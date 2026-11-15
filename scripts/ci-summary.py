#!/usr/bin/env python3
"""The link tests' results as Markdown, for a hosted lane's job summary
(spec/harness.md "Hosted CI").

    scripts/ci-summary.py <results.tsv> [<title>]

Reads the results file of `cargo xtask linktest` (spec/harness.md "Link
tests", "Results and exit status": outcome, test, architecture, linker,
detail, spec; tab-separated) and prints a table of the outcomes per linker
and architecture, then every record whose outcome fails a run (FAIL, XPASS,
KILLED, ERROR, ORACLE), at most 50 of them. A missing results file is said
so, with exit status 0: the gate stopped before its link-test step, and
its log says why.
"""

import sys
from collections import Counter

LINKERS = ["bfd", "lld", "mold", "mortar", "-"]
ARCHES = ["aarch64", "x86_64", "i386", "-"]
OUTCOMES = ["PASS", "XFAIL", "SKIP", "FAIL", "XPASS", "KILLED", "ERROR", "ORACLE"]
FAILING = {"FAIL", "XPASS", "KILLED", "ERROR", "ORACLE"}
LISTED = 50


def main(argv):
    if len(argv) not in (2, 3):
        print("usage: ci-summary.py <results.tsv> [<title>]", file=sys.stderr)
        return 2
    title = argv[2] if len(argv) == 3 else "Link tests"
    print(f"### {title}\n")
    try:
        with open(argv[1], encoding="utf-8", errors="replace") as f:
            records = [line.rstrip("\n").split("\t") for line in f if line.strip()]
    except FileNotFoundError:
        print(f"No results file (`{argv[1]}`): the gate stopped before its link-test step.")
        return 0
    records = [r + [""] * (6 - len(r)) for r in records]
    counts = Counter((r[3], r[2], r[0]) for r in records)
    arches = [a for a in ARCHES if any(r[2] == a for r in records)]
    linkers = [l for l in LINKERS if any(r[3] == l for r in records)]
    print("| linker | " + " | ".join(arches) + " |")
    print("|---|" + "---|" * len(arches))
    for linker in linkers:
        cells = []
        for arch in arches:
            cell = ", ".join(
                f"{counts[(linker, arch, o)]} {o}" for o in OUTCOMES if counts[(linker, arch, o)]
            )
            cells.append(cell or "")
        print(f"| {linker} | " + " | ".join(cells) + " |")
    total = Counter(r[0] for r in records)
    print("\n" + ", ".join(f"{total[o]} {o}" for o in OUTCOMES if total[o]) + f" ({len(records)} records)")
    failing = [r for r in records if r[0] in FAILING]
    if failing:
        print(f"\n{len(failing)} failing record(s):\n")
        print("```")
        for outcome, test, arch, linker, detail, _ in failing[:LISTED]:
            print(f"{outcome:<7} {test} [{linker}: {arch}]: {detail[:300]}")
        if len(failing) > LISTED:
            print(f"... and {len(failing) - LISTED} more (the results artifact has them all)")
        print("```")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
