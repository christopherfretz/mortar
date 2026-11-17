/* WP-077: the audit interface's only refusal: la_objsearch returning NULL for a
 * name makes dlopen fail before the file is mapped (no binding exists yet). */
#define _GNU_SOURCE
#include <link.h>
#include <stdio.h>
#include <string.h>
unsigned int la_version(unsigned int v) { return v; }
char* la_objsearch(const char* name, uintptr_t* cookie, unsigned int flag) {
  (void)cookie; (void)flag;
  if (strstr(name, "h2.so")) { fprintf(stderr, "audit: refusing %s\n", name); return NULL; }
  return (char*)name;
}
unsigned int la_objopen(struct link_map* map, Lmid_t lmid, uintptr_t* cookie) { (void)map; (void)lmid; (void)cookie; return 0; }
