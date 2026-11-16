__attribute__((section("ureg"), used)) const int ureg_a = 11;
__attribute__((section("ureg"), used)) const int ureg_b = 31;
int keepf(int x){ return x * 3; }
int dropf(int x){ return x * 5; }
asm(".pushsection .meta_k,\"ao\",%progbits,.text.keepf\n.word 0x6b6b6b6b\n.popsection");
asm(".pushsection .meta_d,\"ao\",%progbits,.text.dropf\n.word 0x64646464\n.popsection");
asm(".pushsection .text.gfun,\"axG\",%progbits,gsig,comdat\n.globl gfun\n.type gfun,%function\n.p2align 2\ngfun:\n mov w0, #7\n ret\n.size gfun,8\n.popsection");
asm(".pushsection .rodata.gaux,\"aG\",%progbits,gsig,comdat\n.word 0x67676767\n.popsection");
