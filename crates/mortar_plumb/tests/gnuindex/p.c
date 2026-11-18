/* WP-251: g++/gcc -gpubnames, no split DWARF: .debug_pubnames and
   .debug_pubtypes, not in IN-55 option (A)'s list (kept in every output). */
struct pubtype { int v; };
struct pubtype pubvar;
int pubfn(void) { return pubvar.v; }
