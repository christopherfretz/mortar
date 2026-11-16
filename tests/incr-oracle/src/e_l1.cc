int gpad[4] = {1, 2, 3, 4};
int gdata = 5;
int lf(){ return gdata + gpad[1]; }
asm(".pushsection .text.zslack,\"axR\",%progbits\n.balign 256\n.popsection");
asm(".pushsection .data.zslack,\"aw\",%progbits\n.balign 256\n.popsection");
