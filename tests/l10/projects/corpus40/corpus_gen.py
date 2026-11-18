"""The 40-unit corpus of WP-060's review (R2), copied unchanged from the
workspace's `.build/review-incr/corpus2/gen.py` (WP-067: the review's
scripts are read-only, so L10 keeps its own copy).

Files: common.h, t00.cc..t37.cc (small TUs), big.cc (~5,000 lines,
330 functions), main.cc. Markers /*E..*/ are where edits.py applies the
edits e1..e10. main prints a checksum.
"""

NT = 38           # small TUs
NBIG = 540        # functions in big.cc
MID = NBIG // 2   # the middle function (e1/e2/e9/e10)
NO_WIDGET = {5, 13, 21, 29, 37}   # TUs that never touch Widget
SHARED_DEF, SHARED_USERS = 3, (10, 20, 30)   # e7
E8_TU = 7


def header():
    h = ['#pragma once',
         '/*E4*/',
         '#include <string>', '#include <vector>', '#include <stdexcept>',
         '#include <cassert>', '#include <cstdint>', '#include <cstddef>', '',
         'struct Widget {',
         '  int id; double weight; std::string name; /*E6*/',
         '  Widget(int i, std::string n) : id(i), weight(i * 0.5), name(std::move(n)) {}',
         '  int score() const { return id * 3 + (int)name.size(); }',
         '  void bump(int d) { id += d; weight += d * 0.25; }',
         '  std::string label() const { return name + "#" + std::to_string(id); }',
         '};', '',
         'template <class T> struct Bag {',
         '  std::vector<T> items;',
         '  void add(const T &t) { items.push_back(t); }',
         '  std::size_t count() const { return items.size(); }',
         '  const T &at(std::size_t i) const { assert(i < items.size()); return items[i]; }',
         '  template <class F> long fold(F f) const { long s = 0; for (const T &t : items) s += f(t); return s; }',
         '};', '',
         'inline std::uint64_t mix(std::uint64_t h, std::uint64_t v) { return (h ^ v) * 1099511628211ull; }',
         'inline int clampi(int v, int lo, int hi) { return v < lo ? lo : v > hi ? hi : v; }',
         'inline int hdr_line() { return __LINE__; }', '',
         'struct Base {',
         '  virtual ~Base();',
         '  virtual int run(int x) = 0;',
         '  virtual const char *name() const;',
         '};',
         'struct InlineOnly {',
         '  virtual ~InlineOnly() {}',
         '  virtual int f(int x) { return x + 1; }',
         '};', '',
         'struct AppError : std::runtime_error {',
         '  int code;',
         '  AppError(const std::string &m, int c) : std::runtime_error(m), code(c) {}',
         '};', '',
         'int scale(int v, int factor = 3 /*E5*/);',
         'extern thread_local int tls_counter;',
         'std::uint64_t run_big();']
    for n in range(NT):
        h.append('std::uint64_t run_t%02d(); int thrower_%02d(int x);' % (n, n))
    return '\n'.join(h) + '\n'


def small(n):
    nx = (n + 1) % NT
    w = n not in NO_WIDGET
    L = ['#include "common.h"', '#include <cstdio>', '']
    if n == SHARED_DEF:
        L += ['int shared_fn(int x) { return x * 17 + 5; }', '']
    if n in SHARED_USERS:
        L += ['int shared_fn(int x);', '']
    if n == 2:
        L += ['thread_local int tls_counter = 0;',
              'int scale(int v, int factor) { return v * factor; }',
              'Base::~Base() {}',
              'const char *Base::name() const { return "base"; }', '']
    if w:
        L.append('static Widget g_w_%02d(%d, "widget-%02d");' % (n, n, n))
    L += ['static std::string g_s_%02d = std::string("str-") + "%02d";' % (n, n),
          'thread_local int tl_%02d = %d;' % (n, n), '']
    if n % 2 == 0:
        L += ['struct Deriv%02d : Base {' % n,
              '  int k;',
              '  explicit Deriv%02d(int k) : k(k) {}' % n,
              '  int run(int x) override;',
              '  const char *name() const override;',
              '};',
              'int Deriv%02d::run(int x) { return x * k + %d; }' % (n, n),
              'const char *Deriv%02d::name() const { return "deriv-%02d"; }' % (n, n), '']
    else:
        L += ['struct Inl%02d : InlineOnly {' % n,
              '  int f(int x) override { return x * %d + 1; }' % (n + 2),
              '};', '']
    L += ['int thrower_%02d(int x) {' % n,
          '  if (x %% 7 == 3) throw AppError("bad-%02d-" + std::to_string(x), x);' % n,
          '  return x + %d;' % n,
          '}', '',
          'static int catcher_%02d(int x) {' % n,
          '  try {',
          '    return thrower_%02d(x);' % nx,
          '  } catch (const AppError &e) {',
          '    return e.code + (int)std::string(e.what()).size();',
          '  }',
          '}', '']
    if n == E8_TU:
        L.append('/*E8*/')
    L += ['int checked_%02d(int x) {' % n,
          '  assert(x >= 0 && "checked");',
          '  return x + __LINE__;',
          '}', '',
          'int lineb_%02d() { return __builtin_LINE(); }' % n, '',
          'int plain_%02d(int x) {' % n,
          '  int s = 0;',
          '  for (int i = 0; i < x; i++) s += i * %d;' % (n + 3),
          '  return s;',
          '}', '',
          'std::string strs_%02d(int x) {' % n,
          '  std::string r = "alpha-%02d";' % n,
          '  r += "-beta-literal";',
          '  if (x & 1) r += "-odd-branch-text"; else r += "-even-branch-text";',
          '  r += std::to_string(x);',
          '  r += " the quick brown fox %02d";' % n,
          '  return r;',
          '}', '',
          'static std::uint64_t bag_%02d() {' % n,
          '  std::uint64_t h = 0;',
          '  Bag<int> bi;',
          '  for (int i = 0; i < 20; i++) bi.add(i * %d);' % (n + 1),
          '  h = mix(h, bi.fold([](int v) { return v; }));',
          '  h = mix(h, bi.at(3));',
          '  Bag<std::string> bs;',
          '  bs.add(strs_%02d(1)); bs.add(strs_%02d(2));' % (n, n),
          '  h = mix(h, bs.fold([](const std::string &s) { return (long)s.size(); }));']
    if w:
        L += ['  Bag<Widget> bw;',
              '  bw.add(Widget(%d, "bag-w")); bw.add(g_w_%02d);' % (n, n),
              '  h = mix(h, bw.fold([](const Widget &v) { return (long)v.score(); }));',
              '  h = mix(h, bw.at(1).label().size());']
    L += ['  return h;', '}', '',
          'std::uint64_t run_t%02d() {' % n,
          '  std::uint64_t h = %d;' % (n + 1),
          '  tls_counter++; tl_%02d++;' % n,
          '  h = mix(h, (std::uint64_t)tl_%02d);' % n,
          '  h = mix(h, g_s_%02d.size());' % n]
    if w:
        L += ['  Widget lw(%d, "local");' % n,
              '  lw.bump(2);',
              '  h = mix(h, lw.score() + g_w_%02d.score());' % n]
    if n % 2 == 0:
        L += ['  Deriv%02d d(%d);' % (n, n + 1),
              '  Base *b = &d;',
              '  h = mix(h, b->run(5) + std::string(b->name()).size());']
    else:
        L += ['  Inl%02d o;' % n, '  InlineOnly *p = &o;', '  h = mix(h, p->f(4));']
    L += ['  for (int i = 0; i < 10; i++) h = mix(h, catcher_%02d(i));' % n,
          '  h = mix(h, checked_%02d(3) + lineb_%02d() + plain_%02d(10));' % (n, n, n),
          '  h = mix(h, bag_%02d());' % n,
          '  h = mix(h, clampi((int)h, 0, 1000) + hdr_line());']
    if n % 3 == 0:
        L.append('  h = mix(h, scale(%d));' % (n + 1))
    if n in SHARED_USERS:
        L.append('  h = mix(h, shared_fn(%d));' % n)
    L += ['  return h;', '}']
    return '\n'.join(L) + '\n'


def big_fn(i):
    nm = 'big_f%03d' % i
    if i == MID:
        return ['int %s(int x) {' % nm,
                '  const int kMid = 7919; /*E1*/',
                '  int s = x;',
                '  for (int k = 0; k < 8; k++) s = s * 31 + kMid + k;',
                '  /*E2*/',
                '  return s;', '}', '/*E9*/']
    if i == MID + 1:
        return ['int %s(int x) {' % nm,
                '  std::string s = "middle-literal-alpha"; /*E10*/',
                '  s += std::to_string(x);',
                '  return (int)s.size();', '}']
    k = i % 6
    if k == 0:
        b = ['int %s(int x) {' % nm, '  int s = %d;' % (i * 13 + 1),
             '  for (int k = 0; k < (x & 15); k++) {',
             '    s = s * 33 + k;', '    s ^= (s >> 7);', '  }',
             '  if (s & 1) s += %d; else s -= %d;' % (i, i + 1),
             '  return s;', '}']
    elif k == 1:
        b = ['int %s(int x) {' % nm, '  std::string s = "big-literal-%03d-";' % i,
             '  s += std::to_string(x);', '  s += "-tail-%03d";' % i,
             '  if (x > 3) s += " extra words for function %03d";' % i,
             '  return (int)s.size();', '}']
    elif k == 2:
        b = ['int %s(int x) {' % nm, '  Widget w(%d, "bw-%03d");' % (i, i),
             '  w.bump(x);', '  int r = w.score();', '  r += (int)w.label().size();',
             '  return r;', '}']
    elif k == 3:
        b = ['int %s(int x) {' % nm, '  Bag<int> b;',
             '  for (int k = 0; k < 6; k++) b.add(k * %d + x);' % (i % 17 + 1),
             '  long f = b.fold([](int v) { return v * 2; });',
             '  return (int)f + b.at(2);', '}']
    elif k == 4:
        b = ['int %s(int x) {' % nm, '  switch (x %% %d) {' % (i % 5 + 3),
             '  case 0: return %d;' % (i * 3), '  case 1: return x + %d;' % i,
             '  case 2: return x * %d;' % (i % 9 + 2), '  default: return -x;', '  }', '}']
    else:
        b = ['int %s(int x) {' % nm, '  assert(x >= 0 && "big-assert");',
             '  int s = x * 2 + __LINE__;', '  s += __builtin_LINE() & 3;',
             '  return s;', '}']
    return b


def big():
    L = ['#include "common.h"', '#include <cstdio>', '',
         '// big.cc: %d functions' % NBIG, '/*E3*/', '']
    for i in range(NBIG):
        L += big_fn(i)
        L.append('')
    L += ['std::uint64_t run_big() {', '  std::uint64_t h = 7;']
    for i in range(NBIG):
        L.append('  h = mix(h, (std::uint64_t)big_f%03d(%d));' % (i, i % 11))
    L += ['  return h;', '}']
    return '\n'.join(L) + '\n'


def main_cc():
    L = ['#include "common.h"', '#include <cstdio>', '', 'int main() {', '  std::uint64_t h = 1;']
    for n in range(NT):
        L.append('  h = mix(h, run_t%02d());' % n)
    L += ['  h = mix(h, run_big());', '  h = mix(h, (std::uint64_t)tls_counter);',
          '  std::printf("checksum %016llx\\n", (unsigned long long)h);', '  return 0;', '}']
    return '\n'.join(L) + '\n'


def files():
    f = {'common.h': header(), 'big.cc': big(), 'main.cc': main_cc()}
    for n in range(NT):
        f['t%02d.cc' % n] = small(n)
    return f


TUS = ['t%02d' % n for n in range(NT)] + ['big', 'main']
