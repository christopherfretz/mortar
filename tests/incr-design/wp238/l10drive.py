#!/usr/bin/env python3
"""l10drive.py PROBE WORK CONFIG [ROWS] -- WP-238 step 1, experiments (a), (b), (d).

Links L10's gen200 (tests/l10) in configuration CONFIG (an `fs`
configuration: --gc-sections is on) with the probe build of mortar
(`--features mortar_live/wp238-probe`, MORTAR_WP238 and MORTAR_WP238_ROWS
set): the base state, then for each L10 edit row of gen200 (ROWS, comma
separated, default every row that links) the state after the row's
first hop. Per link: the probe's summary (a: replay and per-kind drops),
livemodel.py over the link (b), and the rows dump. Then emulate.py over
(base -> edited) and (edited -> base) for each row (d).

WORK is a scratch directory (removed by the caller); each link's dump
directory is WORK/links/<label>. Prints one line per link and per
emulation, then `FINAL ...`.
"""
import os, random, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
L10 = os.path.join(HERE, '..', '..', 'l10')
sys.path.insert(0, L10)
import build, edits, l10  # noqa: E402

SKIP = {'undef-fix', 'dup-fix', 'option-flip', 'touch', 'same-size-in-tick', 'output-rewrite'}


def main():
    probe, work, cfgname = sys.argv[1:4]
    only = sys.argv[4].split(',') if len(sys.argv) > 4 and sys.argv[4] else None
    work = os.path.abspath(work)
    os.makedirs(work, exist_ok=True)
    cfg = build.Cfg(cfgname)
    proj = l10.load_project('gen200')
    seq = l10.Seq('gen200', cfgname, 0, [], label='wp238')
    opts = {'work': work, 'mortar': probe, 'xtask': '-', 'keep': True}
    r = l10.Runner(seq, opts)
    if os.path.isdir(r.base):
        shutil.rmtree(r.base)
    os.makedirs(r.W)
    os.makedirs(r.stage)
    links = os.path.join(work, 'links')
    os.makedirs(links, exist_ok=True)

    def link(label, k):
        files, words, options, objs = r.wanted(k)
        r.install(files, objs)
        r.remove_stale(files)
        d = os.path.join(links, label)
        if os.path.isdir(d):
            shutil.rmtree(d)
        os.makedirs(d)
        env = dict(os.environ, MORTAR_WP238=d, MORTAR_WP238_ROWS='1')
        argv = [probe] + r.pre + options + words + r.post + ['-o', 'prog', '--mortar-map=prog.map']
        p = subprocess.run(argv, cwd=r.W, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                           errors='replace', timeout=300)
        if p.returncode != 0:
            print('LINK %s failed: %s' % (label, p.stdout[:300]), flush=True)
            return None
        sums = [f for f in os.listdir(d) if f.endswith('.sum')]
        if not sums:
            print('LINK %s: no probe summary (no --gc-sections?) options %s' % (label, options), flush=True)
            return None
        s = open(os.path.join(d, sums[0])).read()
        rep = [l for l in s.splitlines() if l.startswith('replay') or l.startswith('total')]
        lm = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'livemodel.py'), r.W, '--frag', '--quiet'], cwd=r.W,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace')
        lml = [l.strip() for l in lm.stdout.splitlines() if l.startswith('==') or 'fragment sets' in l]
        if not lml:
            lml = ['NO RESULT (rc %d): %s' % (lm.returncode, ' | '.join(lm.stdout.strip().splitlines()[-3:]))]
        print('LINK %s: %s | livemodel: %s' % (label, '; '.join(rep), ' / '.join(lml)), flush=True)
        rows_path = os.path.join(d, sums[0][:-4] + '.rows')
        if os.path.exists(rows_path):
            o = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'oracle_design.py'), rows_path],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace')
            print('ORA1 %s %s: %s' % (cfgname, label, (o.stdout.strip().splitlines() or ['no output'])[-1]),
                  flush=True)
        shutil.copy(os.path.join(r.W, 'prog.map'), os.path.join(d, 'prog.map'))
        os.remove(os.path.join(r.W, 'prog'))
        return os.path.join(d, sums[0][:-4] + '.rows')

    k0 = proj.knobs()
    base = link('base', k0)
    if base is None:
        print('FINAL base link failed')
        return
    rows = [x for x in edits.rows_for('gen200', only) if x.name not in SKIP]
    done = 0
    bad = 0
    ora = __import__('collections').Counter()
    for row in rows:
        k = proj.knobs()
        made = row.make['gen200'](proj, k, random.Random('wp238/' + row.name), set())
        if made is None:
            print('ROW %s: no edit made' % row.name, flush=True)
            continue
        assign, _action = made
        k.update(assign)
        new = link(row.name, k)
        if new is None:
            continue
        for a, b, way in ((base, new, 'fwd'), (new, base, 'back')):
            p = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'emulate.py'), a, b],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace')
            lines = p.stdout.strip().splitlines() or ['no output']
            line = lines[-1]
            print('EMU %s %s %s: %s || %s' % (cfgname, row.name, way, line, ' | '.join(lines[:-1])), flush=True)
            done += 1
            bad += 0 if line.startswith('AGREE') else 1
            p = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'oracle_design.py'), a, b],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace')
            line = (p.stdout.strip().splitlines() or ['no output'])[-1]
            print('ORA2 %s %s %s: %s' % (cfgname, row.name, way, line), flush=True)
            ora[line.split(' ')[0]] += 1
    print('FINAL %s: %d emulations, %d disagree; oracle_design updates %s' % (
        cfgname, done, bad, dict(ora)), flush=True)


if __name__ == '__main__':
    main()
