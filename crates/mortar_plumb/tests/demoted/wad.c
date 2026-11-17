__asm__(".section .data.g,\"awG\",%progbits,g,comdat\n"
        ".weak g\n.type g,%object\n.size g,4\ng: .long 1\n.previous\n");
int Y(void);
extern int g;
void _start(void) { Y(); (void)g; for (;;) {} }
