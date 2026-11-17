"""gen200: L10's generated program (spec/incr.md IN-63, WP-067).

200 units (`u000`..`u199`: C++ except every unit k with k % 4 == 1 and
u020-u022, which are C) over three archives, `main.cc` linked directly,
a Fortran unit (`f_blk.f90`, or a C stand-in `f_blk.c` without a
gfortran for the target) and the lazily extracted `u_lazy.cc`:

  main.o [x_extra.o] lib0.a (u000-u066, regular) lib1.a (u067-u133, thin)
         lib2.a (u134-u199, f_blk, u_lazy; regular)

The program prints a transcript that depends on every unit: the
constructor log (`.init_array` order: C++ dynamic initialisers and C
`constructor` functions), the `l10_reg` registry iterated from
`__start_l10_reg` to `__stop_l10_reg` (link order), a running hash over
every unit's `uNNN_run` in groups of 20, a TLS counter every unit bumps,
and a final checksum. C++ units instantiate `L10Acc<N>` and the inline
`l10_shared_inline`/`l10_hdr_line` (COMDAT groups, so keepers exist).

The sources are a pure function of the KNOBS (`sources(knobs, cfg)`); an
edit (edits.py) sets knobs. Knob -> what it changes:

  body.K   0..7   unit K's constant (same size in the code: the values of
                  BODY are 16-bit, odd, not AArch64 logical immediates)
  grow.K   0..4   statements added to uK_run, GROW[level] of them (0, 1, 2,
                  48, 600: step by step past a region, a section's tail
                  and a segment's reserve; and back)
  newfn.K  bool   a new global function uK_extra, called
  lit.K    0..1   uK's string literal made longer
  ctor.K   bool   uK gains a `constructor` function (.init_array grows)
  reg.K    bool   uK gains an `l10_reg` entry (the __start_/__stop_ set grows)
  gvar.K   0..6   a global variable of uK: none; .data int64[4]; .data
                  int64[64]; .bss int64[4]; .bss int64[256]; .rodata
                  const int64[8]; const char *const[2] (.data.rel.ro when
                  PIC, else .rodata)
  flit.K   bool   uK uses a floating constant and a string literal
                  (default True: the edit removes them, the inverse restores)
  rename.K bool   uK's helper function uK_hlp is named uK_hlp_renamed
  static.K bool   uK's helper function is static (else global)
  eh.K     bool   uK_run has a try/catch with a throw (C++ units only)
  tls.K    1..4   uK's TLS array length (only K % 25 == 4)
  keeper   bool   u002 instantiates L10Keep<1> (the COMDAT keeper moves to u006 without)
  gc_call  bool   u010 calls l10_gc_helper, defined in unchanged u011 (dies and
                  revives under --gc-sections -ffunction-sections)
  gc_call2 bool   u012 calls l10_gc_helper2, defined in unchanged u013;
                  default False: no live code calls it at the first link
  decl_hidden bool u041 declares l10_attr_fn hidden (the definer, u040, is
                  demoted) and calls it
  common_size 0..1 u021's second COMMON l10_cbuf is int[4] or int[64]
  common_ref bool u020 reads COMMON l10_counter that only u021 mentions (B-4)
  fcommon_ref bool u022 reads the Fortran COMMON /l10blk/ (B-4, Fortran)
  move_def 0..1   l10_moved defined in u030 or u031
  attr     0..2   l10_attr_fn in u040 strong / weak / hidden
  undef    bool   u050 calls l10_missing (no definition: the link fails)
  dup      bool   u060 defines l10_dup_sym, as u061 does (the link fails)
  extract  bool   u070 calls l10_lazy_fn: u_lazy is extracted from lib2.a
  extra_input bool x_extra.o on the line (a constructor and a registry entry)
  hdr      0..3   comment lines added to l10.h (every C++ unit's l10_hdr_line)
  fbody    0..3   the Fortran unit's constant
  ar_order.A 0..2 lib<A>.a's members rotated by a third
  gc       bool   --gc-sections flipped from the configuration's default
                  (on with -ffunction-sections, off without)
"""

NAME = 'gen200'
NUNITS = 200
ARCHIVES = (('lib0.a', 0, 67, False), ('lib1.a', 67, 134, True), ('lib2.a', 134, 200, False))
GROW = (0, 1, 2, 48, 600)
BODY = (0x1235, 0x2a57, 0x3c91, 0x4d1b, 0x5e63, 0x6f29, 0x70b5, 0x31cd)
TLS_UNITS = tuple(k for k in range(NUNITS) if k % 25 == 4)
C_UNITS = frozenset(k for k in range(NUNITS) if k % 4 == 1) | {20, 21, 22}


def knobs():
    k = {}
    for u in range(NUNITS):
        k['body.%d' % u] = 0
        k['grow.%d' % u] = 0
        k['newfn.%d' % u] = False
        k['lit.%d' % u] = 0
        k['ctor.%d' % u] = False
        k['reg.%d' % u] = False
        k['gvar.%d' % u] = 0
        k['flit.%d' % u] = True
        k['rename.%d' % u] = False
        k['static.%d' % u] = False
        k['eh.%d' % u] = False
    for u in TLS_UNITS:
        k['tls.%d' % u] = 2
    for a in range(len(ARCHIVES)):
        k['ar_order.%d' % a] = 0
    k.update({'keeper': True, 'gc_call': True, 'common_ref': False, 'fcommon_ref': False, 'move_def': 0,
              'attr': 0, 'undef': False, 'dup': False, 'extract': False, 'extra_input': False, 'hdr': 0,
              'fbody': 0, 'gc': False, 'gc_call2': False, 'decl_hidden': False, 'common_size': 0})
    return k


def lang(u):
    return 'c' if u in C_UNITS else 'c++'


def uname(u):
    return 'u%03d' % u


HEADER = r'''#pragma once
/* l10.h: the generated program's header (tests/l10/projects/gen200) */
/*HDR*/
#include <stdint.h>
#ifdef __cplusplus
#define EXTC extern "C"
#define EXTV extern "C"
#else
#define EXTC
#define EXTV extern
#endif
struct l10_reg { int id; uint64_t (*fn)(uint64_t); };
#define L10_REG __attribute__((section("l10_reg"), used, aligned(sizeof(void *))))
EXTC void l10_log_ctor(int id);
EXTC uint64_t l10_hash_str(const char *s);
EXTV __thread int l10_tls_hits;
static inline uint64_t l10_mixs(uint64_t h, uint64_t v) { return (h ^ v) * 1099511628211ull; }
#ifdef __cplusplus
template <int N> struct L10Acc {
  static uint64_t step(uint64_t h) { return l10_mixs(h, N * 7 + 1); }
};
template <int N> struct L10Keep {
  static uint64_t step(uint64_t h) { return l10_mixs(h, N * 13 + 5); }
};
inline uint64_t l10_shared_inline(uint64_t h) { return l10_mixs(h, 77); }
inline uint64_t l10_hdr_line() { return __LINE__; }
#endif
'''


def header(k):
    return HEADER.replace('/*HDR*/', '/*HDR*/' + ''.join('\n/* header edit %d */' % i for i in range(k['hdr'])))


def unit_src(u, k):
    n = uname(u)
    c = lang(u) == 'c'
    L = ['/* %s: generated (tests/l10/projects/gen200) */' % n, '#include "l10.h"']
    if u == 1:
        L.append('__thread int l10_tls_hits;')
    if u == 21:
        L.append('int l10_counter;   /* a tentative definition: COMMON with -fcommon */')
        L.append('int l10_cbuf[%d];   /* a second COMMON, its size an edit */' % (4, 64)[k['common_size']])
    gv = k['gvar.%d' % u]
    if gv in (1, 2):
        L.append('int64_t %s_gv[%d] = { %d, 1, 2, 3 };' % (n, (4, 64)[gv - 1], u))
    elif gv in (3, 4):
        L.append('int64_t %s_gv[%d];' % (n, (4, 256)[gv - 3]))
    elif gv == 5:
        L.append('extern const int64_t %s_gv[8];' % n)
        L.append('const int64_t %s_gv[8] = { %d, 5, 6, 7, 8, 9, 10, 11 };' % (n, u))
    elif gv == 6:
        L.append('extern const char *const %s_gv[2];' % n)
        L.append('const char *const %s_gv[2] = { "gv-%s-a", "gv-%s-b" };' % (n, n, n))
    L.append('%suint64_t %s_hlp%s(uint64_t h) { return l10_mixs(h, %du); }' % (
        'static ' if k['static.%d' % u] else '', n, '_renamed' if k['rename.%d' % u] else '', u * 11 + 3))
    if k.get('newfn.%d' % u):
        L.append('EXTC uint64_t %s_extra(uint64_t h) { return l10_mixs(h, %du * 3u + 5u); }' % (n, u))
    if u in TLS_UNITS:
        L.append('static __thread uint64_t %s_tls[%d];' % (n, k['tls.%d' % u]))
    if u % 10 == 3:
        if c:
            L.append('__attribute__((constructor)) static void %s_pc(void) { l10_log_ctor(%d); }' % (n, 1000 + u))
        else:
            L.append('static int %s_init = (l10_log_ctor(%d), 1);' % (n, 1000 + u))
    if k.get('ctor.%d' % u):
        L.append('__attribute__((constructor)) static void %s_ctor(void) { l10_log_ctor(%d); }' % (n, 2000 + u))
    regs = ([u] if u % 10 == 5 else []) + ([3000 + u] if k.get('reg.%d' % u) else [])
    for i in regs:
        L.append('static uint64_t %s_regfn%d(uint64_t h) { return l10_mixs(h, %d); }' % (n, i, i * 3 + 1))
        L.append('L10_REG static const struct l10_reg %s_reg%d = { %d, %s_regfn%d };' % (n, i, i, n, i))
    if u == 11:
        L.append('EXTC uint64_t l10_gc_helper(uint64_t h) { return l10_mixs(h, 4242); }')
    if u == 13:
        L.append('EXTC uint64_t l10_gc_helper2(uint64_t h) { return l10_mixs(h, 4343); }')
    if u == 12 and k['gc_call2']:
        L.append('EXTC uint64_t l10_gc_helper2(uint64_t h);')
    if u == 41 and k['decl_hidden']:
        L.append('__attribute__((visibility("hidden"))) EXTC uint64_t l10_attr_fn(uint64_t h);')
    if u == 10 and k['gc_call']:
        L.append('EXTC uint64_t l10_gc_helper(uint64_t h);')
    if u == 20 and k['common_ref']:
        L.append('extern int l10_counter;')
    if u == 22 and k['fcommon_ref']:
        L.append('extern int64_t l10blk_[4];   /* the Fortran COMMON /l10blk/ */')
    if (u == 30 and k['move_def'] == 0) or (u == 31 and k['move_def'] == 1):
        L.append('EXTC uint64_t l10_moved(uint64_t h) { return l10_mixs(h, 30303); }')
    if u == 40:
        attr = ('', '__attribute__((weak)) ', '__attribute__((visibility("hidden"))) ')[k['attr']]
        L.append('%sEXTC uint64_t l10_attr_fn(uint64_t h) { return l10_mixs(h, 40404); }' % attr)
    if u == 50 and k['undef']:
        L.append('EXTC uint64_t l10_missing(uint64_t h);')
    if u == 61 or (u == 60 and k['dup']):
        L.append('EXTC uint64_t l10_dup_sym(uint64_t h) { return l10_mixs(h, %d); }' % (6100 + u))
    if u == 70 and k['extract']:
        L.append('EXTC uint64_t l10_lazy_fn(uint64_t h);')
    L.append('EXTC uint64_t %s_run(uint64_t h) {' % n)
    L.append('  h = l10_mixs(h, %#x);' % BODY[k['body.%d' % u]])
    for j in range(GROW[k['grow.%d' % u]]):
        L.append('  h = l10_mixs(h, h >> %d);' % (j % 61 + 3))
    L.append('  h = %s_hlp%s(h);' % (n, '_renamed' if k['rename.%d' % u] else ''))
    if gv in (1, 2, 3, 4, 5):
        L.append('  h = l10_mixs(h, (uint64_t)%s_gv[%d]);' % (n, gv % 3 if gv != 5 else u % 8))
    elif gv == 6:
        L.append('  h = l10_mixs(h, l10_hash_str(%s_gv[h & 1]));' % n)
    if k['flit.%d' % u]:
        L.append('  h = l10_mixs(h, (uint64_t)(%d.25 * (double)(h & 0xff))); h = l10_mixs(h, l10_hash_str("flit-%s"));'
                 % (u + 1, n))
    if k['eh.%d' % u] and not c:
        L.append('  try { if ((h & 0xffff) == 0x1dea) throw (int)(h & 7); } catch (int e) { h ^= (uint64_t)e; }')
    if k.get('newfn.%d' % u):
        L.append('  h = %s_extra(h);' % n)
    lit = 'lit-%s' % n + ('-made-longer-by-an-edit' if k['lit.%d' % u] else '')
    L.append('  h = l10_mixs(h, l10_hash_str("%s"));' % lit)
    if u % 7 == 0:
        L.append('  h = l10_mixs(h, __LINE__);')
    if not c:
        L.append('  h = L10Acc<%d>::step(h);' % (u % 8))
        L.append('  h = l10_shared_inline(h);')
        L.append('  h = l10_mixs(h, l10_hdr_line());')
        if (u == 2 and k['keeper']) or u in (6, 10):
            L.append('  h = L10Keep<1>::step(h);')
    if u in TLS_UNITS:
        t = k['tls.%d' % u]
        L.append('  %s_tls[%d] += h; h = l10_mixs(h, %s_tls[0] + %d);' % (n, t - 1, n, t))
    if u % 10 == 3 and not c:
        L.append('  h = l10_mixs(h, (uint64_t)%s_init);' % n)
    L.append('  l10_tls_hits++;')
    if u == 21:
        L.append('  l10_counter += 1; h = l10_mixs(h, (uint64_t)l10_counter);')
    if u == 10 and k['gc_call']:
        L.append('  h = l10_gc_helper(h);')
    if u == 12 and k['gc_call2']:
        L.append('  h = l10_gc_helper2(h);')
    if u == 41 and k['decl_hidden']:
        L.append('  h = l10_attr_fn(h);')
    if u == 21:
        i = (3, 63)[k['common_size']]
        L.append('  l10_cbuf[%d] += 1; h = l10_mixs(h, (uint64_t)l10_cbuf[%d]);' % (i, i))
    if u == 20 and k['common_ref']:
        L.append('  h = l10_mixs(h, (uint64_t)l10_counter);')
    if u == 22 and k['fcommon_ref']:
        L.append('  h = l10_mixs(h, (uint64_t)l10blk_[0]);')
    if u == 50 and k['undef']:
        L.append('  h = l10_missing(h);')
    if u == 61:
        L.append('  h = l10_dup_sym(h);')
    if u == 70 and k['extract']:
        L.append('  h = l10_lazy_fn(h);')
    L += ['  return h;', '}', '']
    return '\n'.join(L)


def main_src(k, fortran):
    L = ['/* main.cc: generated (tests/l10/projects/gen200) */',
         '#include "l10.h"', '#include <stdio.h>', '']
    for u in range(NUNITS):
        L.append('EXTC uint64_t %s_run(uint64_t h);' % uname(u))
    L += ['EXTC uint64_t l10_moved(uint64_t h);', 'EXTC uint64_t l10_attr_fn(uint64_t h);',
          'EXTC void l10f_run(int64_t *h);',
          'extern "C" const struct l10_reg __start_l10_reg[], __stop_l10_reg[];',
          'static int ctor_log[512];', 'static int ctor_n;',
          'EXTC void l10_log_ctor(int id) { if (ctor_n < 512) ctor_log[ctor_n++] = id; }',
          'EXTC uint64_t l10_hash_str(const char *s) {',
          '  uint64_t h = 14695981039346656037ull;',
          '  while (*s) h = (h ^ (unsigned char)*s++) * 1099511628211ull;',
          '  return h;', '}',
          'int main() {',
          '  printf("ctors %d:", ctor_n);',
          '  for (int i = 0; i < ctor_n; i++) printf(" %d", ctor_log[i]);',
          '  printf("\\n");',
          '  uint64_t h = 1;']
    for u in range(NUNITS):
        L.append('  h = %s_run(h);' % uname(u))
        if u % 20 == 19:
            L.append('  printf("units %d-%d %%016llx\\n", (unsigned long long)h);' % (u - 19, u))
    L += ['  printf("registry");',
          '  for (const struct l10_reg *p = __start_l10_reg; p < __stop_l10_reg; p++) {',
          '    printf(" %d", p->id);', '    h = p->fn(h);', '  }',
          '  printf("\\n");',
          '  h = l10_moved(h);', '  h = l10_attr_fn(h);',
          '  { int64_t f = (int64_t)(h & 0xffff); l10f_run(&f); h = l10_mixs(h, (uint64_t)f); }',
          '  printf("tls %d\\n", l10_tls_hits);',
          '  printf("checksum %016llx\\n", (unsigned long long)h);',
          '  return 0;', '}', '']
    return '\n'.join(L)


FBODY = (11, 13, 17, 19)


def fortran_src(k):
    return '\n'.join([
        '! f_blk.f90: generated (tests/l10/projects/gen200); /l10blk/ is a COMMON block',
        'subroutine l10f_run(h) bind(C, name="l10f_run")',
        '  use iso_c_binding',
        '  implicit none',
        '  integer(c_int64_t), intent(inout) :: h',
        '  integer(c_int64_t) :: v(4)',
        '  common /l10blk/ v',
        '  v(1) = v(1) + %d_c_int64_t' % FBODY[k['fbody']],
        '  h = ieor(h, ishftc(v(1), 7))',
        'end subroutine l10f_run', ''])


def fortran_c_src(k):
    return '\n'.join([
        '/* f_blk.c: the C stand-in for f_blk.f90 (no gfortran for the target) */',
        '#include <stdint.h>',
        'int64_t l10blk_[4];   /* COMMON with -fcommon, as gfortran emits /l10blk/ */',
        'static int64_t rot(int64_t v) { return (int64_t)(((uint64_t)v << 7) | ((uint64_t)v >> 57)); }',
        'void l10f_run(int64_t *h) { l10blk_[0] += %d; *h ^= rot(l10blk_[0]); }' % FBODY[k['fbody']], ''])


LAZY = '''/* u_lazy.cc: extracted from lib2.a only when u070 calls l10_lazy_fn */
#include "l10.h"
__attribute__((constructor)) static void lazy_ctor(void) { l10_log_ctor(9001); }
static uint64_t lazy_reg(uint64_t h) { return l10_mixs(h, 9001); }
L10_REG static const struct l10_reg lazy_ent = { 9001, lazy_reg };
EXTC uint64_t l10_lazy_fn(uint64_t h) { return l10_mixs(h, 9009); }
'''

EXTRA = '''/* x_extra.cc: an input added to and removed from the link line */
#include "l10.h"
__attribute__((constructor)) static void extra_ctor(void) { l10_log_ctor(9002); }
static uint64_t extra_reg(uint64_t h) { return l10_mixs(h, 9002); }
L10_REG static const struct l10_reg extra_ent = { 9002, extra_reg };
'''


def sources(k, cfg):
    """{unit name: (file name, language, text)} and the headers {name: text}."""
    units = {uname(u): ('%s.%s' % (uname(u), 'c' if lang(u) == 'c' else 'cc'), lang(u), unit_src(u, k))
             for u in range(NUNITS)}
    units['main'] = ('main.cc', 'c++', main_src(k, cfg.fortran))
    if cfg.fortran:
        units['f_blk'] = ('f_blk.f90', 'fortran', fortran_src(k))
    else:
        units['f_blk'] = ('f_blk.c', 'c', fortran_c_src(k))
    units['u_lazy'] = ('u_lazy.cc', 'c++', LAZY)
    units['x_extra'] = ('x_extra.cc', 'c++', EXTRA)
    return units, {'l10.h': header(k)}


def unit_flags(name):
    """Per-unit compiler flags beyond the configuration's."""
    if name in ('u021', 'f_blk'):
        return ['-fcommon']
    return []


def plan(k, cfg):
    """The link: inputs in order ('o', unit) or ('a', archive, [units], thin),
    and the linker options."""
    inputs = [('o', 'main')]
    if k['extra_input']:
        inputs.append(('o', 'x_extra'))
    for a, (name, lo, hi, thin) in enumerate(ARCHIVES):
        members = [uname(u) for u in range(lo, hi)]
        if name == 'lib2.a':
            members += ['f_blk', 'u_lazy']
        r = k['ar_order.%d' % a] * len(members) // 3
        members = members[r:] + members[:r]
        inputs.append(('a', name, members, thin))
    return inputs, (['--gc-sections'] if k['gc'] != cfg.fs else [])


def units_needed(k, cfg):
    names = ['main', 'x_extra'] if k['extra_input'] else ['main']
    for _n, lo, hi, _t in ARCHIVES:
        names += [uname(u) for u in range(lo, hi)]
    return names + ['f_blk', 'u_lazy']
