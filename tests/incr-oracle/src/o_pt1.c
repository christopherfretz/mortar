int lf(int x){ return x + 1; }
asm(".pushsection .text.o_pad,\"ax\",%progbits\n.space 5000\n.popsection");
