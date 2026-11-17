extern int g;
extern int hx __attribute__((weak));
__asm__(".section .data.g,\"awG\",%progbits,g,comdat\n"
        ".weak g\n.type g,%object\n.size g,4\ng: .long 2\n"
        ".weak hx\n.type hx,%object\n.size hx,4\nhx: .long 40\n.previous\n");
int Y(void) { return &hx ? hx : 0; }
