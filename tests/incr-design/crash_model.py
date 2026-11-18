#!/usr/bin/env python3
"""WP-205 (R-C): an enumerator of IN-77's write sequences under a kill and a
power loss.

Model. Files are inodes with named fields; a directory maps names to inodes.
Every protocol is a list of operations:
  ('w', inode, field, value)   a positioned write (or a size change)
  ('meta', inode, field, value) a timestamp change (mtime/ctime): fdatasync
                                does NOT persist it, fsync does
  ('sync', inode)              fdatasync: every earlier 'w' to the inode is durable
  ('create', name, inode)      a new directory entry (O_CREAT)
  ('rename', src, dst)         rename(2)
  ('dsync',)                   fsync of the (one) directory: every earlier
                               create/rename is durable
A KILL after k operations leaves the first k operations, all visible (page
cache). A POWER LOSS after k operations leaves every operation that a
completed sync covers, and ANY subset of the others (each independently;
the latest persisted write of a field wins). This is a superset of what a
real block device does (it may show a later write without an earlier one
to another field of the same block), so every trusted mix it reports is
checked by hand in the report; a protocol it calls clean is clean under
weaker assumptions too.

The next link (IN-77, IN-21, IN-18c) TRUSTS the pair when: the state the
directory names has its flag clear; its hashed parts (header/directory,
blocks) are all of one version (a mix is state-corrupt); and the binding
that version records matches the output the directory names: the inode,
the size, the mtime/ctime, and (unless --build-id=none) the build-id note.
A trusted pair is BAD when the output's content is not wholly the version
the state describes.

WP-207 additions (tests/incr-design/crash_model.py; the reviewer's original
is .build/review-in77/crash_model.py):
  - every positioned write to an output inode also changes its mtime to the
    time of the write (Linux file_update_time: write(2) sets mtime and
    ctime); ('fsync', inode) persists its writes AND its timestamps;
  - a FRESH TORN outcome: a file at the output's path whose content is not
    one version (or whose header is zeroed) and whose mtime is a time
    after the edit that started the link (NEW): make and ninja then see the
    output newer than its inputs and never run the repairing link (in77
    M-3). OLD (the previous link's time, before the edit) and EPOCH are not
    fresh;
  - variants: inplace-epoch (in77 M-3's proposal: W2 sets the mtime to the
    epoch), inplace-aside (WP-207's IN-77: W2 renames P aside, W4 renames
    it back), full-incremental-bootid (in77 M-1 (b): a full link records
    "output not synced"; after a power loss (a new boot id) such a state's
    output is checked against its leaves before it is trusted).

WP-211 additions: ops ('link', src, dst) (EEXIST leaves dst), ('unlink',
name), ('fcreate', name, inode) (another link's durable output); variants
inplace-aside-link (W4 = link + dsync, then unlink + dsync), and the same
and WP-207's rename with a foreign output created at P inside the window;
outcomes report a lost foreign output and P and A naming one inode.

Usage: python3 -I crash_model.py [variant ...]   (no argument: all)
"""
import itertools
import sys

OLD, NEW, ZERO, EPOCH = 'old', 'new', 'zero', 'epoch'


def initial():
    # The old pair, as a previous link left it, durable.
    inodes = {
        'P0': {'hdr': OLD, 'w1': OLD, 'w2': OLD, 'note': OLD, 'size': OLD, 'mtime': OLD},
        'S0': {'flag': 0, 'head': OLD, 'b1': OLD, 'b2': OLD, 'synced': 1},
    }
    names = {'P': 'P0', 'S': 'S0'}
    return inodes, names


def new_output_inode():
    return {'hdr': ZERO, 'w1': ZERO, 'w2': ZERO, 'note': ZERO, 'size': ZERO, 'mtime': NEW}


def new_state_inode():
    return {'flag': 0, 'head': ZERO, 'b1': ZERO, 'b2': ZERO, 'synced': 1}


# ---------------------------------------------------------------- protocols
def inplace(compact=False, header_clears_flag=False):
    """IN-77 W1..W6. header_clears_flag: W5 rewrites the header from an
    in-memory copy whose flags word is 0 (the floor's encode does this)."""
    ops = [
        ('W1', ('w', 'S0', 'flag', 1)), ('W1', ('sync', 'S0')),
        ('W2', ('w', 'P0', 'hdr', ZERO)), ('W2', ('meta', 'P0', 'mtime', NEW)), ('W2', ('sync', 'P0')),
        ('W3', ('w', 'P0', 'w1', NEW)), ('W3', ('w', 'P0', 'w2', NEW)), ('W3', ('w', 'P0', 'note', NEW)),
        ('W3', ('w', 'P0', 'size', NEW)), ('W3', ('sync', 'P0')),
        ('W4', ('w', 'P0', 'hdr', NEW)), ('W4', ('sync', 'P0')),
    ]
    if compact:
        ops += [
            ('K', ('create', 'Stmp', 'S1')),
            ('K', ('w', 'S1', 'head', NEW)), ('K', ('w', 'S1', 'b1', NEW)), ('K', ('w', 'S1', 'b2', NEW)),
            ('K', ('sync', 'S1')), ('K', ('rename', 'Stmp', 'S')), ('K', ('dsync',)),
        ]
    else:
        ops += [('W5', ('w', 'S0', 'b1', NEW)), ('W5', ('w', 'S0', 'b2', NEW)),
                ('W5', ('w', 'S0', 'head', NEW))]
        if header_clears_flag:
            ops += [('W5', ('w', 'S0', 'flag', 0))]
        ops += [('W5', ('sync', 'S0')), ('W6', ('w', 'S0', 'flag', 0)), ('W6', ('sync', 'S0'))]
    return ops, {'S0': ('P0',), 'S1': ('P0',)}


def bycopy(compact=False, sync_t=True, dsync_after_rename=True):
    ops = [
        ('C1', ('create', 'T', 'T1')),
        ('C1', ('w', 'T1', 'hdr', NEW)), ('C1', ('w', 'T1', 'w1', NEW)), ('C1', ('w', 'T1', 'w2', NEW)),
        ('C1', ('w', 'T1', 'note', NEW)), ('C1', ('w', 'T1', 'size', NEW)),
    ]
    if sync_t:
        ops += [('C1', ('sync', 'T1'))]
    ops += [('C2', ('w', 'S0', 'flag', 1)), ('C2', ('sync', 'S0')),
            ('C3', ('rename', 'T', 'P'))]
    if dsync_after_rename:
        ops += [('C3', ('dsync',))]
    if compact:
        ops += [
            ('K', ('create', 'Stmp', 'S1')),
            ('K', ('w', 'S1', 'head', NEW)), ('K', ('w', 'S1', 'b1', NEW)), ('K', ('w', 'S1', 'b2', NEW)),
            ('K', ('sync', 'S1')), ('K', ('rename', 'Stmp', 'S')), ('K', ('dsync',)),
        ]
    else:
        ops += [('C4', ('w', 'S0', 'b1', NEW)), ('C4', ('w', 'S0', 'b2', NEW)),
                ('C4', ('w', 'S0', 'head', NEW)), ('C4', ('sync', 'S0')),
                ('C5', ('w', 'S0', 'flag', 0)), ('C5', ('sync', 'S0'))]
    return ops, {'S0': ('T1',), 'S1': ('T1',)}


def full_incremental(sync_output=False):
    """A full link in incremental mode as today's code does it
    (incremental.rs write_atomically; image.md I-61): the output written to
    a temporary and renamed, NO sync of it nor of the directory; then the
    state to a temporary, fdatasync, rename, fsync of the directory."""
    ops = [
        ('F1', ('create', 'T', 'T1')),
        ('F1', ('w', 'T1', 'hdr', NEW)), ('F1', ('w', 'T1', 'w1', NEW)), ('F1', ('w', 'T1', 'w2', NEW)),
        ('F1', ('w', 'T1', 'note', NEW)), ('F1', ('w', 'T1', 'size', NEW)),
    ]
    if sync_output:
        ops += [('F1', ('sync', 'T1'))]
    ops += [('F2', ('rename', 'T', 'P'))]
    if sync_output:
        ops += [('F2', ('dsync',))]
    ops += [('F3', ('create', 'Stmp', 'S1')),
            ('F3', ('w', 'S1', 'head', NEW)), ('F3', ('w', 'S1', 'b1', NEW)), ('F3', ('w', 'S1', 'b2', NEW)),
            ('F3', ('sync', 'S1')), ('F4', ('rename', 'Stmp', 'S')), ('F4', ('dsync',))]
    return ops, {'S1': ('T1',)}


def wp(ino, field, val):
    # a positioned write to an output inode, and the mtime it sets
    return [('w', ino, field, val), ('meta', ino, 'mtime', NEW)]


def inplace_writes(ino, epoch=False):
    ops = [('W2', o) for o in wp(ino, 'hdr', ZERO)]
    if epoch:
        ops += [('W2', ('meta', ino, 'mtime', EPOCH)), ('W2', ('fsync', ino))]
    else:
        ops += [('W2', ('sync', ino))]
    for f in ('w1', 'w2', 'note', 'size'):
        ops += [('W3', o) for o in wp(ino, f, NEW)]
    ops += [('W3', ('sync', ino))]
    ops += [('W4', o) for o in wp(ino, 'hdr', NEW)] + [('W4', ('fsync', ino))]
    return ops


def inplace_epoch():
    ops = [('W1', ('w', 'S0', 'flag', 1)), ('W1', ('sync', 'S0'))] + inplace_writes('P0', epoch=True)
    ops += [('W5', ('w', 'S0', 'b1', NEW)), ('W5', ('w', 'S0', 'b2', NEW)), ('W5', ('w', 'S0', 'head', NEW)),
            ('W5', ('sync', 'S0')), ('W6', ('w', 'S0', 'flag', 0)), ('W6', ('sync', 'S0'))]
    return ops, {'S0': ('P0',)}


def inplace_aside(compact=False):
    # WP-207 IN-77: W2 renames P to its aside name A (the same inode) and
    # syncs the directory before any byte of P changes; W4 renames it back
    # after the header's fsync, and syncs the directory.
    ops = [('W1', ('w', 'S0', 'flag', 1)), ('W1', ('sync', 'S0')),
           ('W2', ('rename', 'P', 'A')), ('W2', ('dsync',))] + inplace_writes('P0')
    ops += [('W4', ('rename', 'A', 'P')), ('W4', ('dsync',))]
    if compact:
        ops += [('K', ('create', 'Stmp', 'S1')), ('K', ('w', 'S1', 'head', NEW)), ('K', ('w', 'S1', 'b1', NEW)),
                ('K', ('w', 'S1', 'b2', NEW)), ('K', ('sync', 'S1')), ('K', ('rename', 'Stmp', 'S')), ('K', ('dsync',))]
    else:
        ops += [('W5', ('w', 'S0', 'b1', NEW)), ('W5', ('w', 'S0', 'b2', NEW)), ('W5', ('w', 'S0', 'head', NEW)),
                ('W5', ('sync', 'S0')), ('W6', ('w', 'S0', 'flag', 0)), ('W6', ('sync', 'S0'))]
    return ops, {'S0': ('P0',), 'S1': ('P0',)}


def inplace_aside_link(foreign=False, rename_back=False):
    # WP-211 (review-wp207 M-4): W4 is link(A, P), fsync of the directory,
    # then unlink(A), fsync of the directory: two separately durable steps.
    # foreign=True: another (plain or full) link creates P (inode PF) after
    # W2's rename and before W4; then link(A, P) fails with EEXIST and the
    # update removes A, keeps the flag set and stops (output-changed).
    # rename_back=True: WP-207's rename(A, P) instead, for comparison.
    ops = [('W1', ('w', 'S0', 'flag', 1)), ('W1', ('sync', 'S0')),
           ('W2', ('rename', 'P', 'A')), ('W2', ('dsync',))] + inplace_writes('P0')
    if foreign:
        ops.insert(6, ('X', ('fcreate', 'P', 'PF')))
    if rename_back:
        ops += [('W4', ('rename', 'A', 'P')), ('W4', ('dsync',))]
    elif foreign:
        ops += [('W4', ('unlink', 'A')), ('W4', ('dsync',))]
        return ops, {'S0': ('P0',)}
    else:
        ops += [('W4', ('link', 'A', 'P')), ('W4', ('dsync',)), ('W4u', ('unlink', 'A')), ('W4u', ('dsync',))]
    ops += [('W5', ('w', 'S0', 'b1', NEW)), ('W5', ('w', 'S0', 'b2', NEW)), ('W5', ('w', 'S0', 'head', NEW)),
            ('W5', ('sync', 'S0')), ('W6', ('w', 'S0', 'flag', 0)), ('W6', ('sync', 'S0'))]
    return ops, {'S0': ('P0',)}


def inplace_mtime_writes():
    # the reviewer's in-place order with every write also setting the mtime
    ops = [('W1', ('w', 'S0', 'flag', 1)), ('W1', ('sync', 'S0'))] + inplace_writes('P0')
    ops += [('W5', ('w', 'S0', 'b1', NEW)), ('W5', ('w', 'S0', 'b2', NEW)), ('W5', ('w', 'S0', 'head', NEW)),
            ('W5', ('sync', 'S0')), ('W6', ('w', 'S0', 'flag', 0)), ('W6', ('sync', 'S0'))]
    return ops, {'S0': ('P0',)}


def full_bootid():
    ops, bound = full_incremental()
    # the new state records synced = 0 (the output was renamed unsynced)
    i = [k for k, (st, o) in enumerate(ops) if o == ('w', 'S1', 'b2', NEW)][0]
    ops.insert(i + 1, ('F3', ('w', 'S1', 'synced', 0)))
    return ops, bound


VARIANTS = {
    'inplace-mtime-writes': lambda: inplace_mtime_writes(),
    'inplace-epoch': lambda: inplace_epoch(),
    'inplace-aside': lambda: inplace_aside(),
    'inplace-aside-compact': lambda: inplace_aside(compact=True),
    'inplace-aside-link': lambda: inplace_aside_link(),
    'inplace-aside-link-foreign': lambda: inplace_aside_link(foreign=True),
    'inplace-aside-rename-foreign': lambda: inplace_aside_link(foreign=True, rename_back=True),
    'full-incremental-bootid': lambda: full_bootid(),
    'inplace': lambda: inplace(),
    'inplace-compact': lambda: inplace(compact=True),
    'inplace-W5-header-flag0': lambda: inplace(header_clears_flag=True),
    'copy': lambda: bycopy(),
    'copy-compact': lambda: bycopy(compact=True),
    'copy-no-sync-T': lambda: bycopy(sync_t=False),
    'copy-no-dirsync': lambda: bycopy(dsync_after_rename=False),
    'full-incremental-today': lambda: full_incremental(),
    'full-incremental-synced': lambda: full_incremental(sync_output=True),
}


# ---------------------------------------------------------------- semantics
def apply(ops_persisted):
    inodes, names = initial()
    for op in ops_persisted:
        k = op[0]
        if k in ('w', 'meta'):
            _, ino, field, val = op
            inodes.setdefault(ino, new_output_inode() if ino.startswith(('P', 'T')) else new_state_inode())
            inodes[ino][field] = val
        elif k == 'create':
            _, name, ino = op
            inodes.setdefault(ino, new_output_inode() if ino.startswith(('P', 'T')) else new_state_inode())
            names[name] = ino
        elif k == 'rename':
            _, src, dst = op
            if src in names:
                names[dst] = names.pop(src)
        elif k == 'link':                   # link(2): EEXIST leaves dst
            _, src, dst = op
            if src in names and dst not in names:
                names[dst] = names[src]
        elif k == 'unlink':
            names.pop(op[1], None)
        elif k == 'fcreate':                # another link's output, durable
            _, name, ino = op
            inodes[ino] = {f: 'foreign' for f in ('hdr', 'w1', 'w2', 'note', 'size', 'mtime')}
            names[name] = ino
    return inodes, names


def judge(inodes, names, bound, build_id=True, rebooted=False):
    """(trusted, bad, description)."""
    s = inodes.get(names.get('S'))
    p_ino = names.get('P')
    p = inodes.get(p_ino)
    if s is None or p is None:
        return False, False, 'missing'
    parts = {s['head'], s['b1'], s['b2']}
    if len(parts) != 1 or ZERO in parts:
        return False, False, 'state-corrupt'
    ver = parts.pop()
    if s['flag']:
        return False, False, 'output-torn'
    want_ino = 'P0' if ver == OLD else bound[names['S']][0]
    if p_ino != want_ino:
        return False, False, 'output-changed (inode)'
    if p['size'] != ver or p['mtime'] != ver:
        return False, False, 'output-changed (size/mtime)'
    if build_id and p['note'] != ver:
        return False, False, 'output-changed (build id)'
    content = (p['hdr'], p['w1'], p['w2'], p['note'])
    bad = any(c != ver for c in content)
    if rebooted and not s.get('synced', 1) and bad:
        return False, False, 'output-changed (leaves checked after a reboot)'
    return True, bad, f'trusted {ver} state, output {content}'


def fresh_torn(inodes, names):
    p = inodes.get(names.get('P'))
    if p is None:
        return False
    torn = p['hdr'] == ZERO or len({p['hdr'], p['w1'], p['w2'], p['note']}) != 1
    return torn and p['mtime'] == NEW


def runnable_mix(inodes, names):
    p = inodes.get(names.get('P'))
    if p is None or p['hdr'] == ZERO:
        return False
    return len({p['hdr'], p['w1'], p['w2']}) != 1


def enumerate_variant(name, build_id=True):
    ops, bound = VARIANTS[name]()
    seen = {}
    for k in range(len(ops) + 1):
        prefix = [o for _, o in ops[:k]]
        label = ops[k - 1][0] + f'#{k}' if k else 'start'
        # kill: the whole prefix is visible
        scenarios = [('kill', prefix)]
        # power loss: durable = covered by a completed sync; free = the rest
        forced, free = [], []
        for i, op in enumerate(prefix):
            later = prefix[i + 1:]
            if op[0] in ('w',):
                covered = any(o in (('sync', op[1]), ('fsync', op[1])) for o in later)
            elif op[0] == 'meta':
                covered = any(o == ('fsync', op[1]) for o in later)
            elif op[0] in ('create', 'rename', 'link', 'unlink'):
                covered = any(o == ('dsync',) for o in later)
            elif op[0] == 'fcreate':
                covered = True               # the other link synced its own rename
            else:
                covered = False  # meta: fdatasync never covers it (fsync not used)
            (forced if covered else free).append(i)
        for bits in itertools.product((0, 1), repeat=len(free)):
            keep = set(forced) | {i for i, b in zip(free, bits) if b}
            scenarios.append(('power', [o for i, o in enumerate(prefix) if i in keep]))
        for how, persisted in scenarios:
            inodes, names = apply(persisted)
            trusted, bad, desc = judge(inodes, names, bound, build_id, rebooted=(how == 'power'))
            mix = runnable_mix(inodes, names)
            ft = fresh_torn(inodes, names)
            if 'PF' in inodes and names.get('P') != 'PF':
                desc += ' [FOREIGN OUTPUT LOST]'
            if 'A' in names:
                desc += ' [aside name left' + (', same inode as P' if names.get('A') == names.get('P') else '') + ']'
            key = (how, desc, bad, mix, ft)
            seen.setdefault(key, label)
    return seen


def main():
    wanted = sys.argv[1:] or list(VARIANTS)
    total_bad = 0
    for v in wanted:
        for build_id in (True, False):
            seen = enumerate_variant(v, build_id)
            bad = [(k, lab) for k, lab in seen.items() if k[2]]
            mixes = [(k, lab) for k, lab in seen.items() if k[3]]
            fresh = [(k, lab) for k, lab in seen.items() if k[4]]
            lost = [(k, lab) for k, lab in seen.items() if 'FOREIGN OUTPUT LOST' in k[1]]
            two = [(k, lab) for k, lab in seen.items() if 'same inode as P' in k[1]]
            print(f'== {v} (build id {"on" if build_id else "none"}): '
                  f'{len(seen)} distinct outcomes, {len(bad)} trusted mixes, '
                  f'{len(mixes)} runnable-mix outputs, {len(fresh)} fresh torn outputs, '
                  f'{len(lost)} foreign outputs lost, {len(two)} with P and A one inode')
            for (how, desc, _, _, _), lab in sorted(lost + two, key=lambda x: x[1]):
                print(f'   note         first at {lab} ({how}): {desc}')
            for (how, desc, _, _, _), lab in sorted(bad, key=lambda x: x[1]):
                print(f'   TRUSTED MIX  first at {lab} ({how}): {desc}')
            for (how, desc, _, _, _), lab in sorted(mixes, key=lambda x: x[1]):
                print(f'   runnable mix first at {lab} ({how}): next link: {desc}')
            for (how, desc, _, _, _), lab in sorted(fresh, key=lambda x: x[1]):
                print(f'   FRESH TORN   first at {lab} ({how}): next link (if run): {desc}')
            total_bad += len(bad)
    print(f'TOTAL trusted mixes: {total_bad}')


if __name__ == '__main__':
    main()
