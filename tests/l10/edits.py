"""L10's edit table and its seeded sequences (spec/incr.md IN-63, WP-067).

One ROW per edit: its name, what it does to the sources or the link
line, the IN-27 rows and §7 reasons it is expected to exercise, the
projects that have it, and whether it is a TWO-HOP row (its inverse is
scheduled one to three steps later, and with some probability the edit
again after that: the shapes of review-in77's B-3 and B-4 traces, where
the stale datum shows on the second or third update).

An edit is an assignment of project knobs (projects/*/project.py) and
optionally an ACTION on the files (`touch`, `inplace`, `output-rewrite`).
A step is a JSON object:

  {"id": 7, "edit": "ctor-grow-shrink", "set": {"ctor.17": true},
   "action": null, "hop": 1, "fault": null}

`id` is the step's position in the generated sequence and survives
minimization (a wrong subject keyed on it still finds its step); `hop`
is 1 for an edit, 2 for its inverse, 3 for the edit again; `fault` is
the seam for H-2's crash injection: always null here (IN-63's "Faults"
are WP-068's, the runner refuses a non-null one).
"""

import random

GEN = 'gen200'
CORPUS = 'corpus40'
PROTOC = 'protoc'       # behind --project protoc (projects/protoc/project.py)
BOTH = (GEN, CORPUS)


class Row:
    def __init__(self, name, what, expects, projects, make, two_hop=False, shape=None, error=False, wp207=()):
        self.name, self.what, self.expects = name, what, expects
        self.projects, self.make, self.two_hop, self.shape = projects, make, two_hop, shape
        self.error = error
        self.wp207 = wp207      # the shapes of WP-207 §D ("Edit shapes the L10 lane must generate") it generates


def _units(proj, k, prefix, rng, locked, pred=lambda u: True):
    keys = sorted(x for x in k if x.startswith(prefix + '.') and x not in locked and pred(int(x.split('.')[1])))
    return rng.choice(keys) if keys else None


def flip(knob):
    """A two-hop toggle of one bool knob."""
    def make(proj, k, rng, locked):
        if knob in locked:
            return None
        return {knob: not k[knob]}, None
    return make


def per_unit_flip(prefix, pred=lambda u: True):
    def make(proj, k, rng, locked):
        key = _units(proj, k, prefix, rng, locked, pred)
        return None if key is None else ({key: not k[key]}, None)
    return make


def cycle(knob, n):
    def make(proj, k, rng, locked):
        if knob in locked:
            return None
        return {knob: (k[knob] + 1 + rng.randrange(n - 1)) % n}, None
    return make


def per_unit_value(prefix, values, pred=lambda u: True):
    def make(proj, k, rng, locked):
        key = _units(proj, k, prefix, rng, locked, pred)
        if key is None:
            return None
        return {key: rng.choice([v for v in values if v != k[key]])}, None
    return make


def corpus_flip(e):
    return {CORPUS: flip(e)}


def body(pred):
    return per_unit_value('body', range(8), pred)


def touch(proj, k, rng, locked):
    names = proj.all_units(k) if hasattr(proj, 'all_units') else proj.units_needed(k, None)
    return {}, 'touch:' + rng.choice(sorted(names))


def same_size_tick(proj, k, rng, locked):
    if proj.NAME == CORPUS:
        return (None if 'same' in locked else ({'same': not k['same']}, 'inplace'))
    a = body(lambda u: u < 67 or u >= 134)(proj, k, rng, locked)
    return None if a is None else (a[0], 'inplace')


def output_rewrite(proj, k, rng, locked):
    return {}, 'output-rewrite'


def ar_reorder(proj, k, rng, locked):
    keys = sorted(x for x in k if x.startswith('ar_order.') and x not in locked)
    if not keys:
        return None
    key = rng.choice(keys)
    return {key: (k[key] + 1) % 3}, None


def grow(proj, k, rng, locked):
    """One level up (gen200's GROW: 0, 1, 2, 48, 600 statements); from the
    top level back to 0. Step by step past a region, a tail, a reserve."""
    key = _units(proj, k, 'grow', rng, locked)
    if key is None:
        return None
    return {key: (k[key] + 1) % len(proj.GROW)}, None


def cxx_unit_flip(prefix):
    def make(proj, k, rng, locked):
        key = _units(proj, k, prefix, rng, locked, lambda u: proj.lang(u) == 'c++')
        return None if key is None else ({key: not k[key]}, None)
    return make


def many_files(proj, k, rng, locked):
    """Several files change in one step (8 bodies; WP-207 §D 14, the
    extraction bound and too-much-changed)."""
    keys = sorted(x for x in k if x.startswith('body.') and x not in locked)
    if len(keys) < 8:
        return None
    pick = rng.sample(keys, 8)
    return {key: rng.choice([v for v in range(len(proj.BODY)) if v != k[key]]) for key in pick}, None


ROWS = [
    # R2's edits e1-e10 (spec/incr.md §1.1 Table 1); gen200's analogue where it has one
    Row('e1', 'a constant, same size (corpus: big.cc; gen200: a unit of lib0/lib2)',
        'IN-27 file (refill in place); the floor\'s case', BOTH + (PROTOC,),
        {CORPUS: flip('e1'), GEN: body(lambda u: u < 67 or u >= 134),
         PROTOC: per_unit_value('edit', range(1, 8), lambda u: u == 0)}, two_hop=True),
    Row('e2', 'statements added to a function (corpus: once; gen200: one GROW level up, 0-1-2-48-600 '
        'statements, the top level back to 0; the inverse shrinks it)',
        'IN-27 file, global (definitions moved in their section), moved, eh-frame; IN-72 growth', BOTH,
        {CORPUS: flip('e2'), GEN: grow}, two_hop=True, wp207=(1,)),
    Row('e3', 'a comment line at the top of big.cc (__LINE__ users change)',
        'IN-27 file (same size or -4 B)', (CORPUS,), corpus_flip('e3'), two_hop=True),
    Row('e4', 'a comment line in the common header (every includer recompiled)',
        'many changed files; too-much-changed (IN-16) when the threshold is met', BOTH,
        {CORPUS: flip('e4'), GEN: cycle('hdr', 4)}, two_hop=True),
    Row('e5', 'a default argument in common.h', 'IN-27 file in 13 of 40 units', (CORPUS,), corpus_flip('e5'),
        two_hop=True),
    Row('e6', 'a member added to Widget (common.h)', 'IN-27 file, global, moved in 34 of 40 units', (CORPUS,),
        corpus_flip('e6'), two_hop=True),
    Row('e7', 'a function renamed in 4 units', 'symbols new and gone; IN-27 global', (CORPUS,), corpus_flip('e7'),
        two_hop=True),
    Row('e8', 'a line above checked_07 in t07.cc', 'IN-27 file, same size', (CORPUS,), corpus_flip('e8'), two_hop=True),
    Row('e9', 'a global function added (and removed)', 'IN-27 file, global; symbols new / gone', BOTH,
        {CORPUS: flip('e9'), GEN: per_unit_flip('newfn')}, two_hop=True, wp207=(2,)),
    Row('e10', 'a string literal made longer (and back)', 'IN-27 frag (merged strings), file', BOTH,
        {CORPUS: flip('e10'), GEN: per_unit_flip('lit')}, two_hop=True),
    # Table 1's other rows
    Row('member-replace', 'an archive member\'s bytes replaced (corpus: t07 same size; gen200: a thin lib1.a member)',
        'IN-24: a changed member; thin: the member file changes, the archive not', BOTH + (PROTOC,),
        {CORPUS: flip('same'), GEN: body(lambda u: 67 <= u < 134),
         PROTOC: per_unit_value('edit', range(1, 8), lambda u: u >= 1)}, two_hop=True),
    Row('extraction', 'a member newly extracted, then dropped (corpus: t07 calls tx_fn; gen200: u070 calls l10_lazy_fn)',
        'appeared / extracted, gone / unextracted files; .init_array and l10_reg grow (gen200)', BOTH,
        {CORPUS: flip('call'), GEN: flip('extract')}, two_hop=True, wp207=(10,)),
    Row('add-remove-input', 'an input added to the line and removed (corpus: tx.o; gen200: x_extra.o)',
        'appeared / gone file; §7: not options-changed; packed sections grow (gen200)', BOTH,
        {CORPUS: flip('tx_add'), GEN: flip('extra_input')}, two_hop=True),
    Row('sym-attr', 'a global made weak or hidden, and back (l10_attr_fn)',
        'IN-71 attrs (a refill in a static executable); symbol-kind-changed only for IFUNC/TLS/ABS', (GEN,),
        {GEN: cycle('attr', 3)}, two_hop=True, wp207=(7,)),
    Row('decl-hidden', 'u041 declares l10_attr_fn hidden and calls it (the definer u040 is demoted), and back',
        'IN-71 attrs of an unchanged definer (review-in72 N2: n2_visibility_demotes_unchanged_definer)', (GEN,),
        {GEN: flip('decl_hidden')}, two_hop=True, wp207=(7,)),
    Row('global-var', 'a global variable of a unit added, removed, grown, or moved between .data, .bss, '
        '.rodata and .data.rel.ro (gen200 gvar levels)',
        'IN-27 file, global; output sections the file had nothing in; .bss growth', (GEN,),
        {GEN: per_unit_value('gvar', range(7))}, two_hop=True, wp207=(2, 3)),
    Row('const-remove', 'a floating constant and a string literal removed, then restored',
        'IN-27 frag (merged constants die and revive: merged_constant_dies_revives)', (GEN,),
        {GEN: per_unit_flip('flit')}, two_hop=True, wp207=(9,)),
    Row('rename', 'a unit\'s helper function renamed, and back', 'symbols gone / new; IN-27 global', (GEN,),
        {GEN: per_unit_flip('rename')}, two_hop=True, wp207=(11,)),
    Row('static-global', 'a unit\'s helper function made static, and global again',
        'a global gone and new (the last global moves: global_removed_twice_last_moved)', (GEN,),
        {GEN: per_unit_flip('static')}, two_hop=True, wp207=(11,)),
    Row('eh-add', 'a throw/catch added to a C++ unit\'s function, and removed',
        'LSDA and .eh_frame ranges (.gcc_except_table appears in the file)', (GEN,),
        {GEN: cxx_unit_flip('eh')}, two_hop=True, wp207=(12,)),
    Row('many-files', 'eight units\' constants changed in one step, and back',
        'many changed files at once (the extraction bound; too-much-changed when over the threshold)', (GEN,),
        {GEN: many_files}, two_hop=True, wp207=(14,)),
    # IN-63's additions
    Row('move-def', 'a definition moved between files (l10_moved: u030 <-> u031)',
        'IN-27 global (definer changed); IN-71', (GEN,), {GEN: cycle('move_def', 2)}, two_hop=True),
    Row('comdat-keeper', 'the COMDAT keeper changes both ways (u002 stops / starts instantiating L10Keep<1>)',
        'IN-27 comdat', (GEN,), {GEN: flip('keeper')}, two_hop=True, wp207=(6,)),
    Row('gc-kill-revive', 'l10_gc_helper (unchanged u011) dies and revives under --gc-sections',
        'IN-27 liveness; B-3 trace 1 (gc_section_dies_then_revives) on the second hop', (GEN,),
        {GEN: flip('gc_call')}, two_hop=True, shape='B-3 trace 1', wp207=(5,)),
    Row('gc-revive-first', 'u012 starts calling l10_gc_helper2 (unchanged u013), which no live code called at the '
        'first link; then stops', 'IN-27 liveness; B-3 trace 1 with the first link never placing it', (GEN,),
        {GEN: flip('gc_call2')}, two_hop=True, shape='B-3 trace 1 (never placed)', wp207=(5,)),
    Row('tls-grow-shrink', 'a TLS array grows and shrinks', '§7 tls-layout-changed', (GEN,),
        {GEN: per_unit_value('tls', (1, 2, 3, 4))}, two_hop=True, wp207=(7,)),
    Row('ctor-grow-shrink', 'a unit gains and loses a constructor (.init_array grows and shrinks)',
        'IN-27 synthetic / packed sections, __init_array_end; B-3 trace 2 (init_array_grows_then_shrinks)', (GEN,),
        {GEN: per_unit_flip('ctor')}, two_hop=True, shape='B-3 trace 2', wp207=(4,)),
    Row('registry-grow-shrink', 'a unit gains and loses an l10_reg entry (__start_/__stop_ set grows and shrinks)',
        'packed section, __stop_l10_reg; B-3 trace 2 (start_stop_registry_grows_then_shrinks)', (GEN,),
        {GEN: per_unit_flip('reg')}, two_hop=True, shape='B-3 trace 2 (registry)', wp207=(4,)),
    Row('common-ref', 'u020 starts and stops reading COMMON l10_counter, which only unchanged u021 mentions',
        'IN-71 commons; B-4 trace 1 (first hop) and trace 2 (third hop)', (GEN,),
        {GEN: flip('common_ref')}, two_hop=True, shape='B-4 traces 1, 2', wp207=(8,)),
    Row('common-grow', 'u021\'s -fcommon COMMON l10_cbuf grows from int[4] to int[64], and back',
        'IN-71 commons; n4_common_grows / two_commons_one_grows (l10_counter never moves)', (GEN,),
        {GEN: cycle('common_size', 2)}, two_hop=True, wp207=(8,)),
    Row('fcommon-ref', 'u022 starts and stops reading the Fortran COMMON /l10blk/ (or its C stand-in)',
        'IN-71 commons; B-4 trace 1, FORTRAN COMMON', (GEN,), {GEN: flip('fcommon_ref')}, two_hop=True,
        shape='B-4 trace 1 (Fortran)'),
    Row('fortran-body', 'the Fortran unit\'s constant', 'IN-27 file (a gfortran object)', (GEN,), {GEN: cycle('fbody', 4)}),
    Row('ar-reorder', 'an archive\'s members rotated', '§7 inputs-reordered (extraction order); COMDAT keepers',
        BOTH + (PROTOC,), {CORPUS: ar_reorder, GEN: ar_reorder, PROTOC: ar_reorder}, two_hop=True),
    Row('touch', 'an input touched without change', 'nothing (hashed-same); never a fall-back', BOTH + (PROTOC,),
        {CORPUS: touch, GEN: touch, PROTOC: touch}, wp207=(13,)),
    Row('same-size-in-tick', 'a same-size rewrite IN PLACE, right after the previous link (clock_race.py, R6)',
        'IN-22a racy input hashed; IN-27 file', BOTH, {CORPUS: same_size_tick, GEN: same_size_tick}),
    Row('output-rewrite', 'the output rewritten in place with its own bytes right after its link (state_tick.py, R6)',
        '§7 output-changed, or racy and hashed (IN-21)', BOTH + (PROTOC,),
        {CORPUS: output_rewrite, GEN: output_rewrite, PROTOC: output_rewrite}),
    Row('option-flip', '--gc-sections on and off', '§7 options-changed', BOTH + (PROTOC,),
        {CORPUS: flip('gc'), GEN: flip('gc'), PROTOC: flip('gc')}, two_hop=True),
    Row('undef-fix', 'an undefined symbol introduced, then fixed', 'both links fail alike; then the state of the last '
        'success is used', (GEN,), {GEN: flip('undef')}, two_hop=True, error=True),
    Row('dup-fix', 'a duplicate definition introduced, then fixed', 'both links fail alike', (GEN,),
        {GEN: flip('dup')}, two_hop=True, error=True),
]
BY_NAME = {r.name: r for r in ROWS}


def rows_for(project, only=None):
    return [r for r in ROWS if project in r.projects and (only is None or r.name in only)]


def generate(proj, cfg, seed, steps, only=None, hops=None):
    """The seeded sequence: a list of steps (module docstring). The same
    (project, configuration, seed, steps, rows, hops) gives the same list on
    any host: random.Random seeded with a string is stable across platforms.
    hops=3 is the THREE-HOP mode (WP-207 §D 14): every two-hop edit is
    followed by its inverse and then by the edit again."""
    rng = random.Random('l10/%s/%s/%d%s' % (proj.NAME, cfg.name, seed, '/h3' if hops == 3 else ''))
    rows = rows_for(proj.NAME, only)
    k = proj.knobs()
    pending = []      # [due step, row name, assignment, hop]
    locked = set()
    out = []
    for i in range(steps):
        due = [p for p in pending if p[0] <= i]
        if due:
            p = min(due, key=lambda p: (p[0], p[1]))
            pending.remove(p)
            _due, name, assign, hop = p
            for key in assign:
                locked.discard(key)
            step = {'id': i, 'edit': name, 'set': assign, 'action': None, 'hop': hop, 'fault': None}
        else:
            step = None
            for _try in range(20):
                row = rng.choice(rows)
                made = row.make[proj.NAME](proj, k, rng, locked)
                if made is None:
                    continue
                assign, action = made
                step = {'id': i, 'edit': row.name, 'set': assign, 'action': action, 'hop': 1, 'fault': None}
                if row.two_hop and assign and (row.error or hops == 3 or rng.random() < 0.7):
                    inverse = {key: k[key] for key in assign}
                    gap = rng.choice((1, 1, 2, 3)) if not row.error else rng.choice((1, 2))
                    pending.append([i + gap, row.name, inverse, 2])
                    locked.update(assign)
                    if not row.error and (hops == 3 or rng.random() < 0.5):
                        pending.append([i + gap + rng.choice((1, 2)), row.name, dict(assign), 3])
                break
            if step is None:
                step = {'id': i, 'edit': 'touch', 'set': {}, 'action': touch(proj, k, rng, locked)[1], 'hop': 1,
                        'fault': None}
        # a third hop re-applies the edit: lock again until it is applied
        for p in pending:
            if p[3] == 3 and p[0] > i:
                locked.update(p[2])
        k.update(step['set'])
        out.append(step)
    return out


def state_at(proj, steps):
    """The knobs after applying `steps` in order to the defaults."""
    k = proj.knobs()
    for s in steps:
        k.update(s['set'])
    return k
