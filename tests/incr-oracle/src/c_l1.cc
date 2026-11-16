int other(int);
int last_fn(int x){ return other(x) * 11 + 1; }
asm(".pushsection .text.zslack,\"axR\",%progbits\n.balign 256\n.popsection");
