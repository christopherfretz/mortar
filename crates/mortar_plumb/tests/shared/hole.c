/* D-117's residual hole: an address combined with a register. h is
   local and writable, the exported hg beside it; h's anchor plus a
   register index reaches hg, which no rule follows. */
static int h[2] = {1, 2};
int hg = 3;
int hole(long i) { return h[i]; }
void hole_set(long i, int v) { h[i] = v; }
