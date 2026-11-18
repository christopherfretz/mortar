#!/usr/bin/env python3
"""WP-244: the link line a GCC driver runs, as a mortar response file.

    respfile.py OUT.rsp CXX ARGS...

Runs `CXX -### ARGS` and writes the collect2 command's words (all but the
first), one per line, to OUT.rsp: what `ld` is given. The caller lists
`<directory>\\t<OUT.rsp>` in MORTAR_WP244_SERIES for the in-process
facts and demand tests (crates/mortar/tests/state5_capture.rs).
"""
import shlex
import subprocess
import sys

out, cxx, args = sys.argv[1], sys.argv[2], sys.argv[3:]
p = subprocess.run([cxx, "-###", *args], capture_output=True, text=True, check=True)
lines = [l for l in p.stderr.splitlines() if "collect2" in l or l.strip().startswith('"') and "/ld" in l]
words = shlex.split(lines[-1])[1:]
with open(out, "w") as f:
    f.write("\n".join(words) + "\n")
