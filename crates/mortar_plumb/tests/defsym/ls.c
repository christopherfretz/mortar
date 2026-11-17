/* -shared --defsym=calla=b --defsym=k=0x10: calla an alias preemptible
   by its own name. */
int b(void) { return 7; }
int callb(void) { return b(); }
int calla(void);
int via(void) { return calla(); }
