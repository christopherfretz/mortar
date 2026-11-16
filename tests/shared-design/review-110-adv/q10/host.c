#include <stdio.h>
extern const unsigned gdk_pixbuf_major_version, gdk_pixbuf_minor_version;
int main(void){ printf("gdk-pixbuf %u.%u (copied, read-only: correct)\n", gdk_pixbuf_major_version, gdk_pixbuf_minor_version); return 0; }
