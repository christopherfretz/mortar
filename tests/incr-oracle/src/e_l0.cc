int gdata = 5;
int lf(){ return gdata; }
asm(".pushsection .text.zslack,\"axR\",%progbits\n.balign 256\n.popsection");
asm(".pushsection .data.zslack,\"aw\",%progbits\n.balign 256\n.popsection");
