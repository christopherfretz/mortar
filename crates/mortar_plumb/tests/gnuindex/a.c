/* WP-251: a -gsplit-dwarf object (GCC: .debug_gnu_pubnames and
   .debug_gnu_pubtypes in the object, the skeleton unit in .debug_info). */
struct point { int x; int y; };
int helper(int);
struct point origin;
void _start(void) {
  struct point p = { helper(2), 4 };
  origin = p;
  for (;;) {
  }
}
