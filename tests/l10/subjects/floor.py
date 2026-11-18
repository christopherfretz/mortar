#!/usr/bin/env python3
"""The test path's update as L10's subject (WP-067; crates/mortar/src/
floor.rs, the `incr_floor` example named by L10_FLOOR): the first link,
or a link whose state is missing, is mortar's; every later one is
`incr_floor in-place -- ARGS` (WP-061's same-size floor), its report one
line `incremental: update (floor) ...`. When the floor refuses, mortar
links in full and the report says why the floor refused. The floor's own
output (its result and phase lines) goes to floor.log, not to the
linker's diagnostics."""
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, opt, output  # noqa: E402

args = sys.argv[1:]
floor = os.environ.get('L10_FLOOR')
rep = opt(args, '--incremental-report')
state = output(args) + '.mortar-state'
if not floor or not os.path.exists(state) or not os.path.exists(output(args)):
    finish(mortar(args)[0])
fargs = [a for a in args if not a.startswith('--incremental-report=')]
p = subprocess.run([floor, 'in-place', '--'] + fargs, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                   errors='replace')
with open('floor.log', 'a') as f:
    f.write(p.stdout)
first = (p.stdout.splitlines() or ['?'])[0]
if p.returncode == 0:
    if rep:
        with open(rep, 'w') as f:
            f.write('incremental: update (floor) %s\n' % first)
    finish(0)
rc = mortar(args)[0]
if rep and os.path.exists(rep):
    with open(rep, 'a') as f:
        f.write('l10: the floor refused: %s\n' % first)
finish(rc)
