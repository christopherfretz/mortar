#include <stdio.h>
int probe(void);
int main(void) { int m = probe(); const char *k[] = {"FUNC","OBJECT","NOTYPE","TLS","IFUNC"};
  for (int b = 0; b < 5; b++) printf("%s=%c ", k[b], (m >> b) & 1 ? 'P' : 'B'); printf("\n"); return 0; }
