int helper(int);
int last_fn(int x){ return helper(x) + 1; }
asm(".pushsection .text.zslack,\"axR\",%progbits\n.balign 256\n.popsection");
