/* GCC section anchors: the exported datum is reached only at anchor + immediate. */
static int h = 1;
int g = 2;
int both(void) { return h + g; }
