// WP-077: prototype 3 of the load-time ODR check (spec/odr.md §10.3).
// Test code, not shipped: the runtime mortar would link into a module that
// asks for it. It is written in the subset kiln+hod compile at -O0 (C++, no
// exceptions, no RTTI, no static constructors, no libstdc++, no stdio).
//
// The mechanism ("read the binding"): the loader has already bound every
// reference of a module when the module's first constructor runs (BIND_NOW;
// data references always). For each dynamic relocation of the module whose
// symbol has a row in the module's own table (spec/odr.md O-51), the slot's
// value IS the address the reference bound to; the module that contains the
// address, and that module's row whose symbol has exactly that address, are
// the definition that runs. A mismatch is: the two rows' hashes differ. The
// runtime never asks the loader where a name would bind now (no dlsym), keeps
// no state shared between modules (no exported symbol), and allocates no
// memory unless a module has more than 16,384 dynamic symbols (then one mmap,
// and without it a slower path that needs none).
//
// Entries: .init_array.00000 of every shared object that carries the
// runtime; an executable's copy (built with -DMASON_RT_EXE) has a
// .preinit_array entry instead, which checks the initial set before any
// constructor of any module runs.
//
// Modes: MASON_ODR=off|report|fatal[,strict] (environment) over the default
// the link recorded in the runtime note (NT_MASON_ODR_RT, 0x4d4f4403) of the
// executable, else of the module itself, else report.

#include <link.h>
#include <elf.h>
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/mman.h>
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

#if defined(__aarch64__)
#define R_ABS 257u
#define R_GLOB 1025u
#define R_SLOT 1026u
#define R_COPYREL 1024u
#elif defined(__x86_64__)
#define R_ABS 1u
#define R_GLOB 6u
#define R_SLOT 7u
#define R_COPYREL 5u
#elif defined(__i386__)
#define R_ABS 1u
#define R_GLOB 6u
#define R_SLOT 7u
#define R_COPYREL 5u
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

struct Mod {
  uintptr_t base;
  const ElfW(Phdr)* phdr;
  uintptr_t lo;
  uintptr_t hi;
  const char* name;
  const Row* rows;
  uint32_t count;
  uint32_t malformed;
  const ElfW(Dyn)* dyn;
  const ElfW(Sym)* symtab;
  const char* strtab;
  uint32_t nsyms;
  const uint32_t* gnuhash;
  uint32_t hasrt;
  uint32_t rtmode;
  uint32_t rtflags;
  uint32_t main;
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
  uint32_t slow;
};

#define NCACHE 16

struct Ctx {
  uint32_t mode;
  uint32_t strict;
  uint32_t stats;
  uint32_t nommap;
  uint32_t quiet_lazy;
  const char* preload;
  Mod cache[NCACHE];
  uint32_t ncache;
  uint32_t next;
  uint32_t mismatches;
  uint32_t modules;
  Counts total;
};

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

static uint64_t fnv(const char* s) {
  uint64_t h = 0xcbf29ce484222325ull;
  while (*s) {
    h = (h ^ (uint64_t)(unsigned char)*s) * 0x100000001b3ull;
    s++;
  }
  return h;
}

static uintptr_t dptr(uintptr_t base, uintptr_t v) {
  // glibc relocates DT_SYMTAB/DT_STRTAB/DT_REL(A)/DT_JMPREL/DT_*HASH in
  // place for every object it maps (not the vDSO): add the base only when
  // the value is still below it.
  return v < base ? base + v : v;
}

// ---- reading one module ----

static void read_notes(Mod* m, const ElfW(Phdr)* p) {
  const char* n = (const char*)(m->base + p->p_vaddr);
  const char* e = n + p->p_memsz;
  uintptr_t al = p->p_align > 4 ? 8 : 4;
  while (n + 12 <= e) {
    const uint32_t* h = (const uint32_t*)n;
    uint32_t namesz = h[0];
    uint32_t descsz = h[1];
    uint32_t type = h[2];
    uintptr_t doff = (12 + namesz + al - 1) & ~(al - 1);
    const char* desc = n + doff;
    if (desc + descsz > e || desc < n) return;
    if (namesz == 10 && str_eqn(n + 12, "MasonMeta", 10)) {
      const uint32_t* d = (const uint32_t*)desc;
      if (type == NT_MASON_ODR) {
        // O-51: {u32 version = 1; u32 count; u32 row_size = 24; u32 reserved}
        if (m->rows || descsz < 16 || d[0] != 1 || d[2] != 24 ||
            (uint64_t)d[1] * 24 > (uint64_t)(descsz - 16)) {
          m->malformed = 1;
        } else {
          m->rows = (const Row*)(desc + 16);
          m->count = d[1];
        }
      } else if (type == NT_MASON_ODR_RT && descsz >= 16 && d[0] == 1) {
        m->hasrt = 1;
        m->rtmode = d[1];
        m->rtflags = d[2];
      }
    }
    uintptr_t next = (doff + descsz + al - 1) & ~(al - 1);
    n = n + next;
  }
}

static uint32_t gnu_hash_count(const uint32_t* h) {
  uint32_t nbuckets = h[0];
  uint32_t symoffset = h[1];
  uint32_t bloom = h[2];
  const uint32_t* buckets = (const uint32_t*)((const char*)(h + 4) + bloom * sizeof(ElfW(Addr)));
  const uint32_t* chain = buckets + nbuckets;
  uint32_t max = 0;
  for (uint32_t b = 0; b < nbuckets; b++)
    if (buckets[b] > max) max = buckets[b];
  if (max < symoffset) return symoffset;
  while (!(chain[max - symoffset] & 1u)) max++;
  return max + 1;
}

static void fill_from(Mod* m, uintptr_t base, const ElfW(Phdr)* ph, int phnum, const char* name, int is_main) {
  m->base = base;
  m->phdr = ph;
  m->lo = ~(uintptr_t)0;
  m->hi = 0;
  m->name = name;
  m->main = is_main;
  m->rows = 0;
  m->count = 0;
  m->malformed = 0;
  m->dyn = 0;
  m->symtab = 0;
  m->strtab = 0;
  m->nsyms = 0;
  m->gnuhash = 0;
  m->hasrt = 0;
  m->rtmode = 0;
  m->rtflags = 0;
  for (int i = 0; i < phnum; i++) {
    const ElfW(Phdr)* p = &ph[i];
    if (p->p_type == PT_LOAD) {
      uintptr_t a = m->base + p->p_vaddr;
      if (a < m->lo) m->lo = a;
      if (a + p->p_memsz > m->hi) m->hi = a + p->p_memsz;
    } else if (p->p_type == PT_DYNAMIC) {
      m->dyn = (const ElfW(Dyn)*)(m->base + p->p_vaddr);
    }
  }
  for (int i = 0; i < phnum; i++) {
    const ElfW(Phdr)* p = &ph[i];
    if (p->p_type == PT_NOTE) read_notes(m, p);
  }
  if (!m->dyn) return;
  const uint32_t* gnuhash = 0;
  const uint32_t* hash = 0;
  for (const ElfW(Dyn)* d = m->dyn; d->d_tag != DT_NULL; d++) {
    if (d->d_tag == DT_SYMTAB) m->symtab = (const ElfW(Sym)*)dptr(m->base, d->d_un.d_ptr);
    if (d->d_tag == DT_STRTAB) m->strtab = (const char*)dptr(m->base, d->d_un.d_ptr);
    if (d->d_tag == DT_GNU_HASH) gnuhash = (const uint32_t*)dptr(m->base, d->d_un.d_ptr);
    if (d->d_tag == DT_HASH) hash = (const uint32_t*)dptr(m->base, d->d_un.d_ptr);
  }
  m->gnuhash = gnuhash;
  if (hash) m->nsyms = hash[1];
  else if (gnuhash) m->nsyms = gnu_hash_count(gnuhash);
  if (m->rows && (!m->symtab || !m->strtab)) m->malformed = 1;
  // every row's dynsym index must name a defined symbol of this module
  if (m->rows && !m->malformed) {
    for (uint32_t i = 0; i < m->count; i++) {
      uint32_t s = m->rows[i].dynsym;
      if ((m->nsyms && s >= m->nsyms) || s == 0 || (i && m->rows[i - 1].key > m->rows[i].key)) {
        m->malformed = 1;
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
static const Mod* module_of(Ctx* c, uintptr_t addr) {
  for (uint32_t i = 0; i < c->ncache; i++) {
    const Mod* m = &c->cache[i];
    if (addr >= m->lo && addr < m->hi) return m;
  }
  Mod* slot = &c->cache[c->next];
  FindAddr f;
  f.addr = addr;
  f.out = slot;
  f.found = 0;
  dl_iterate_phdr(find_addr_cb, &f);
  if (!f.found) return 0;
  c->next = (c->next + 1) % NCACHE;
  if (c->ncache < NCACHE) c->ncache = c->ncache + 1;
  return slot;
}

// The row of `m` for (key, symbol index).
static const Row* row_by_sym(const Mod* m, uint64_t key, uint32_t sym) {
  uint32_t lo = 0;
  uint32_t hi = m->count;
  while (lo < hi) {
    uint32_t mid = lo + (hi - lo) / 2;
    if (m->rows[mid].key < key) lo = mid + 1;
    else hi = mid;
  }
  for (uint32_t i = lo; i < m->count && m->rows[i].key == key; i++)
    if (m->rows[i].dynsym == sym) return &m->rows[i];
  return 0;
}

// The row of `m` for `key` whose symbol is defined at `addr`.
static const Row* row_by_addr(const Mod* m, uint64_t key, uintptr_t addr) {
  uint32_t lo = 0;
  uint32_t hi = m->count;
  while (lo < hi) {
    uint32_t mid = lo + (hi - lo) / 2;
    if (m->rows[mid].key < key) lo = mid + 1;
    else hi = mid;
  }
  for (uint32_t i = lo; i < m->count && m->rows[i].key == key; i++) {
    const ElfW(Sym)* s = &m->symtab[m->rows[i].dynsym];
    if (s->st_shndx != SHN_UNDEF && m->base + s->st_value == addr) return &m->rows[i];
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

#define F_ROW 1u
#define F_DONE 2u
#define F_LAZY 4u
#define STACK_SYMS 16384u

struct Pass {
  Ctx* c;
  const Mod* x;
  unsigned char* flags;  // per dynsym index, or 0: the slow path
  uint32_t nflags;
  uint32_t recent[64];  // the slow path's dedupe of the symbols already judged
  uint32_t nrecent;
  Counts n;
};

static int slow_done(Pass* p, uint32_t sym) {
  for (uint32_t i = 0; i < p->nrecent && i < 64; i++)
    if (p->recent[i] == sym) return 1;
  return 0;
}

// ---- copy relocations ----
// A non-PIE executable that reads a library's data gets an R_*_COPY: the
// loader copies the first definition after the executable (in the initial
// set's load order) into the executable, and every reference, the
// library's own included, binds the copy. The copy has no row; its source
// is found the way the loader found it.

static int has_copy_at(const Mod* m, uintptr_t addr) {
  if (!m->dyn) return 0;
  uintptr_t t = 0;
  uintptr_t sz = 0;
  int rela = 0;
  for (const ElfW(Dyn)* d = m->dyn; d->d_tag != DT_NULL; d++) {
    if (d->d_tag == DT_RELA) {
      t = dptr(m->base, d->d_un.d_ptr);
      rela = 1;
    }
    if (d->d_tag == DT_RELASZ) sz = d->d_un.d_val;
    if (d->d_tag == DT_REL) t = dptr(m->base, d->d_un.d_ptr);
    if (d->d_tag == DT_RELSZ) sz = d->d_un.d_val;
  }
  if (!t) return 0;
  uintptr_t ent = rela ? sizeof(ElfW(Rela)) : sizeof(ElfW(Rel));
  for (uintptr_t off = 0; off + ent <= sz; off += ent) {
    const ElfW(Rel)* r = (const ElfW(Rel)*)(t + off);  // r_offset, r_info lead both layouts
    if (RTYPE(r->r_info) == R_COPYREL && m->base + r->r_offset == addr) return 1;
  }
  return 0;
}

// The number of defined, global or weak dynamic symbols of `m` named
// `name` (several are versions of one name); `*idx` the last one.
static uint32_t defines(const Mod* m, const char* name, uint32_t* idx) {
  if (!m->symtab || !m->strtab) return 0;
  uint32_t found = 0;
  if (m->gnuhash) {
    const uint32_t* h = m->gnuhash;
    uint32_t nb = h[0];
    uint32_t symoff = h[1];
    const uint32_t* buckets = (const uint32_t*)((const char*)(h + 4) + h[2] * sizeof(ElfW(Addr)));
    const uint32_t* chain = buckets + nb;
    uint32_t hv = 5381;
    for (const char* q = name; *q; q++) hv = hv * 33u + (uint32_t)(unsigned char)*q;
    if (!nb) return 0;
    uint32_t i = buckets[hv % nb];
    if (i < symoff) return 0;
    for (;; i++) {
      uint32_t ch = chain[i - symoff];
      if ((ch | 1u) == (hv | 1u)) {
        const ElfW(Sym)* s = &m->symtab[i];
        if (s->st_shndx != SHN_UNDEF && str_eq(m->strtab + s->st_name, name)) {
          found++;
          *idx = i;
        }
      }
      if (ch & 1u) break;
    }
    return found;
  }
  for (uint32_t i = 1; i < m->nsyms; i++) {
    const ElfW(Sym)* s = &m->symtab[i];
    if (s->st_shndx != SHN_UNDEF && (s->st_info >> 4) != STB_LOCAL &&
        str_eq(m->strtab + s->st_name, name)) {
      found++;
      *idx = i;
    }
  }
  return found;
}

struct FindSrc {
  const char* name;
  Mod* out;
  uint32_t idx;
  uint32_t n;  // 0: not found yet
};

static int find_src_cb(struct dl_phdr_info* info, size_t sz, void* data) {
  (void)sz;
  FindSrc* f = (FindSrc*)data;
  if (!(info->dlpi_name && info->dlpi_name[0])) return 0;  // the executable itself
  fill(f->out, info);
  uint32_t idx = 0;
  uint32_t n = defines(f->out, f->name, &idx);
  if (!n) return 0;
  f->n = n;
  f->idx = idx;
  return 1;
}

static void judge(Pass* p, uint32_t sym, uint32_t type, uintptr_t value) {
  const Mod* x = p->x;
  Ctx* c = p->c;
  const ElfW(Sym)* s = &x->symtab[sym];
  const char* name = x->strtab + s->st_name;
  uint64_t key = fnv(name);
  const Row* rx = row_by_sym(x, key, sym);
  if (!rx) return;  // the slow path's candidate without a row
  if (p->flags) {
    if (p->flags[sym] & F_DONE) return;
  } else if (slow_done(p, sym)) {
    return;
  }
  uintptr_t own = s->st_shndx != SHN_UNDEF ? x->base + s->st_value : 0;
  int judged = 1;
  if (value == own) {
    p->n.own++;
  } else if (value >= x->lo && value < x->hi) {
    if (type == R_SLOT) {
      // a lazy slot still points at this module's PLT: not bound yet
      judged = 0;
      if (p->flags) p->flags[sym] = (unsigned char)(p->flags[sym] | F_LAZY);
      else p->n.lazy++;
    } else {
      p->n.unknown++;
    }
  } else {
    const Mod* y = module_of(c, value);
    const Row* ry = 0;
    Mod src;
    int own_copy = 0;
    if (y && y->main && (!y->rows || !(ry = row_by_addr(y, key, value))) && has_copy_at(y, value)) {
      // the executable's copy: judge against the copy's source
      FindSrc f;
      f.name = name;
      f.out = &src;
      f.idx = 0;
      f.n = 0;
      dl_iterate_phdr(find_src_cb, &f);
      p->n.copy++;
      y = 0;
      if (f.n == 1) {  // several versions of the name: which one the copy took is not known here
        y = &src;
        if (src.base == x->base) own_copy = 1;
        else if (src.rows) ry = row_by_sym(&src, key, f.idx);
      }
    } else if (y && y->rows) {
      ry = row_by_addr(y, key, value);
    }
    if (own_copy) {
      p->n.own++;
    } else if (!y) {
      p->n.unknown++;
    } else if (!y->rows) {
      p->n.notable++;
    } else if (!ry) {
      p->n.norow++;
    } else if (((rx->flags | ry->flags) & ROW_SYSTEM) && !c->strict) {
      p->n.system++;
    } else if (rx->hash == ry->hash) {
      p->n.same++;
    } else {
      int pre = is_preloaded(c, y) && !c->strict;
      report(c, x, y, name, rx, ry, pre);
      if (pre) {
        p->n.interposed++;
      } else {
        p->n.differ++;
        c->mismatches++;
      }
    }
  }
  if (judged) {
    if (p->flags) p->flags[sym] = (unsigned char)(p->flags[sym] | F_DONE);
    else {
      p->recent[p->nrecent % 64] = sym;
      p->nrecent++;
    }
  }
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
    if (!sym || (type != R_ABS && type != R_GLOB && type != R_SLOT)) continue;
    if (x->nsyms && sym >= x->nsyms) continue;
    if (p->flags) {
      if (sym >= p->nflags || !(p->flags[sym] & F_ROW) || (p->flags[sym] & F_DONE)) continue;
    }
    p->n.relocs++;
    uintptr_t value = *(const uintptr_t*)(x->base + r_offset);
    if (type != R_SLOT) value = value - (uintptr_t)addend;
    judge(p, sym, type, value);
  }
}

static void add_counts(Counts* t, const Counts* n) {
  t->relocs += n->relocs;
  t->own += n->own;
  t->same += n->same;
  t->differ += n->differ;
  t->interposed += n->interposed;
  t->notable += n->notable;
  t->norow += n->norow;
  t->lazy += n->lazy;
  t->unknown += n->unknown;
  t->system += n->system;
  t->copy += n->copy;
  t->slow += n->slow;
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

static void malformed_warning(const Mod* x) {
  Out o;
  o.n = 0;
  out_str(&o, "mason: warning: odr-hook-malformed-table: ");
  out_str(&o, mod_name(x));
  out_str(&o, ": its table does not parse; the module is NOT checked\n");
  out_flush(&o);
}

// Checks the references of `x` (any module with a table, this runtime's own
// or another's: every byte read is in loaded memory).
static void check_module(Ctx* c, const Mod* x) {
  if (x->malformed) {
    malformed_warning(x);
    return;
  }
  if (!x->rows || !x->dyn) return;
  uintptr_t rela = 0, relasz = 0, rel = 0, relsz = 0, jmprel = 0, pltrelsz = 0, pltrel = 0;
  for (const ElfW(Dyn)* d = x->dyn; d->d_tag != DT_NULL; d++) {
    if (d->d_tag == DT_RELA) rela = dptr(x->base, d->d_un.d_ptr);
    if (d->d_tag == DT_RELASZ) relasz = d->d_un.d_val;
    if (d->d_tag == DT_REL) rel = dptr(x->base, d->d_un.d_ptr);
    if (d->d_tag == DT_RELSZ) relsz = d->d_un.d_val;
    if (d->d_tag == DT_JMPREL) jmprel = dptr(x->base, d->d_un.d_ptr);
    if (d->d_tag == DT_PLTRELSZ) pltrelsz = d->d_un.d_val;
    if (d->d_tag == DT_PLTREL) pltrel = d->d_un.d_val;
  }
  Pass p;
  p.c = c;
  p.x = x;
  p.nrecent = 0;
  Counts zero = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
  p.n = zero;
  uint32_t max = 0;
  for (uint32_t i = 0; i < x->count; i++)
    if (x->rows[i].dynsym > max) max = x->rows[i].dynsym;
  unsigned char stack[STACK_SYMS];
  unsigned char* mapped = 0;
  size_t mapsz = 0;
  p.nflags = max + 1;
  if (p.nflags <= STACK_SYMS) {
    p.flags = stack;
  } else {
    mapsz = ((size_t)p.nflags + 4095) & ~(size_t)4095;
    void* m = c->nommap ? MAP_FAILED : mmap(0, mapsz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) {
      p.flags = 0;
      p.n.slow = 1;
    } else {
      mapped = (unsigned char*)m;
      p.flags = mapped;
    }
  }
  if (p.flags) {
    for (uint32_t i = 0; i < p.nflags; i++) p.flags[i] = 0;
    for (uint32_t i = 0; i < x->count; i++) p.flags[x->rows[i].dynsym] = F_ROW;
  }
  walk(&p, rela, relasz, 1);
  walk(&p, rel, relsz, 0);
  walk(&p, jmprel, pltrelsz, pltrel == DT_RELA);
  if (p.flags) {
    for (uint32_t i = 0; i < p.nflags; i++)
      if ((p.flags[i] & F_LAZY) && !(p.flags[i] & F_DONE)) p.n.lazy++;
  }
  if (mapped) munmap(mapped, mapsz);
  if (p.n.lazy && !c->quiet_lazy) lazy_warning(x, p.n.lazy);
  if (c->stats) {
    Out o;
    o.n = 0;
    out_str(&o, "mason-stats ");
    out_str(&o, mod_name(x));
    out_str(&o, " rows=");
    out_dec(&o, x->count);
    out_str(&o, " relocs=");
    out_dec(&o, p.n.relocs);
    out_str(&o, " own=");
    out_dec(&o, p.n.own);
    out_str(&o, " same=");
    out_dec(&o, p.n.same);
    out_str(&o, " differ=");
    out_dec(&o, p.n.differ);
    out_str(&o, " interposed=");
    out_dec(&o, p.n.interposed);
    out_str(&o, " no-table=");
    out_dec(&o, p.n.notable);
    out_str(&o, " no-row=");
    out_dec(&o, p.n.norow);
    out_str(&o, " lazy=");
    out_dec(&o, p.n.lazy);
    out_str(&o, " unknown=");
    out_dec(&o, p.n.unknown);
    out_str(&o, " copy=");
    out_dec(&o, p.n.copy);
    out_str(&o, " system=");
    out_dec(&o, p.n.system);
    out_str(&o, p.n.slow ? " path=slow\n" : "\n");
    out_flush(&o);
  }
  add_counts(&c->total, &p.n);
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
  else m->dyn = 0;
  return 1;
}

static void fill_main(Mod* m) {
  m->hasrt = 0;
  m->rows = 0;
  m->malformed = 0;
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

static void parse_env(Ctx* c, uint32_t dflt) {
  c->mode = dflt;
  c->strict = 0;
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
      if (*p == ',') p++;
    }
  }
  c->stats = env_get("MASON_ODR_STATS") != 0;
  c->nommap = env_get("MASON_ODR_TEST_NOMMAP") != 0;
  c->preload = env_get("LD_PRELOAD");
}

static void init_ctx(Ctx* c) {
  c->ncache = 0;
  c->next = 0;
  c->mismatches = 0;
  c->modules = 0;
  c->quiet_lazy = 0;
  Counts zero = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
  c->total = zero;
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

static void time_line(uint64_t t0) {
  if (!env_get("MASON_ODR_TIME")) return;
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
  uint64_t t0 = env_get("MASON_ODR_TIME") ? now_ns() : 0;
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
    time_line(t0);
    fatal(&c);
  }
  time_line(t0);
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
  uint64_t t0 = env_get("MASON_ODR_TIME") ? now_ns() : 0;
  Mod self;
  fill_self(&self);
  uint32_t dflt = self.hasrt ? self.rtmode : MODE_REPORT;
  Ctx c;
  init_ctx(&c);
  parse_env(&c, dflt);
  if (c.stats) entry_line("mason-preinit ", mod_name(&self));
  if (c.mode == MODE_OFF) return;
  sweep(&c, 0, c.mode != MODE_FATAL);
  time_line(t0);
  if (c.mismatches && c.mode == MODE_FATAL) fatal(&c);
}

__attribute__((section(".preinit_array"), used)) static void (*const mason_odr_preinit_p)(int, char**, char**) =
    mason_odr_preinit;
#endif
