// The load-time one-definition-rule check (spec/odr.md §10.3, O-53..O-59):
// the runtime mortar links into a module that asks for it (--odr-runtime).
// Built by hod at -O0 from IR of kiln (aarch64, x86-64) or clang (i386):
// runtime/odr/PINS, `cargo xtask odr-runtime`. Its objects are under
// runtime/odr/obj/; this file is their only source. It is written in the
// subset kiln+hod compile at -O0: C++, no exceptions, no RTTI, no static
// constructors, no libstdc++, no stdio.
//
// The mechanism ("read the binding", O-54): the loader has bound every
// reference of a module when the module's first constructor runs (BIND_NOW;
// data references always). For each dynamic relocation of the module whose
// symbol has a row in the module's own table (O-51), the slot's value IS the
// address the reference bound to; the module that contains the address, and
// that module's row whose symbol has exactly that address, are the definition
// that runs. A mismatch is: the two rows' hashes differ. The runtime never
// asks the loader where a name would bind now (no dlsym), keeps no state
// shared between modules and exports no symbol.
//
// Every byte of another module is untrusted (O-54b): each read of a module's
// memory goes through in_mod(), which admits only ranges inside one of the
// module's readable PT_LOAD segments; a table, dynamic section or symbol
// table that is not inside them is named (odr-hook-malformed-table), never
// read. The program headers themselves come from the loader
// (dl_iterate_phdr) and are trusted.
//
// Memory (O-54b): a map from .dynsym index to row, 4 bytes per symbol, on the
// stack up to STACK_SYMS symbols and mmap'd above; the same map of a module
// that references bind into, mmap'd at need. Without mmap, binary searches of
// the tables: slower, and no memory.
//
// Entries (O-54a): .init_array.00000 of every shared object that carries the
// runtime; the executable's flavour (-DMASON_RT_EXE) has a .preinit_array
// entry instead, which checks the initial set before any constructor runs.
//
// Modes (O-56): MASON_ODR=off|report|fatal[,strict] over the default the
// link recorded in the runtime note (NT_MASON_ODR_RT, 0x4d4f4403, in any
// PT_NOTE) of the executable, else of the module itself, else report. Under
// secure execution (AT_SECURE) the environment is not read.

#include <link.h>
#include <elf.h>
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/auxv.h>
#include <time.h>

#define NT_MASON_ODR 0x4d4f4401u
#define NT_MASON_ODR_RT 0x4d4f4403u
#define ROW_SYSTEM 0x0100u

#define MODE_OFF 0u
#define MODE_REPORT 1u
#define MODE_FATAL 2u

#if defined(__LP64__)
#define RSYM(i) ELF64_R_SYM(i)
#define RTYPE(i) ELF64_R_TYPE(i)
#else
#define RSYM(i) ELF32_R_SYM(i)
#define RTYPE(i) ELF32_R_TYPE(i)
#endif

// The relocation numbers the runtime reads. TLS_*: the thread-local
// references (counted, not compared: odr-hook-tls). TARGET_RELA: the target's
// relocation format; DT_PLTREL naming the other one is malformed.
#if defined(__aarch64__)
#define R_ABS 257u
#define R_GLOB 1025u
#define R_SLOT 1026u
#define R_COPYREL 1024u
#define R_TLS_A 1028u
#define R_TLS_B 1029u
#define R_TLS_C 1030u
#define R_TLS_D 1031u
#define R_TLS_E 1031u
#define TARGET_RELA 1
#elif defined(__x86_64__)
#define R_ABS 1u
#define R_GLOB 6u
#define R_SLOT 7u
#define R_COPYREL 5u
#define R_TLS_A 16u
#define R_TLS_B 17u
#define R_TLS_C 18u
#define R_TLS_D 36u
#define R_TLS_E 36u
#define TARGET_RELA 1
#elif defined(__i386__)
#define R_ABS 1u
#define R_GLOB 6u
#define R_SLOT 7u
#define R_COPYREL 5u
#define R_TLS_A 14u
#define R_TLS_B 35u
#define R_TLS_C 36u
#define R_TLS_D 37u
#define R_TLS_E 41u
#define TARGET_RELA 0
#else
#error "no relocation numbers for this target"
#endif

struct Row {
  uint64_t key;
  uint64_t hash;
  uint32_t dynsym;
  uint16_t flags;
  uint16_t reserved;
};

// Why a module's table is not believed (odr-hook-malformed-table).
#define BAD_NOTE 1u
#define BAD_TWICE 2u
#define BAD_NOTE_SEGMENT 3u
#define BAD_DYNAMIC 4u
#define BAD_NSYMS 5u
#define BAD_SYMTAB 6u
#define BAD_ROW_SYM 7u
#define BAD_ORDER 8u
#define BAD_RELOC_TABLE 9u
#define BAD_PLTREL 10u
#define BAD_SLOT 11u
#define BAD_KEY 12u

#define NLOAD 16

struct Mod {
  uintptr_t base;
  const ElfW(Phdr)* phdr;
  uintptr_t lo;  // the hull of the PT_LOAD segments: which module holds an address
  uintptr_t hi;
  uintptr_t rlo[NLOAD];  // the readable PT_LOAD segments: what may be read
  uintptr_t rhi[NLOAD];
  uint32_t nr;  // past NLOAD readable segments, the rest is never read
  const char* name;
  const Row* rows;
  uint32_t count;
  uint32_t malformed;  // 0 or BAD_*
  const ElfW(Dyn)* dyn;
  const ElfW(Sym)* symtab;
  const char* strtab;
  uintptr_t strsz;
  uint32_t nsyms;  // 0: not known
  const uint32_t* gnuhash;
  const uint32_t* hash;
  uintptr_t rela;
  uintptr_t relasz;
  uintptr_t rel;
  uintptr_t relsz;
  uintptr_t jmprel;
  uintptr_t pltrelsz;
  uint32_t pltrela;
  uint32_t hasrt;
  uint32_t rtmode;
  uint32_t rtflags;
  uint32_t main;
  // the map of a module that references bind into (built at need)
  uint32_t* ymap;
  size_t ymapsz;
  uint32_t ynomap;
  uint32_t warned;
};

struct Counts {
  uint32_t relocs;
  uint32_t own;
  uint32_t same;
  uint32_t differ;
  uint32_t interposed;
  uint32_t notable;
  uint32_t norow;
  uint32_t lazy;
  uint32_t unknown;
  uint32_t system;
  uint32_t copy;
  uint32_t plt;
  uint32_t ifunc;
  uint32_t tls;
  uint32_t malformed;
  uint32_t slow;
};

#define NCACHE 16

struct Ctx {
  uint32_t mode;
  uint32_t strict;
  uint32_t stats;
  uint32_t nommap;
  uint32_t timing;
  uint32_t secure;
  uint32_t quiet_lazy;
  const char* preload;
  Mod cache[NCACHE];
  uint32_t ncache;
  uint32_t next;
  uint32_t mismatches;
  uint32_t modules;
};

static void zero_counts(Counts* n) {
  n->relocs = 0;
  n->own = 0;
  n->same = 0;
  n->differ = 0;
  n->interposed = 0;
  n->notable = 0;
  n->norow = 0;
  n->lazy = 0;
  n->unknown = 0;
  n->system = 0;
  n->copy = 0;
  n->plt = 0;
  n->ifunc = 0;
  n->tls = 0;
  n->malformed = 0;
  n->slow = 0;
}

// ---- output without stdio: write(2) of a small buffer ----

struct Out {
  char buf[512];
  uint32_t n;
};

static void out_flush(Out* o) {
  uint32_t done = 0;
  while (done < o->n) {
    long w = (long)write(2, o->buf + done, o->n - done);
    if (w <= 0) break;
    done += (uint32_t)w;
  }
  o->n = 0;
}

static void out_char(Out* o, char c) {
  if (o->n == sizeof o->buf) out_flush(o);
  o->buf[o->n] = c;
  o->n = o->n + 1;
}

static void out_str(Out* o, const char* s) {
  if (!s) s = "(null)";
  while (*s) {
    out_char(o, *s);
    s++;
  }
}

// at most n bytes of s, unprintable bytes as '?'
static void out_strn(Out* o, const char* s, size_t n) {
  for (size_t i = 0; i < n && s[i]; i++) {
    char ch = s[i];
    out_char(o, ch >= 32 && ch < 127 ? ch : '?');
  }
}

// v / 10 without a 64-bit division (no __udivdi3 from libgcc on i386)
static uint64_t div10(uint64_t v, uint32_t* rem) {
  uint64_t q = (v >> 1) + (v >> 2);
  q = q + (q >> 4);
  q = q + (q >> 8);
  q = q + (q >> 16);
  q = q + (q >> 32);
  q = q >> 3;
  uint64_t r = v - q * 10;
  while (r > 9) {
    q = q + 1;
    r = r - 10;
  }
  *rem = (uint32_t)r;
  return q;
}

static void out_dec(Out* o, uint64_t v) {
  char t[24];
  int i = 0;
  do {
    uint32_t rem = 0;
    v = div10(v, &rem);
    t[i] = (char)('0' + (int)rem);
    i++;
  } while (v && i < 24);
  while (i > 0) {
    i--;
    out_char(o, t[i]);
  }
}

static void out_hex(Out* o, uint64_t v) {
  const char* d = "0123456789abcdef";
  for (int s = 60; s >= 0; s -= 4) out_char(o, d[(v >> s) & 15u]);
}

// ---- small helpers (no libc string functions) ----

static int str_eq(const char* a, const char* b) {
  while (*a && *a == *b) {
    a++;
    b++;
  }
  return *a == *b;
}

static int str_eqn(const char* a, const char* b, size_t n) {
  for (size_t i = 0; i < n; i++) {
    if (a[i] != b[i]) return 0;
    if (!a[i]) return 1;
  }
  return 1;
}

static const char* base_name(const char* p) {
  const char* r = p;
  while (*p) {
    if (*p == '/') r = p + 1;
    p++;
  }
  return r;
}

static uintptr_t dptr(uintptr_t base, uintptr_t v) {
  // glibc relocates DT_SYMTAB/DT_STRTAB/DT_REL(A)/DT_JMPREL/DT_*HASH in
  // place for every object it maps (not the vDSO): add the base only when
  // the value is still below it. in_mod() bounds the result either way.
  return v < base ? base + v : v;
}

// ---- reading a module's memory: only inside its readable PT_LOAD segments ----

// [a, a + n) lies inside one readable PT_LOAD segment of m.
static int in_mod(const Mod* m, uintptr_t a, uintptr_t n) {
  for (uint32_t i = 0; i < m->nr; i++)
    if (a >= m->rlo[i] && a <= m->rhi[i] && n <= m->rhi[i] - a) return 1;
  return 0;
}

static const ElfW(Sym)* sym_at(const Mod* m, uint32_t i) {
  if (!m->symtab) return 0;
  if (m->nsyms && i >= m->nsyms) return 0;
  uintptr_t a = (uintptr_t)m->symtab + (uintptr_t)i * sizeof(ElfW(Sym));
  if ((uintptr_t)i > ~(uintptr_t)0 / sizeof(ElfW(Sym))) return 0;
  return in_mod(m, a, sizeof(ElfW(Sym))) ? (const ElfW(Sym)*)a : 0;
}

// The NUL-terminated name at `off` of m's string table, or 0. The table's
// range was checked against the segments when the module was read.
struct Name {
  const char* s;
  uint64_t fnv;    // the key (O-51: FNV-1a 64 of the .dynsym name)
  uint32_t gnu;    // DT_GNU_HASH's hash
  uint32_t elf;    // DT_HASH's hash
};

static int name_at(const Mod* m, uint32_t off, Name* out) {
  if (!m->strtab || off >= m->strsz) return 0;
  const char* s = m->strtab + off;
  uintptr_t left = m->strsz - off;
  uint64_t h = 0xcbf29ce484222325ull;
  uint32_t g = 5381;
  uint32_t e = 0;
  for (uintptr_t i = 0; i < left; i++) {
    unsigned char ch = (unsigned char)s[i];
    if (!ch) {
      out->s = s;
      out->fnv = h;
      out->gnu = g;
      out->elf = e;
      return 1;
    }
    h = (h ^ (uint64_t)ch) * 0x100000001b3ull;
    g = g * 33u + ch;
    e = (e << 4) + ch;
    uint32_t t = e & 0xf0000000u;
    if (t) e = e ^ (t >> 24);
    e = e & ~t;
  }
  return 0;
}

// The symbol `s` of m is named `name` (bounded by the string table).
static int sym_named(const Mod* m, const ElfW(Sym)* s, const char* name) {
  if (!m->strtab || s->st_name >= m->strsz) return 0;
  const char* p = m->strtab + s->st_name;
  uintptr_t left = m->strsz - s->st_name;
  for (uintptr_t i = 0; i < left; i++) {
    if (p[i] != name[i]) return 0;
    if (!name[i]) return 1;
  }
  return 0;
}

static uint32_t read_u32(const Mod* m, const uint32_t* p, int* ok) {
  if (!in_mod(m, (uintptr_t)p, 4)) {
    *ok = 0;
    return 0;
  }
  return *p;
}

// Calls fn for each symbol of m whose name may be `nm` (the hash tables'
// candidates; DT_GNU_HASH first, else DT_HASH), until fn returns 1. Every
// bucket and chain word is read through in_mod().
typedef int (*SymFn)(const Mod* m, uint32_t idx, const ElfW(Sym)* s, void* arg);

static int each_candidate(const Mod* m, const Name* nm, SymFn fn, void* arg) {
  int ok = 1;
  if (m->gnuhash) {
    const uint32_t* h = m->gnuhash;
    uint32_t nb = h[0];
    uint32_t symoff = h[1];
    if (!nb) return 0;
    const uint32_t* buckets = (const uint32_t*)((const char*)(h + 4) + (uintptr_t)h[2] * sizeof(ElfW(Addr)));
    const uint32_t* chain = buckets + nb;
    uint32_t i = read_u32(m, &buckets[nm->gnu % nb], &ok);
    if (!ok || i < symoff) return 0;
    for (;; i++) {
      uint32_t ch = read_u32(m, &chain[i - symoff], &ok);
      if (!ok) return 0;
      if ((ch | 1u) == (nm->gnu | 1u)) {
        const ElfW(Sym)* s = sym_at(m, i);
        if (s && fn(m, i, s, arg)) return 1;
      }
      if (ch & 1u) return 0;
    }
  }
  if (m->hash) {
    uint32_t nb = m->hash[0];
    uint32_t nc = m->hash[1];
    if (!nb) return 0;
    const uint32_t* buckets = m->hash + 2;
    const uint32_t* chain = buckets + nb;
    uint32_t i = read_u32(m, &buckets[nm->elf % nb], &ok);
    for (uint32_t steps = 0; ok && i && i < nc && steps < nc; steps++) {
      const ElfW(Sym)* s = sym_at(m, i);
      if (s && fn(m, i, s, arg)) return 1;
      i = read_u32(m, &chain[i], &ok);
    }
  }
  return 0;
}

// ---- reading one module ----

static void read_notes(Mod* m, const ElfW(Phdr)* p) {
  uintptr_t start = m->base + p->p_vaddr;
  uint64_t size = (uint64_t)p->p_memsz;
  if (!size) return;
  if (!in_mod(m, start, (uintptr_t)p->p_memsz)) {
    // a note the module's segments do not hold: it may be the table
    m->malformed = BAD_NOTE_SEGMENT;
    return;
  }
  const char* seg = (const char*)start;
  uint64_t al = p->p_align > 4 ? 8 : 4;
  uint64_t off = 0;
  while (off + 12 <= size) {
    const uint32_t* h = (const uint32_t*)(seg + off);
    uint32_t namesz = h[0];
    uint32_t descsz = h[1];
    uint32_t type = h[2];
    uint64_t doff = (off + 12 + namesz + al - 1) & ~(al - 1);
    uint64_t dend = doff + descsz;
    if (dend > size) return;
    if (namesz == 10 && str_eqn(seg + off + 12, "MasonMeta", 10)) {
      const uint32_t* d = (const uint32_t*)(seg + doff);
      if (type == NT_MASON_ODR) {
        // O-51: {u32 version = 1; u32 count; u32 row_size = 24; u32 reserved}
        if (m->rows) {
          m->malformed = BAD_TWICE;
        } else if (descsz < 16 || d[0] != 1 || d[2] != 24 || (uint64_t)d[1] * 24 > (uint64_t)(descsz - 16)) {
          m->malformed = BAD_NOTE;
        } else {
          m->rows = (const Row*)(seg + doff + 16);
          m->count = d[1];
        }
      } else if (type == NT_MASON_ODR_RT && descsz >= 16 && d[0] == 1) {
        m->hasrt = 1;
        m->rtmode = d[1];
        m->rtflags = d[2];
      }
    }
    off = (dend + al - 1) & ~(al - 1);
  }
}

static uint32_t gnu_hash_count(const Mod* m, const uint32_t* h) {
  uint32_t nbuckets = h[0];
  uint32_t symoffset = h[1];
  const uint32_t* buckets = (const uint32_t*)((const char*)(h + 4) + (uintptr_t)h[2] * sizeof(ElfW(Addr)));
  const uint32_t* chain = buckets + nbuckets;
  uint32_t max = 0;
  for (uint32_t b = 0; b < nbuckets; b++)
    if (buckets[b] > max) max = buckets[b];
  if (max < symoffset) return symoffset;
  int ok = 1;
  while (!(read_u32(m, &chain[max - symoffset], &ok) & 1u)) {
    if (!ok) return 0;
    max++;
  }
  if (!ok) return 0;
  return max + 1;
}

static void add_range(Mod* m, uintptr_t a, uintptr_t b) {
  for (uint32_t i = 0; i < m->nr; i++) {
    if (a == m->rhi[i]) {
      m->rhi[i] = b;
      return;
    }
    if (b == m->rlo[i]) {
      m->rlo[i] = a;
      return;
    }
  }
  if (m->nr == NLOAD) return;
  m->rlo[m->nr] = a;
  m->rhi[m->nr] = b;
  m->nr = m->nr + 1;
}

static void fill_from(Mod* m, uintptr_t base, const ElfW(Phdr)* ph, int phnum, const char* name, int is_main) {
  m->base = base;
  m->phdr = ph;
  m->lo = ~(uintptr_t)0;
  m->hi = 0;
  m->nr = 0;
  m->name = name;
  m->main = is_main;
  m->rows = 0;
  m->count = 0;
  m->malformed = 0;
  m->dyn = 0;
  m->symtab = 0;
  m->strtab = 0;
  m->strsz = 0;
  m->nsyms = 0;
  m->gnuhash = 0;
  m->hash = 0;
  m->rela = 0;
  m->relasz = 0;
  m->rel = 0;
  m->relsz = 0;
  m->jmprel = 0;
  m->pltrelsz = 0;
  m->pltrela = TARGET_RELA;
  m->hasrt = 0;
  m->rtmode = 0;
  m->rtflags = 0;
  m->ymap = 0;
  m->ymapsz = 0;
  m->ynomap = 0;
  m->warned = 0;
  const ElfW(Phdr)* pdyn = 0;
  for (int i = 0; i < phnum; i++) {
    const ElfW(Phdr)* p = &ph[i];
    if (p->p_type == PT_LOAD) {
      uintptr_t a = m->base + p->p_vaddr;
      if (a < m->lo) m->lo = a;
      if (a + p->p_memsz > m->hi) m->hi = a + p->p_memsz;
      if ((p->p_flags & PF_R) && p->p_memsz && a + p->p_memsz > a) add_range(m, a, a + p->p_memsz);
    } else if (p->p_type == PT_DYNAMIC) {
      pdyn = p;
    }
  }
  for (int i = 0; i < phnum; i++) {
    const ElfW(Phdr)* p = &ph[i];
    if (p->p_type == PT_NOTE) read_notes(m, p);
  }
  if (pdyn) {
    uintptr_t a = m->base + pdyn->p_vaddr;
    uintptr_t n = pdyn->p_memsz / sizeof(ElfW(Dyn));
    int ended = 0;
    if (in_mod(m, a, n * sizeof(ElfW(Dyn)))) {
      const ElfW(Dyn)* d = (const ElfW(Dyn)*)a;
      for (uintptr_t k = 0; k < n; k++) {
        if (d[k].d_tag == DT_NULL) {
          ended = 1;
          break;
        }
      }
    }
    if (ended) m->dyn = (const ElfW(Dyn)*)a;
    else if (m->rows && !m->malformed) m->malformed = BAD_DYNAMIC;
  }
  if (!m->dyn) {
    if (m->malformed) {
      m->rows = 0;
      m->count = 0;
    }
    return;
  }
  uintptr_t symtab = 0, strtab = 0, gnu = 0, hash = 0, syment = sizeof(ElfW(Sym));
  uintptr_t relaent = sizeof(ElfW(Rela)), relent = sizeof(ElfW(Rel));
  uintptr_t pltrel = 0;
  for (const ElfW(Dyn)* d = m->dyn; d->d_tag != DT_NULL; d++) {
    switch (d->d_tag) {
      case DT_SYMTAB: symtab = dptr(m->base, d->d_un.d_ptr); break;
      case DT_STRTAB: strtab = dptr(m->base, d->d_un.d_ptr); break;
      case DT_STRSZ: m->strsz = d->d_un.d_val; break;
      case DT_SYMENT: syment = d->d_un.d_val; break;
      case DT_GNU_HASH: gnu = dptr(m->base, d->d_un.d_ptr); break;
      case DT_HASH: hash = dptr(m->base, d->d_un.d_ptr); break;
      case DT_RELA: m->rela = dptr(m->base, d->d_un.d_ptr); break;
      case DT_RELASZ: m->relasz = d->d_un.d_val; break;
      case DT_RELAENT: relaent = d->d_un.d_val; break;
      case DT_REL: m->rel = dptr(m->base, d->d_un.d_ptr); break;
      case DT_RELSZ: m->relsz = d->d_un.d_val; break;
      case DT_RELENT: relent = d->d_un.d_val; break;
      case DT_JMPREL: m->jmprel = dptr(m->base, d->d_un.d_ptr); break;
      case DT_PLTRELSZ: m->pltrelsz = d->d_un.d_val; break;
      case DT_PLTREL: pltrel = d->d_un.d_val; break;
      default: break;
    }
  }
  // the hash tables: their headers and bucket arrays inside the segments
  if (gnu && in_mod(m, gnu, 16)) {
    const uint32_t* h = (const uint32_t*)gnu;
    uint64_t span = 16 + (uint64_t)h[2] * sizeof(ElfW(Addr)) + (uint64_t)h[0] * 4;
    if (span <= (uint64_t)(uintptr_t)~(uintptr_t)0 && in_mod(m, gnu, (uintptr_t)span)) m->gnuhash = h;
  }
  if (hash && in_mod(m, hash, 8)) {
    const uint32_t* h = (const uint32_t*)hash;
    uint64_t span = 8 + (uint64_t)h[0] * 4 + (uint64_t)h[1] * 4;
    if (span <= (uint64_t)(uintptr_t)~(uintptr_t)0 && in_mod(m, hash, (uintptr_t)span)) {
      m->hash = h;
      m->nsyms = h[1];
    }
  }
  if (!m->nsyms && m->gnuhash) m->nsyms = gnu_hash_count(m, m->gnuhash);
  if (syment == sizeof(ElfW(Sym)) && symtab) m->symtab = (const ElfW(Sym)*)symtab;
  if (strtab && m->strsz && in_mod(m, strtab, m->strsz)) m->strtab = (const char*)strtab;
  // the relocation tables: inside the segments, whole entries of the right size
  int badrel = 0;
  if (m->rela && (relaent != sizeof(ElfW(Rela)) || m->relasz % sizeof(ElfW(Rela)) || !in_mod(m, m->rela, m->relasz))) badrel = 1;
  if (m->rel && (relent != sizeof(ElfW(Rel)) || m->relsz % sizeof(ElfW(Rel)) || !in_mod(m, m->rel, m->relsz))) badrel = 1;
  if (m->jmprel) {
    if (pltrel != DT_RELA && pltrel != DT_REL) badrel = 1;
    m->pltrela = pltrel == DT_RELA;
    uintptr_t ent = m->pltrela ? sizeof(ElfW(Rela)) : sizeof(ElfW(Rel));
    if (m->pltrelsz % ent || !in_mod(m, m->jmprel, m->pltrelsz)) badrel = 1;
  }
  if (badrel) {
    m->rela = 0;
    m->relasz = 0;
    m->rel = 0;
    m->relsz = 0;
    m->jmprel = 0;
    m->pltrelsz = 0;
  }
  if (!m->rows || m->malformed) {
    if (m->malformed) {
      m->rows = 0;
      m->count = 0;
    }
    return;
  }
  // a module with a table: everything the check reads must be there
  if (badrel) m->malformed = BAD_RELOC_TABLE;
  else if (m->jmprel && (int)m->pltrela != TARGET_RELA) m->malformed = BAD_PLTREL;
  else if (!m->nsyms) m->malformed = BAD_NSYMS;
  else if (!m->symtab || !m->strtab ||
           (uint64_t)m->nsyms * sizeof(ElfW(Sym)) > (uint64_t)(uintptr_t)~(uintptr_t)0 ||
           !in_mod(m, (uintptr_t)m->symtab, (uintptr_t)m->nsyms * sizeof(ElfW(Sym))))
    m->malformed = BAD_SYMTAB;
  else if (m->count && !in_mod(m, (uintptr_t)m->rows, (uintptr_t)m->count * sizeof(Row)))
    m->malformed = BAD_NOTE;
  else {
    // every row's dynsym index names a symbol of this module, in key order
    for (uint32_t i = 0; i < m->count; i++) {
      uint32_t s = m->rows[i].dynsym;
      if (s >= m->nsyms || s == 0) {
        m->malformed = BAD_ROW_SYM;
        break;
      }
      if (i && m->rows[i - 1].key > m->rows[i].key) {
        m->malformed = BAD_ORDER;
        break;
      }
    }
  }
  if (m->malformed) {
    m->rows = 0;
    m->count = 0;
  }
}

static void fill(Mod* m, struct dl_phdr_info* info) {
  int is_main = !(info->dlpi_name && info->dlpi_name[0]);
  fill_from(m, info->dlpi_addr, info->dlpi_phdr, info->dlpi_phnum, is_main ? "(main program)" : info->dlpi_name,
            is_main);
}

// The module's name, found by its program headers when it was filled without one.
struct FindName {
  const ElfW(Phdr)* phdr;
  const char* name;
};

static int find_name_cb(struct dl_phdr_info* info, size_t sz, void* data) {
  (void)sz;
  FindName* f = (FindName*)data;
  if (info->dlpi_phdr != f->phdr) return 0;
  f->name = info->dlpi_name && info->dlpi_name[0] ? info->dlpi_name : "(main program)";
  return 1;
}

static const char* mod_name(const Mod* m) {
  if (m->name) return m->name;
  FindName f;
  f.phdr = m->phdr;
  f.name = "(unknown module)";
  dl_iterate_phdr(find_name_cb, &f);
  return f.name;
}

// ---- the map of a module that references bind into ----

#define YMAP_MIN_ROWS 64u

static void drop_ymap(Mod* m) {
  if (m->ymap) munmap(m->ymap, m->ymapsz);
  m->ymap = 0;
  m->ymapsz = 0;
}

static void build_ymap(Ctx* c, Mod* y) {
  y->ynomap = 1;
  if (c->nommap || !y->rows || y->count < YMAP_MIN_ROWS || !y->nsyms) return;
  size_t sz = ((size_t)y->nsyms * 4 + 4095) & ~(size_t)4095;
  void* p = mmap(0, sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (p == MAP_FAILED) return;
  uint32_t* map = (uint32_t*)p;  // zeroed by the kernel
  for (uint32_t i = 0; i < y->count; i++) {
    uint32_t s = y->rows[i].dynsym;
    if (!map[s]) map[s] = i + 1;
  }
  y->ymap = map;
  y->ymapsz = sz;
}

struct FindAddr {
  uintptr_t addr;
  Mod* out;
  int found;
};

static int find_addr_cb(struct dl_phdr_info* info, size_t sz, void* data) {
  (void)sz;
  FindAddr* f = (FindAddr*)data;
  for (int i = 0; i < info->dlpi_phnum; i++) {
    const ElfW(Phdr)* p = &info->dlpi_phdr[i];
    if (p->p_type != PT_LOAD) continue;
    uintptr_t a = info->dlpi_addr + p->p_vaddr;
    if (f->addr >= a && f->addr < a + p->p_memsz) {
      fill(f->out, info);
      f->found = 1;
      return 1;
    }
  }
  return 0;
}

// The module that contains `addr`: the cache, then dl_iterate_phdr.
static Mod* module_of(Ctx* c, uintptr_t addr) {
  for (uint32_t i = 0; i < c->ncache; i++) {
    Mod* m = &c->cache[i];
    if (addr >= m->lo && addr < m->hi) return m;
  }
  Mod* slot = &c->cache[c->next];
  if (c->next < c->ncache) drop_ymap(slot);
  FindAddr f;
  f.addr = addr;
  f.out = slot;
  f.found = 0;
  dl_iterate_phdr(find_addr_cb, &f);
  if (!f.found) {
    // the slot was not filled; when it held a module, it still does, without its map
    return 0;
  }
  c->next = (c->next + 1) % NCACHE;
  if (c->ncache < NCACHE) c->ncache = c->ncache + 1;
  return slot;
}

static void drop_cache(Ctx* c) {
  for (uint32_t i = 0; i < c->ncache; i++) drop_ymap(&c->cache[i]);
  c->ncache = 0;
  c->next = 0;
}

// The first row of `m` for `key` (binary search), or m->count.
static uint32_t first_of_key(const Mod* m, uint64_t key) {
  uint32_t lo = 0;
  uint32_t hi = m->count;
  while (lo < hi) {
    uint32_t mid = lo + (hi - lo) / 2;
    if (m->rows[mid].key < key) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

// The row of `m` for (key, symbol index): the no-memory path.
static const Row* row_by_sym(const Mod* m, uint64_t key, uint32_t sym) {
  for (uint32_t i = first_of_key(m, key); i < m->count && m->rows[i].key == key; i++)
    if (m->rows[i].dynsym == sym) return &m->rows[i];
  return 0;
}

// The row of `m` for `key` whose symbol is defined at `addr`: the no-memory path.
static const Row* row_by_addr(const Mod* m, uint64_t key, uintptr_t addr) {
  for (uint32_t i = first_of_key(m, key); i < m->count && m->rows[i].key == key; i++) {
    const ElfW(Sym)* s = sym_at(m, m->rows[i].dynsym);
    if (s && s->st_shndx != SHN_UNDEF && m->base + s->st_value == addr) return &m->rows[i];
  }
  return 0;
}

struct YFind {
  uint64_t key;
  uintptr_t addr;
  const Row* row;
};

static int yfind_cb(const Mod* m, uint32_t idx, const ElfW(Sym)* s, void* arg) {
  YFind* f = (YFind*)arg;
  if (s->st_shndx == SHN_UNDEF || m->base + s->st_value != f->addr || idx >= m->nsyms) return 0;
  uint32_t r = m->ymap[idx];
  if (r && m->rows[r - 1].key == f->key) {
    f->row = &m->rows[r - 1];
    return 1;
  }
  return 0;
}

// The row of y for `key` whose symbol is defined at `addr`: through y's map
// and its hash table (the name's candidates), else the binary search.
static const Row* row_in(Ctx* c, Mod* y, const Name* nm, uintptr_t addr) {
  if (!y->ymap && !y->ynomap) build_ymap(c, y);
  if (y->ymap && (y->gnuhash || y->hash)) {
    YFind f;
    f.key = nm->fnv;
    f.addr = addr;
    f.row = 0;
    each_candidate(y, nm, yfind_cb, &f);
    return f.row;
  }
  return row_by_addr(y, nm->fnv, addr);
}

// y has a row for `key` whose symbol is an IFUNC.
static int ifunc_row(const Mod* y, uint64_t key) {
  for (uint32_t i = first_of_key(y, key); i < y->count && y->rows[i].key == key; i++) {
    const ElfW(Sym)* s = sym_at(y, y->rows[i].dynsym);
    if (s && s->st_shndx != SHN_UNDEF && (s->st_info & 0xf) == STT_GNU_IFUNC) return 1;
  }
  return 0;
}

static int is_preloaded(const Ctx* c, const Mod* m) {
  const char* p = c->preload;
  if (!p || m->main) return 0;
  const char* mine = base_name(mod_name(m));
  char tok[256];
  while (*p) {
    while (*p == ' ' || *p == ':') p++;
    uint32_t n = 0;
    while (*p && *p != ' ' && *p != ':') {
      if (n < sizeof tok - 1) {
        tok[n] = *p;
        n++;
      }
      p++;
    }
    tok[n] = 0;
    if (n && (str_eq(tok, mod_name(m)) || str_eq(base_name(tok), mine))) return 1;
  }
  return 0;
}

static void report(Ctx* c, const Mod* x, const Mod* y, const char* name, const Row* rx, const Row* ry,
                   int interposed) {
  Out o;
  o.n = 0;
  if (interposed) {
    out_str(&o, "mason: warning: odr-interposed-at-load: ");
  } else {
    out_str(&o, c->mode == MODE_FATAL ? "mason: error: odr-violation-at-load: "
                                      : "mason: warning: odr-violation-at-load: ");
  }
  out_str(&o, name);
  out_str(&o, ": ");
  out_str(&o, mod_name(x));
  out_str(&o, interposed ? " binds the preloaded definition in " : " binds the definition in ");
  out_str(&o, mod_name(y));
  out_str(&o, ", which differs from its own (hash ");
  out_hex(&o, rx->hash);
  out_str(&o, " here, ");
  out_hex(&o, ry->hash);
  out_str(&o, " there)\n");
  out_flush(&o);
}

static const char* bad_reason(uint32_t why) {
  switch (why) {
    case BAD_NOTE: return "its table note does not parse (header, version, row size, or a count past the descriptor)";
    case BAD_TWICE: return "it has two table notes";
    case BAD_NOTE_SEGMENT: return "a PT_NOTE lies outside its loaded segments";
    case BAD_DYNAMIC: return "its dynamic section lies outside its loaded segments or has no end";
    case BAD_NSYMS: return "neither DT_HASH nor DT_GNU_HASH gives its symbol count";
    case BAD_SYMTAB: return "its symbol or string table lies outside its loaded segments";
    case BAD_ROW_SYM: return "a row's symbol index is 0 or past .dynsym";
    case BAD_ORDER: return "its rows are not in key order";
    case BAD_RELOC_TABLE: return "a relocation table lies outside its loaded segments or has entries of the wrong size";
    case BAD_PLTREL: return "DT_PLTREL names the other relocation format than the target's";
    case BAD_SLOT: return "a relocation's slot lies outside its loaded segments";
    case BAD_KEY: return "a row's key is not its symbol's name";
    default: return "its table does not parse";
  }
}

static void malformed_warning(const Mod* x, uint32_t why, const Mod* from) {
  Out o;
  o.n = 0;
  out_str(&o, "mason: warning: odr-hook-malformed-table: ");
  out_str(&o, mod_name(x));
  out_str(&o, ": ");
  out_str(&o, bad_reason(why));
  if (from) {
    out_str(&o, "; its definitions are NOT compared (bound from ");
    out_str(&o, mod_name(from));
    out_str(&o, ")\n");
  } else {
    out_str(&o, "; the module is NOT checked\n");
  }
  out_flush(&o);
}

// Per-symbol state of the module being checked: the row index + 1 in the
// low bits, and two flags.
#define M_ROW 0x3fffffffu
#define M_DONE 0x40000000u
#define M_LAZY 0x80000000u
#define STACK_SYMS 4096u

struct Pass {
  Ctx* c;
  const Mod* x;
  uint32_t* map;  // per dynsym index, or 0: the no-memory path
  uint32_t nmap;
  uint32_t recent[64];  // the no-memory path's dedupe of the symbols already judged
  uint32_t nrecent;
  uint32_t badkey;
  uint32_t badslot;
  Counts n;
};

static int slow_done(Pass* p, uint32_t sym) {
  for (uint32_t i = 0; i < p->nrecent && i < 64; i++)
    if (p->recent[i] == sym) return 1;
  return 0;
}

static void mark_done(Pass* p, uint32_t sym) {
  if (p->map) {
    p->map[sym] = p->map[sym] | M_DONE;
  } else {
    p->recent[p->nrecent % 64] = sym;
    p->nrecent++;
  }
}

// ---- copy relocations and canonical PLT entries ----
// A non-PIE executable that reads a library's data gets an R_*_COPY: the
// loader copies the first definition after the executable (in the initial
// set's load order) into the executable, and every reference, the library's
// own included, binds the copy. A non-PIE executable that takes a library
// function's address without PIC gets a canonical PLT entry: an undefined
// symbol with a value, which every GLOB_DAT of the name binds. Neither has a
// row; each is judged against the definition the loader took, found as the
// loader found it.

static int has_copy_at(const Mod* m, uintptr_t addr) {
  uintptr_t t = m->rela ? m->rela : m->rel;
  uintptr_t sz = m->rela ? m->relasz : m->relsz;
  if (!t) return 0;
  uintptr_t ent = m->rela ? sizeof(ElfW(Rela)) : sizeof(ElfW(Rel));
  for (uintptr_t off = 0; off + ent <= sz; off += ent) {
    const ElfW(Rel)* r = (const ElfW(Rel)*)(t + off);  // r_offset, r_info lead both layouts
    if (RTYPE(r->r_info) == R_COPYREL && m->base + r->r_offset == addr) return 1;
  }
  return 0;
}

struct PltFind {
  const char* name;
  uintptr_t addr;
  int found;
};

static int plt_cb(const Mod* m, uint32_t idx, const ElfW(Sym)* s, void* arg) {
  (void)idx;
  PltFind* f = (PltFind*)arg;
  if (s->st_shndx != SHN_UNDEF || !s->st_value || m->base + s->st_value != f->addr) return 0;
  if (!sym_named(m, s, f->name)) return 0;
  f->found = 1;
  return 1;
}

// m (the executable) has an undefined symbol `name` whose value is `addr`.
static int canonical_plt_at(const Mod* m, const Name* nm, uintptr_t addr) {
  PltFind f;
  f.name = nm->s;
  f.addr = addr;
  f.found = 0;
  each_candidate(m, nm, plt_cb, &f);
  return f.found;
}

struct DefFind {
  const char* name;
  uint32_t n;
  uint32_t idx;
};

static int def_cb(const Mod* m, uint32_t idx, const ElfW(Sym)* s, void* arg) {
  DefFind* f = (DefFind*)arg;
  if (s->st_shndx == SHN_UNDEF || (s->st_info >> 4) == STB_LOCAL) return 0;
  if (!sym_named(m, s, f->name)) return 0;
  f->n++;
  f->idx = idx;
  return 0;  // count every version of the name
}

struct FindSrc {
  const Name* nm;
  Mod* out;
  uint32_t idx;
  uint32_t n;  // 0: not found yet
};

static int find_src_cb(struct dl_phdr_info* info, size_t sz, void* data) {
  (void)sz;
  FindSrc* f = (FindSrc*)data;
  if (!(info->dlpi_name && info->dlpi_name[0])) return 0;  // the executable itself
  fill(f->out, info);
  DefFind d;
  d.name = f->nm->s;
  d.n = 0;
  d.idx = 0;
  each_candidate(f->out, f->nm, def_cb, &d);
  if (!d.n) return 0;
  f->n = d.n;
  f->idx = d.idx;
  return 1;
}

static void judge(Pass* p, uint32_t sym, uint32_t type, uintptr_t value, const Row* rx) {
  const Mod* x = p->x;
  Ctx* c = p->c;
  const ElfW(Sym)* s = &x->symtab[sym];
  uintptr_t own = s->st_shndx != SHN_UNDEF ? x->base + s->st_value : 0;
  if (own && value == own) {
    // bound to its own definition: nothing to compare
    p->n.own++;
    mark_done(p, sym);
    return;
  }
  if (value >= x->lo && value < x->hi) {
    if (own && (s->st_info & 0xf) == STT_GNU_IFUNC) {
      // its own IFUNC: the slot holds the resolver's result, which names no definition
      p->n.ifunc++;
      mark_done(p, sym);
    } else if (type == R_SLOT) {
      // a lazy slot still points at this module's PLT: not bound yet
      if (p->map) p->map[sym] = p->map[sym] | M_LAZY;
      else p->n.lazy++;
    } else {
      p->n.unknown++;
      mark_done(p, sym);
    }
    return;
  }
  Name nm;
  if (!name_at(x, s->st_name, &nm) || nm.fnv != rx->key) {
    p->badkey = 1;
    p->n.unknown++;
    mark_done(p, sym);
    return;
  }
  Mod* y = module_of(c, value);
  const Row* ry = 0;
  Mod src;
  const Mod* judge_y = y;
  int own_src = 0;
  int via = 0;  // 1: the executable's copy, 2: its canonical PLT entry
  if (y && y->main && !y->malformed && !(y->rows && (ry = row_in(c, y, &nm, value)))) {
    if (has_copy_at(y, value)) via = 1;
    else if (canonical_plt_at(y, &nm, value)) via = 2;
  }
  if (via) {
    FindSrc f;
    f.nm = &nm;
    f.out = &src;
    f.idx = 0;
    f.n = 0;
    dl_iterate_phdr(find_src_cb, &f);
    if (via == 1) p->n.copy++;
    else p->n.plt++;
    judge_y = 0;
    ry = 0;
    if (f.n == 1) {  // several versions of the name: which one the loader took is not known here
      judge_y = &src;
      if (src.base == x->base) own_src = 1;
      else if (src.rows) ry = row_by_sym(&src, nm.fnv, f.idx);
    }
  } else if (y && y->rows && !y->main) {
    ry = row_in(c, y, &nm, value);
  }
  if (own_src) {
    p->n.own++;
  } else if (!judge_y) {
    p->n.unknown++;
  } else if (judge_y->malformed) {
    p->n.malformed++;
    if (y && judge_y == y && !y->warned) {
      malformed_warning(y, y->malformed, x);
      y->warned = 1;
    } else if (judge_y != y) {
      malformed_warning(judge_y, judge_y->malformed, x);
    }
  } else if (!judge_y->rows) {
    p->n.notable++;
  } else if (!ry) {
    if (ifunc_row(judge_y, nm.fnv)) p->n.ifunc++;
    else p->n.norow++;
  } else if (((rx->flags | ry->flags) & ROW_SYSTEM) && !c->strict) {
    p->n.system++;
  } else if (rx->hash == ry->hash) {
    p->n.same++;
  } else {
    int pre = is_preloaded(c, judge_y) && !c->strict;
    report(c, x, judge_y, nm.s, rx, ry, pre);
    if (pre) {
      p->n.interposed++;
    } else {
      p->n.differ++;
      c->mismatches++;
    }
  }
  mark_done(p, sym);
}

static int is_tls(uint32_t type) {
  return type == R_TLS_A || type == R_TLS_B || type == R_TLS_C || type == R_TLS_D || type == R_TLS_E;
}

static void walk(Pass* p, uintptr_t table, uintptr_t size, int rela) {
  if (!table || !size) return;
  const Mod* x = p->x;
  uintptr_t ent = rela ? sizeof(ElfW(Rela)) : sizeof(ElfW(Rel));
  for (uintptr_t off = 0; off + ent <= size; off += ent) {
    uintptr_t r_offset;
    uintptr_t r_info;
    intptr_t addend = 0;
    if (rela) {
      const ElfW(Rela)* r = (const ElfW(Rela)*)(table + off);
      r_offset = r->r_offset;
      r_info = r->r_info;
      addend = (intptr_t)r->r_addend;
    } else {
      const ElfW(Rel)* r = (const ElfW(Rel)*)(table + off);
      r_offset = r->r_offset;
      r_info = r->r_info;
    }
    uint32_t sym = (uint32_t)RSYM(r_info);
    uint32_t type = (uint32_t)RTYPE(r_info);
    if (!sym || sym >= x->nsyms) continue;
    int tls = is_tls(type);
    if (!tls && type != R_ABS && type != R_GLOB && type != R_SLOT) continue;
    const Row* rx = 0;
    if (p->map) {
      if (sym >= p->nmap) continue;
      uint32_t st = p->map[sym];
      if (!(st & M_ROW) || (st & M_DONE)) continue;
      rx = &x->rows[(st & M_ROW) - 1];
    } else {
      Name nm;
      if (!name_at(x, x->symtab[sym].st_name, &nm)) continue;
      rx = row_by_sym(x, nm.fnv, sym);
      if (!rx || slow_done(p, sym)) continue;
    }
    if (tls) {
      // thread-local: the slot holds a module id or an offset, not an address (odr-hook-tls)
      p->n.tls++;
      mark_done(p, sym);
      continue;
    }
    p->n.relocs++;
    uintptr_t slot = x->base + r_offset;
    if (!in_mod(x, slot, sizeof(uintptr_t))) {
      p->badslot = 1;
      p->n.unknown++;
      mark_done(p, sym);
      continue;
    }
    uintptr_t value = *(const uintptr_t*)slot;
    if (type != R_SLOT) value = value - (uintptr_t)addend;
    judge(p, sym, type, value, rx);
  }
}

static void lazy_warning(const Mod* x, uint32_t lazy) {
  Out o;
  o.n = 0;
  out_str(&o, "mason: warning: odr-hook-lazy: ");
  out_str(&o, mod_name(x));
  out_str(&o, ": ");
  out_dec(&o, lazy);
  out_str(&o, " reference(s) with rows are bound lazily and NOT checked (link the module with -z now)\n");
  out_flush(&o);
}

static void out_stat(Out* o, const char* label, uint32_t v) {
  out_str(o, label);
  out_dec(o, v);
}

// Checks the references of `x` (any module with a table, this runtime's own
// or another's: every byte read is inside its loaded segments).
static void check_module(Ctx* c, const Mod* x) {
  if (x->malformed) {
    malformed_warning(x, x->malformed, 0);
    return;
  }
  if (!x->rows || !x->dyn) return;
  Pass p;
  p.c = c;
  p.x = x;
  p.nrecent = 0;
  p.badkey = 0;
  p.badslot = 0;
  zero_counts(&p.n);
  uint32_t max = 0;
  for (uint32_t i = 0; i < x->count; i++)
    if (x->rows[i].dynsym > max) max = x->rows[i].dynsym;
  uint32_t stack[STACK_SYMS];
  void* mapped = 0;
  size_t mapsz = 0;
  p.nmap = max + 1;
  if (x->count >= M_ROW) {
    p.map = 0;  // more rows than the map's field holds
    p.n.slow = 1;
  } else if (p.nmap <= STACK_SYMS) {
    p.map = stack;
    for (uint32_t i = 0; i < p.nmap; i++) p.map[i] = 0;
  } else {
    mapsz = ((size_t)p.nmap * 4 + 4095) & ~(size_t)4095;
    void* m = c->nommap ? MAP_FAILED : mmap(0, mapsz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) {
      p.map = 0;
      p.n.slow = 1;
    } else {
      mapped = m;
      p.map = (uint32_t*)m;  // zeroed by the kernel
    }
  }
  if (p.map) {
    for (uint32_t i = 0; i < x->count; i++) {
      uint32_t s = x->rows[i].dynsym;
      if (!(p.map[s] & M_ROW)) p.map[s] = i + 1;
    }
  }
  walk(&p, x->rela, x->relasz, 1);
  walk(&p, x->rel, x->relsz, 0);
  walk(&p, x->jmprel, x->pltrelsz, x->pltrela);
  if (p.map) {
    for (uint32_t i = 0; i < p.nmap; i++)
      if ((p.map[i] & M_LAZY) && !(p.map[i] & M_DONE)) p.n.lazy++;
  }
  if (mapped) munmap(mapped, mapsz);
  if (p.badkey) malformed_warning(x, BAD_KEY, 0);
  if (p.badslot) malformed_warning(x, BAD_SLOT, 0);
  if (p.n.lazy && !c->quiet_lazy) lazy_warning(x, p.n.lazy);
  if (c->stats) {
    Out o;
    o.n = 0;
    out_str(&o, "mason-stats ");
    out_str(&o, mod_name(x));
    out_stat(&o, " rows=", x->count);
    out_stat(&o, " relocs=", p.n.relocs);
    out_stat(&o, " own=", p.n.own);
    out_stat(&o, " same=", p.n.same);
    out_stat(&o, " differ=", p.n.differ);
    out_stat(&o, " interposed=", p.n.interposed);
    out_stat(&o, " no-table=", p.n.notable);
    out_stat(&o, " no-row=", p.n.norow);
    out_stat(&o, " lazy=", p.n.lazy);
    out_stat(&o, " unknown=", p.n.unknown);
    out_stat(&o, " copy=", p.n.copy);
    out_stat(&o, " system=", p.n.system);
    out_stat(&o, " canonical-plt=", p.n.plt);
    out_stat(&o, " ifunc=", p.n.ifunc);
    out_stat(&o, " tls=", p.n.tls);
    out_stat(&o, " malformed=", p.n.malformed);
    out_str(&o, p.n.slow ? " path=slow\n" : "\n");
    out_flush(&o);
  }
  c->modules++;
}

// ---- the entries ----

// This module's own file header: every linker that links the runtime
// (GNU ld, lld, mold, mortar: spec/layout.md L-15) defines it when it is
// referenced. Its program headers give the module without walking the
// loader's list, which would make N modules' entries cost O(N^2).
extern "C" const ElfW(Ehdr) __ehdr_start __attribute__((visibility("hidden")));

static void fill_self(Mod* m) {
  const ElfW(Ehdr)* eh = &__ehdr_start;
  const ElfW(Phdr)* ph = (const ElfW(Phdr)*)((const char*)eh + eh->e_phoff);
  uintptr_t base = (uintptr_t)eh;
  for (int i = 0; i < eh->e_phnum; i++) {
    if (ph[i].p_type == PT_LOAD && ph[i].p_offset == 0) {
      base = (uintptr_t)eh - ph[i].p_vaddr;
      break;
    }
  }
  fill_from(m, base, ph, eh->e_phnum, 0, eh->e_type == ET_EXEC);
}

#if !defined(MASON_RT_EXE)
// The main program: the loader's first entry (glibc).
static int find_main_cb(struct dl_phdr_info* info, size_t sz, void* data) {
  (void)sz;
  Mod* m = (Mod*)data;
  if (!(info->dlpi_name && info->dlpi_name[0])) fill(m, info);
  return 1;
}

static void fill_main(Mod* m) {
  m->hasrt = 0;
  m->rows = 0;
  m->malformed = 0;
  m->ymap = 0;
  dl_iterate_phdr(find_main_cb, m);
}
#endif

// glibc runs .preinit_array before libc's __environ is set: getenv() there
// returns NULL for every name (measured, glibc 2.41). The loader passes
// (argc, argv, envp) to every init function; the preinit entry keeps envp.
static char** mason_envp;

static const char* env_get(const char* name) {
  if (!mason_envp) return getenv(name);
  for (char** e = mason_envp; *e; e++) {
    const char* a = *e;
    const char* b = name;
    while (*b && *a == *b) {
      a++;
      b++;
    }
    if (!*b && *a == '=') return a + 1;
  }
  return 0;
}

static void bad_mode(const char* w, size_t n, uint32_t dflt) {
  Out o;
  o.n = 0;
  out_str(&o, "mason: warning: odr-hook-bad-mode: MASON_ODR: unknown word '");
  out_strn(&o, w, n < 64 ? n : 64);
  out_str(&o, n > 64 ? "...'" : "'");
  out_str(&o, " (the words are off, report, fatal, strict); it is ignored, the mode is ");
  out_str(&o, dflt == MODE_FATAL ? "fatal" : dflt == MODE_OFF ? "off" : "report");
  out_str(&o, " unless another word sets it\n");
  out_flush(&o);
}

// Secure execution (AT_SECURE: a set-user-ID or set-group-ID program, or
// one with file capabilities): the loader ignores LD_* there, and the
// runtime reads none of its environment variables nor LD_PRELOAD; the notes
// decide (O-56). MASON_ODR_TEST_SECURE is the matrix's way in (case SE): it
// can only turn secure execution on.
static uint32_t secure_execution(void) {
  if (getauxval(AT_SECURE)) return 1;
  return env_get("MASON_ODR_TEST_SECURE") != 0;
}

static void parse_env(Ctx* c, uint32_t dflt) {
  c->mode = dflt;
  c->strict = 0;
  c->stats = 0;
  c->nommap = 0;
  c->timing = 0;
  c->preload = 0;
  c->secure = secure_execution();
  if (c->secure) return;
  const char* e = env_get("MASON_ODR");
  if (e) {
    const char* p = e;
    while (*p) {
      const char* w = p;
      while (*p && *p != ',') p++;
      size_t n = (size_t)(p - w);
      if (n == 3 && str_eqn(w, "off", 3)) c->mode = MODE_OFF;
      else if (n == 6 && str_eqn(w, "report", 6)) c->mode = MODE_REPORT;
      else if (n == 5 && str_eqn(w, "fatal", 5)) c->mode = MODE_FATAL;
      else if (n == 6 && str_eqn(w, "strict", 6)) c->strict = 1;
      else if (n) bad_mode(w, n, dflt);
      if (*p == ',') p++;
    }
  }
  c->stats = env_get("MASON_ODR_STATS") != 0;
  c->nommap = env_get("MASON_ODR_TEST_NOMMAP") != 0;
  c->timing = env_get("MASON_ODR_TIME") != 0;
  c->preload = env_get("LD_PRELOAD");
}

static void init_ctx(Ctx* c) {
  c->ncache = 0;
  c->next = 0;
  c->mismatches = 0;
  c->modules = 0;
  c->quiet_lazy = 0;
}

struct Sweep {
  Ctx* c;
  uintptr_t skip;  // the base of a module not to check again (0: none)
  int only_without_rt;
  uintptr_t self;
};

static int sweep_cb(struct dl_phdr_info* info, size_t sz, void* data) {
  (void)sz;
  Sweep* s = (Sweep*)data;
  Mod m;
  fill(&m, info);
  if (!m.rows && !m.malformed) return 0;
  if (s->skip && m.base == s->skip && m.lo <= s->self && s->self < m.hi) return 0;
  if (s->only_without_rt && m.hasrt && !(m.lo <= s->self && s->self < m.hi)) return 0;
  check_module(s->c, &m);
  return 0;
}

static void sweep(Ctx* c, const Mod* skip, int only_without_rt) {
  Sweep s;
  s.c = c;
  s.skip = skip ? skip->base : 0;
  s.only_without_rt = only_without_rt;
  s.self = (uintptr_t)&__ehdr_start;
  dl_iterate_phdr(sweep_cb, &s);
}

static void fatal(Ctx* c) {
  Out o;
  o.n = 0;
  out_str(&o, "mason: fatal: odr-violation-at-load: ");
  out_dec(&o, c->mismatches);
  out_str(&o, " mismatch(es); the process stops (MASON_ODR=report continues)\n");
  out_flush(&o);
  abort();
}

static uint64_t now_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000ull + (uint64_t)t.tv_nsec;
}

static void time_line(const Ctx* c, uint64_t t0) {
  if (!c->timing) return;
  uint64_t t1 = now_ns();
  Out o;
  o.n = 0;
  out_str(&o, "mason-time ");
  out_dec(&o, t1 - t0);
  out_char(&o, '\n');
  out_flush(&o);
}

static void entry_line(const char* what, const char* name) {
  Out o;
  o.n = 0;
  out_str(&o, what);
  out_str(&o, name);
  out_char(&o, '\n');
  out_flush(&o);
}

#if !defined(MASON_RT_EXE)
static void mason_odr_init(int argc, char** argv, char** envp) {
  (void)argc;
  (void)argv;
  (void)envp;
  uint64_t t0 = now_ns();
  Mod self;
  Mod main;
  fill_self(&self);
  fill_main(&main);
  uint32_t dflt = MODE_REPORT;
  if (main.hasrt) dflt = main.rtmode;
  else if (self.hasrt) dflt = self.rtmode;
  Ctx c;
  init_ctx(&c);
  parse_env(&c, dflt);
  if (c.stats) entry_line("mason-entry ", mod_name(&self));
  if (c.mode == MODE_OFF) return;
  check_module(&c, &self);
  if (c.mismatches && c.mode == MODE_FATAL) {
    // report every other mismatch the process has before it stops: every
    // module with a table is mapped and relocated at this point
    c.quiet_lazy = 1;  // the entries that ran before already said so
    sweep(&c, &self, 0);
    drop_cache(&c);
    time_line(&c, t0);
    fatal(&c);
  }
  drop_cache(&c);
  time_line(&c, t0);
}

__attribute__((section(".init_array.00000"), used)) static void (*const mason_odr_init_p)(int, char**, char**) =
    mason_odr_init;
#endif

#if defined(MASON_RT_EXE)
// The executable's entry: before any constructor of any module, every module
// of the initial set is mapped and relocated. In fatal mode all are checked
// here (nothing user-written has run yet); in report mode only those that
// carry no runtime of their own (they check themselves at their init).
static void mason_odr_preinit(int argc, char** argv, char** envp) {
  (void)argc;
  (void)argv;
  mason_envp = envp;
  uint64_t t0 = now_ns();
  Mod self;
  fill_self(&self);
  uint32_t dflt = self.hasrt ? self.rtmode : MODE_REPORT;
  Ctx c;
  init_ctx(&c);
  parse_env(&c, dflt);
  if (c.stats) entry_line("mason-preinit ", mod_name(&self));
  if (c.mode == MODE_OFF) return;
  sweep(&c, 0, c.mode != MODE_FATAL);
  drop_cache(&c);
  time_line(&c, t0);
  if (c.mismatches && c.mode == MODE_FATAL) fatal(&c);
}

__attribute__((section(".preinit_array"), used)) static void (*const mason_odr_preinit_p)(int, char**, char**) =
    mason_odr_preinit;
#endif
