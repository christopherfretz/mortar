/* WP-077 candidate 2 (glibc's audit interface): which bindings does la_symbind
 * report? Prints every binding of a name that starts with _ZN7fmtlite or is
 * "dvar" (a data symbol, GLOB_DAT), with the binding and the defining module.
 * use: LD_AUDIT=./audit.so PROGRAM ... */
#define _GNU_SOURCE
#include <link.h>
#include <stdio.h>
#include <string.h>
unsigned int la_version(unsigned int v) { return v; }
unsigned int la_objopen(struct link_map* map, Lmid_t lmid, uintptr_t* cookie) {
  *cookie = (uintptr_t)map;
  fprintf(stderr, "audit: objopen %s (ns %ld)\n", map->l_name[0] ? map->l_name : "(main)", (long)lmid);
  return LA_FLG_BINDTO | LA_FLG_BINDFROM;
}
static const char* nm(uintptr_t* c) { struct link_map* m = (struct link_map*)*c; const char* s = strrchr(m->l_name, '/'); return s ? s + 1 : m->l_name[0] ? m->l_name : "(main)"; }
uintptr_t la_symbind64(Elf64_Sym* sym, unsigned int ndx, uintptr_t* refcook, uintptr_t* defcook, unsigned int* flags, const char* symname) {
  (void)ndx; (void)flags;
  if (!strncmp(symname, "_ZN7fmtlite", 11) || !strcmp(symname, "dvar"))
    fprintf(stderr, "audit: symbind %s: %s -> %s%s\n", symname, nm(refcook), nm(defcook), (*flags & LA_SYMB_DLSYM) ? " (dlsym)" : "");
  return sym->st_value;
}
